//! Subscription parsing: converts any format the desktop client accepts into
//! a full mihomo config. Clash/mihomo YAML passes through (device header
//! re-applied); base64 node lists and raw share links (ss://, ssr://,
//! vmess://, vless://, trojan://, hysteria2://, tuic://) are parsed into
//! proxies + proxy-groups + rules.
//!
//! The parsers live here so every profile on disk is always loadable by the
//! core — the historical bug (raw subscription written into config.yaml,
//! crash loop, "无法启动 FlClash 服务") cannot recur through this path.

use base64::Engine;
use serde_json::Value;
use std::path::Path;

use crate::ctrl::{curl_fetch, header_value, percent_decode, percent_decode_plus};

// ---------------------------------------------------------------- yaml emit

fn yaml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Clone)]
pub struct Kv {
    pub k: &'static str,
    pub v: String,
}

fn kv(k: &'static str, v: impl Into<String>) -> Kv {
    Kv { k, v: v.into() }
}

fn emit_node(pairs: &[Kv]) -> String {
    let mut out = String::new();
    for (i, item) in pairs.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("  - {}: {}\n", item.k, item.v));
        } else {
            out.push_str(&format!("    {}: {}\n", item.k, item.v));
        }
    }
    out
}

// ------------------------------------------------------------- base64 input

/// Tries standard/url alphabets with and without padding, ignoring
/// whitespace, optionally percent-unfolding first. Mirrors b64Decode().
pub fn b64_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut s: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    if s.is_empty() {
        return Err("empty base64".into());
    }
    if s.contains('%') {
        s = percent_decode(&s);
    }
    let pad = (4 - s.len() % 4) % 4;
    let padded = format!("{}{}", s, "=".repeat(pad));
    let raw = s.trim_end_matches('=').to_string();
    let attempts: [(&base64::engine::general_purpose::GeneralPurpose, &str); 4] = [
        (&base64::engine::general_purpose::STANDARD, &padded),
        (&base64::engine::general_purpose::URL_SAFE, &padded),
        (&base64::engine::general_purpose::STANDARD_NO_PAD, &raw),
        (&base64::engine::general_purpose::URL_SAFE_NO_PAD, &raw),
    ];
    let mut first_err = None;
    for (engine, input) in attempts {
        match engine.decode(input) {
            Ok(v) if !v.is_empty() => return Ok(v),
            Ok(_) => first_err = first_err.or_else(|| Some("empty payload".to_string())),
            Err(e) => first_err = first_err.or_else(|| Some(e.to_string())),
        }
    }
    Err(first_err.unwrap_or_else(|| "base64 decode failed".into()))
}

// ------------------------------------------------------------ url utilities

fn split_host_port(s: &str) -> Result<(String, u16), String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| "bad ipv6 literal".to_string())?;
        let host = &rest[..end];
        let after = rest[end + 1..].strip_prefix(':').unwrap_or("");
        let port: u16 = after
            .split('/')
            .next()
            .unwrap_or("")
            .parse()
            .map_err(|_| "bad port".to_string())?;
        return Ok((host.to_string(), port));
    }
    let i = s
        .rfind(':')
        .ok_or_else(|| "missing port".to_string())?;
    let port: u16 = s[i + 1..]
        .split('/')
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "bad port".to_string())?;
    Ok((s[..i].to_string(), port))
}

/// Parses `k=v&k=v` the way Go's url.ParseQuery does (percent-decoding with
/// '+' treated as space).
fn parse_query(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in s.split('&') {
        if part.is_empty() {
            continue;
        }
        let (k, v) = match part.split_once('=') {
            Some((k, v)) => (k, v),
            None => (part, ""),
        };
        out.push((percent_decode_plus(k), percent_decode_plus(v)));
    }
    out
}

fn qget<'a>(query: &'a [(String, String)], key: &str) -> &'a str {
    query
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

fn qget_any<'a>(query: &'a [(String, String)], keys: &[&str]) -> &'a str {
    for key in keys {
        let v = qget(query, key);
        if !v.is_empty() {
            return v;
        }
    }
    ""
}

fn split_plugin_opts(plugin: &str) -> Vec<(String, String)> {
    plugin
        .split(';')
        .skip(1)
        .filter_map(|part| match part.split_once('=') {
            Some((k, v)) => Some((k.to_string(), v.to_string())),
            None if !part.is_empty() => Some((part.to_string(), String::new())),
            None => None,
        })
        .collect()
}

fn opts_get<'a>(opts: &'a [(String, String)], key: &str) -> &'a str {
    opts.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

// ---------------------------------------------------------------- parsers

fn parse_ss(raw: &str) -> Result<Vec<Kv>, String> {
    let body = raw
        .trim_start_matches("ss://")
        .trim_start_matches("SS://");
    let (body, frag) = match body.split_once('#') {
        Some((b, f)) => (b, f),
        None => (body, ""),
    };
    let name = percent_decode_plus(frag);

    let (method, password, target);
    if let Some(at) = body.rfind('@') {
        let userinfo = &body[..at];
        let rest = &body[at + 1..];
        let mut cred = userinfo.to_string();
        if let Ok(decoded) = b64_decode(userinfo) {
            let text = String::from_utf8_lossy(&decoded).into_owned();
            if text.contains(':') {
                cred = text;
            }
        }
        if !cred.contains(':') {
            let unescaped = percent_decode_plus(userinfo);
            if unescaped.contains(':') {
                cred = unescaped;
            }
        }
        let ci = cred
            .find(':')
            .ok_or_else(|| "bad ss userinfo".to_string())?;
        method = cred[..ci].to_string();
        password = cred[ci + 1..].to_string();
        target = rest.to_string();
    } else {
        let decoded = b64_decode(body)?;
        let cred = String::from_utf8_lossy(&decoded).into_owned();
        let ai = cred
            .rfind('@')
            .ok_or_else(|| "bad ss legacy body".to_string())?;
        let ci = cred[..ai]
            .find(':')
            .ok_or_else(|| "bad ss legacy userinfo".to_string())?;
        method = cred[..ci].to_string();
        password = cred[ci + 1..ai].to_string();
        target = cred[ai + 1..].to_string();
    }

    let (hostport, query) = match target.split_once('?') {
        Some((hp, q)) => (hp.to_string(), parse_query(q)),
        None => (target.clone(), Vec::new()),
    };
    let hostport = hostport.split('/').next().unwrap_or("").trim_end_matches('/');
    let (host, port) = split_host_port(hostport)?;

    let mut pairs = vec![
        kv("name", yaml_str(&name)),
        kv("type", "ss"),
        kv("server", yaml_str(&host)),
        kv("port", port.to_string()),
        kv("cipher", yaml_str(&method)),
        kv("password", yaml_str(&password)),
        kv("udp", "true"),
    ];
    let plugin = qget(&query, "plugin");
    if !plugin.is_empty() {
        let opts = split_plugin_opts(plugin);
        if plugin.starts_with("obfs-local") {
            let mode = if opts_get(&opts, "obfs").is_empty() {
                "http".to_string()
            } else {
                opts_get(&opts, "obfs").to_string()
            };
            pairs.push(kv("plugin", "obfs"));
            pairs.push(kv(
                "plugin-opts",
                format!("{{mode: {}, host: {}}}", yaml_str(&mode), yaml_str(opts_get(&opts, "obfs-host"))),
            ));
        } else if plugin.starts_with("v2ray-plugin") {
            let mode = if opts_get(&opts, "mode").is_empty() {
                "websocket".to_string()
            } else {
                opts_get(&opts, "mode").to_string()
            };
            let tls = if opts.iter().any(|(k, _)| k == "tls") { "true" } else { "false" };
            pairs.push(kv("plugin", "v2ray-plugin"));
            pairs.push(kv("plugin-opts", format!("{{mode: {}, tls: {}}}", yaml_str(&mode), tls)));
        }
    }
    Ok(pairs)
}

fn parse_ssr(raw: &str) -> Result<Vec<Kv>, String> {
    let body = raw.trim_start_matches("ssr://").trim_start_matches("SSR://");
    let decoded = b64_decode(body)?;
    let text = String::from_utf8_lossy(&decoded).into_owned();
    let (main, query) = match text.split_once("/?") {
        Some((m, q)) => (m.to_string(), q.to_string()),
        None => (text.clone(), String::new()),
    };
    let fields: Vec<&str> = main.split(':').collect();
    if fields.len() < 6 {
        return Err("bad ssr body".into());
    }
    let host = fields[..fields.len() - 5].join(":");
    let port: u16 = fields[fields.len() - 5]
        .parse()
        .map_err(|_| "bad ssr port".to_string())?;
    let protocol = fields[fields.len() - 4];
    let method = fields[fields.len() - 3];
    let obfs = fields[fields.len() - 2];
    let password = String::from_utf8_lossy(&b64_decode(fields[fields.len() - 1])?).into_owned();

    let mut pairs = vec![
        kv("name", yaml_str(&host)),
        kv("type", "ssr"),
        kv("server", yaml_str(&host)),
        kv("port", port.to_string()),
        kv("protocol", yaml_str(protocol)),
        kv("cipher", yaml_str(method)),
        kv("obfs", yaml_str(obfs)),
        kv("password", yaml_str(&password)),
    ];
    let params = parse_query(&query);
    let dec_param = |key: &str| -> Option<String> {
        let v = qget(&params, key);
        if v.is_empty() {
            return None;
        }
        b64_decode(v).ok().map(|d| String::from_utf8_lossy(&d).into_owned())
    };
    if let Some(v) = dec_param("obfsparam") {
        pairs.push(kv("obfs-param", yaml_str(&v)));
    }
    if let Some(v) = dec_param("protoparam") {
        pairs.push(kv("protocol-param", yaml_str(&v)));
    }
    if let Some(v) = dec_param("remarks") {
        if !v.is_empty() {
            pairs[0].v = yaml_str(&v);
        }
    }
    Ok(pairs)
}

fn json_str(data: &Value, key: &str, fallback: &str) -> String {
    match data.get(key) {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) if n.as_f64().unwrap_or(0.0) > 0.0 => {
            format!("{}", n.as_i64().unwrap_or(0))
        }
        _ => fallback.to_string(),
    }
}

fn parse_vmess(raw: &str) -> Result<Vec<Kv>, String> {
    let body = raw.trim_start_matches("vmess://");
    let decoded = b64_decode(body)?;
    let data: Value =
        serde_json::from_slice(&decoded).map_err(|e| format!("bad vmess json: {}", e))?;

    let port: u16 = json_str(&data, "port", "0")
        .parse()
        .map_err(|_| "bad vmess port".to_string())?;
    if port == 0 {
        return Err("bad vmess port".into());
    }
    let name = {
        let ps = json_str(&data, "ps", "");
        if ps.is_empty() {
            json_str(&data, "add", "vmess")
        } else {
            ps
        }
    };
    let net = {
        let n = json_str(&data, "net", "");
        if n.is_empty() {
            "tcp".to_string()
        } else {
            n
        }
    };

    let mut pairs = vec![
        kv("name", yaml_str(&name)),
        kv("type", "vmess"),
        kv("server", yaml_str(&json_str(&data, "add", ""))),
        kv("port", port.to_string()),
        kv("uuid", yaml_str(&json_str(&data, "id", ""))),
        kv("alterId", json_str(&data, "aid", "0")),
        kv("cipher", yaml_str(&{
            let scy = json_str(&data, "scy", "");
            if scy.is_empty() { "auto".to_string() } else { scy }
        })),
        kv("udp", "true"),
    ];
    let tls = json_str(&data, "tls", "");
    if tls == "tls" || tls == "true" || tls == "1" {
        pairs.push(kv("tls", "true"));
        let sni = {
            let s = json_str(&data, "sni", "");
            if s.is_empty() {
                json_str(&data, "host", "")
            } else {
                s
            }
        };
        if !sni.is_empty() {
            pairs.push(kv("servername", yaml_str(&sni)));
        }
    }
    match net.as_str() {
        "ws" => {
            pairs.push(kv("network", "ws"));
            let mut opts = format!("{{path: {}", yaml_str(&json_str(&data, "path", "/")));
            let host = json_str(&data, "host", "");
            if !host.is_empty() {
                opts.push_str(&format!(", headers: {{Host: {}}}", yaml_str(&host)));
            }
            opts.push('}');
            pairs.push(kv("ws-opts", opts));
        }
        "grpc" => {
            pairs.push(kv("network", "grpc"));
            pairs.push(kv(
                "grpc-opts",
                format!("{{grpc-service-name: {}}}", yaml_str(&json_str(&data, "path", ""))),
            ));
        }
        "h2" => {
            pairs.push(kv("network", "h2"));
            pairs.push(kv(
                "h2-opts",
                format!(
                    "{{host: [{}], path: {}}}",
                    yaml_str(&json_str(&data, "host", "")),
                    yaml_str(&json_str(&data, "path", "/"))
                ),
            ));
        }
        _ => {}
    }
    Ok(pairs)
}

fn csv_yaml_list(value: &str) -> String {
    let items: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(yaml_str)
        .collect();
    format!("[{}]", items.join(", "))
}

/// mihomo accepts a bare number (Mbps) or a value with a unit. Keep the unit
/// when the link already carries one, otherwise make the unit explicit.
fn mbps_yaml(value: &str) -> String {
    let v = value.trim();
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') {
        yaml_str(&format!("{} Mbps", v))
    } else {
        yaml_str(v)
    }
}

fn truthy(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "Yes")
}

/// Hysteria v1 share link:
///   hysteria://[auth@]host:port?protocol=udp&auth=...&peer=sni
///              &insecure=1&upmbps=100&downmbps=100&alpn=h3
///              &obfsParam=...&mport=443,5000-6000#name
///
/// Unlike vless/trojan, the auth string is usually a query parameter instead
/// of userinfo, so `@` must stay optional.
fn parse_hysteria(raw: &str) -> Result<Vec<Kv>, String> {
    let body = raw.trim_start_matches("hysteria://");
    let (body, frag) = match body.split_once('#') {
        Some((b, f)) => (b, f),
        None => (body, ""),
    };
    let name = percent_decode_plus(frag);
    let (body, query_str) = match body.split_once('?') {
        Some((b, q)) => (b, q),
        None => (body, ""),
    };
    let query = parse_query(query_str);

    let (cred, hostport) = match body.rfind('@') {
        Some(i) => (Some(&body[..i]), &body[i + 1..]),
        None => (None, body),
    };
    let (host, port) = split_host_port(hostport)?;

    let auth = {
        let q = qget_any(&query, &["auth", "auth_str", "auth-str"]);
        if q.is_empty() {
            cred.map(percent_decode).unwrap_or_default()
        } else {
            q.to_string()
        }
    };

    let mut pairs = vec![
        kv("name", yaml_str(&name)),
        kv("type", "hysteria"),
        kv("server", yaml_str(&host)),
        kv("port", port.to_string()),
    ];

    if !auth.is_empty() {
        pairs.push(kv("auth-str", yaml_str(&auth)));
    }

    let sni = qget_any(&query, &["peer", "sni", "server_name"]);
    if !sni.is_empty() {
        pairs.push(kv("sni", yaml_str(sni)));
    }

    let insecure = qget_any(&query, &["insecure", "allowInsecure", "allow_insecure"]);
    if truthy(insecure) {
        pairs.push(kv("skip-cert-verify", "true"));
    }

    let up = qget_any(&query, &["upmbps", "up", "up_mbps"]);
    if !up.is_empty() {
        pairs.push(kv("up", mbps_yaml(up)));
    }
    let down = qget_any(&query, &["downmbps", "down", "down_mbps"]);
    if !down.is_empty() {
        pairs.push(kv("down", mbps_yaml(down)));
    }

    let protocol = qget(&query, "protocol");
    if !protocol.is_empty() {
        pairs.push(kv("protocol", yaml_str(protocol)));
    }

    // Hysteria v1 obfuscation is a single password (no type selector).
    let obfs = qget_any(&query, &["obfsParam", "obfs-password", "obfs_password", "obfs"]);
    if !obfs.is_empty() {
        pairs.push(kv("obfs", yaml_str(obfs)));
    }

    let alpn = qget(&query, "alpn");
    if !alpn.is_empty() {
        pairs.push(kv("alpn", csv_yaml_list(alpn)));
    }

    let ports = qget_any(&query, &["mport", "ports"]);
    if !ports.is_empty() {
        pairs.push(kv("ports", yaml_str(ports)));
    }

    Ok(pairs)
}

fn parse_share_url(raw: &str, scheme: &str) -> Result<Vec<Kv>, String> {
    let prefix = format!("{}://", scheme);
    let body = raw.trim_start_matches(&prefix);
    let (body, frag) = match body.split_once('#') {
        Some((b, f)) => (b, f),
        None => (body, ""),
    };
    let name = percent_decode_plus(frag);
    let (body, query_str) = match body.split_once('?') {
        Some((b, q)) => (b, q),
        None => (body, ""),
    };
    let query = parse_query(query_str);

    let (cred, hostport) = match body.rfind('@') {
        Some(at) => (&body[..at], &body[at + 1..]),
        // AnyTLS links may omit userinfo and carry ?password= instead.
        None if scheme == "anytls" => ("", body),
        None => return Err(format!("missing @ in {} link", scheme)),
    };
    let (host, port) = split_host_port(hostport)?;

    let mut pairs;
    match scheme {
        "vless" => {
            pairs = vec![
                kv("name", yaml_str(&name)),
                kv("type", "vless"),
                kv("server", yaml_str(&host)),
                kv("port", port.to_string()),
                kv("uuid", yaml_str(cred)),
                kv("udp", "true"),
            ];
            let flow = qget(&query, "flow");
            if !flow.is_empty() {
                pairs.push(kv("flow", yaml_str(flow)));
            }
            let mut security = qget(&query, "security").to_string();
            if security.is_empty() && !qget(&query, "pbk").is_empty() {
                security = "reality".into();
            }
            if security.is_empty() {
                security = "none".into();
            }
            if security != "none" {
                pairs.push(kv("tls", "true"));
            }
            let sni = qget_any(&query, &["sni", "peer"]);
            if !sni.is_empty() {
                pairs.push(kv("servername", yaml_str(sni)));
            }
            let fp = qget(&query, "fp");
            if !fp.is_empty() {
                pairs.push(kv("client-fingerprint", yaml_str(fp)));
            }
            let pbk = qget(&query, "pbk");
            if !pbk.is_empty() {
                pairs.push(kv(
                    "reality-opts",
                    format!("{{public-key: {}, short-id: {}}}", yaml_str(pbk), yaml_str(qget(&query, "sid"))),
                ));
            }
            ws_grpc_opts(&mut pairs, qget(&query, "type"), &query);
        }
        "trojan" => {
            pairs = vec![
                kv("name", yaml_str(&name)),
                kv("type", "trojan"),
                kv("server", yaml_str(&host)),
                kv("port", port.to_string()),
                kv("password", yaml_str(cred)),
                kv("udp", "true"),
            ];
            let sni = qget_any(&query, &["sni", "peer"]);
            if !sni.is_empty() {
                pairs.push(kv("sni", yaml_str(sni)));
            }
            let insecure = qget_any(&query, &["allowInsecure", "insecure"]);
            if insecure == "1" || insecure == "true" {
                pairs.push(kv("skip-cert-verify", "true"));
            }
            ws_grpc_opts(&mut pairs, qget(&query, "type"), &query);
        }
        "hysteria2" => {
            pairs = vec![
                kv("name", yaml_str(&name)),
                kv("type", "hysteria2"),
                kv("server", yaml_str(&host)),
                kv("port", port.to_string()),
                kv("password", yaml_str(cred)),
            ];
            let sni = qget_any(&query, &["sni", "peer"]);
            if !sni.is_empty() {
                pairs.push(kv("sni", yaml_str(sni)));
            }
            let insecure = qget(&query, "insecure");
            if insecure == "1" || insecure == "true" {
                pairs.push(kv("skip-cert-verify", "true"));
            }
            let obfs = qget(&query, "obfs");
            if !obfs.is_empty() {
                pairs.push(kv("obfs", yaml_str(obfs)));
                let pw = qget(&query, "obfs-password");
                if !pw.is_empty() {
                    pairs.push(kv("obfs-password", yaml_str(pw)));
                }
            }
        }
        "tuic" => {
            let (uuid, password) = match cred.split_once(':') {
                Some((u, p)) => (u.to_string(), p.to_string()),
                None => (cred.to_string(), String::new()),
            };
            pairs = vec![
                kv("name", yaml_str(&name)),
                kv("type", "tuic"),
                kv("server", yaml_str(&host)),
                kv("port", port.to_string()),
                kv("uuid", yaml_str(&uuid)),
                kv("password", yaml_str(&password)),
            ];
            let sni = qget(&query, "sni");
            if !sni.is_empty() {
                pairs.push(kv("sni", yaml_str(sni)));
            }
            let cc = qget_any(&query, &["congestion_control", "congestion-controller"]);
            if !cc.is_empty() {
                pairs.push(kv("congestion-controller", yaml_str(cc)));
            }
            let alpn = qget(&query, "alpn");
            if !alpn.is_empty() {
                let items: Vec<String> = alpn
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .map(yaml_str)
                    .collect();
                pairs.push(kv("alpn", format!("[{}]", items.join(", "))));
            }
            let mode = qget(&query, "udp_relay_mode");
            if !mode.is_empty() {
                pairs.push(kv("udp-relay-mode", yaml_str(mode)));
            }
        }
        "anytls" => {
            // anytls://password@host:port?insecure=1&sni=...&fp=...
            // Password lives in userinfo; accept ?password= as a fallback.
            let password = {
                let q = qget_any(&query, &["password", "auth", "auth_str", "auth-str"]);
                if q.is_empty() {
                    percent_decode(cred)
                } else {
                    q.to_string()
                }
            };
            pairs = vec![
                kv("name", yaml_str(&name)),
                kv("type", "anytls"),
                kv("server", yaml_str(&host)),
                kv("port", port.to_string()),
                kv("password", yaml_str(&password)),
                kv("udp", "true"),
            ];
            let sni = qget_any(&query, &["sni", "peer", "server_name"]);
            if !sni.is_empty() {
                pairs.push(kv("sni", yaml_str(sni)));
            }
            let insecure = qget_any(&query, &["insecure", "allowInsecure", "allow_insecure"]);
            if truthy(insecure) {
                pairs.push(kv("skip-cert-verify", "true"));
            }
            let fp = qget_any(&query, &["fp", "client-fingerprint", "client_fingerprint"]);
            if !fp.is_empty() {
                pairs.push(kv("client-fingerprint", yaml_str(fp)));
            }
            let alpn = qget(&query, "alpn");
            if !alpn.is_empty() {
                pairs.push(kv("alpn", csv_yaml_list(alpn)));
            }
            let pin = qget_any(&query, &["pinSHA256", "pin-sha256"]);
            if !pin.is_empty() {
                pairs.push(kv("fingerprint", yaml_str(pin)));
            }
        }
        _ => return Err(format!("unsupported scheme {}", scheme)),
    }
    Ok(pairs)
}

fn ws_grpc_opts(pairs: &mut Vec<Kv>, network: &str, query: &[(String, String)]) {
    match network {
        "ws" => {
            pairs.push(kv("network", "ws"));
            let path = {
                let p = qget(query, "path");
                if p.is_empty() {
                    "/".to_string()
                } else {
                    p.to_string()
                }
            };
            let mut opts = format!("{{path: {}", yaml_str(&path));
            let host = qget(query, "host");
            if !host.is_empty() {
                opts.push_str(&format!(", headers: {{Host: {}}}", yaml_str(host)));
            }
            opts.push('}');
            pairs.push(kv("ws-opts", opts));
        }
        "grpc" => {
            pairs.push(kv("network", "grpc"));
            let svc = qget_any(query, &["serviceName", "path"]);
            pairs.push(kv("grpc-opts", format!("{{grpc-service-name: {}}}", yaml_str(svc))));
        }
        _ => {}
    }
}

// ------------------------------------------------------------ node registry

const SUPPORTED_SCHEMES: [&str; 10] = [
    "ss", "ssr", "vmess", "vless", "trojan", "hysteria2", "hy2", "hysteria", "tuic", "anytls",
];

pub fn scheme_of(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    for scheme in SUPPORTED_SCHEMES {
        if lower.starts_with(&format!("{}://", scheme)) {
            if scheme == "hy2" {
                return "hy2";
            }
            return scheme;
        }
    }
    ""
}

fn parse_node(line: &str) -> Result<Vec<Kv>, String> {
    match scheme_of(line) {
        "ss" => parse_ss(line),
        "ssr" => parse_ssr(line),
        "vmess" => parse_vmess(line),
        "vless" => parse_share_url(line, "vless"),
        "trojan" => parse_share_url(line, "trojan"),
        "hysteria2" => parse_share_url(line, "hysteria2"),
        "hy2" => parse_share_url(line.trim_start_matches("hy2://").trim_start_matches("hysteria2://"), "hysteria2"),
        "hysteria" => parse_hysteria(line),
        "tuic" => parse_share_url(line, "tuic"),
        "anytls" => parse_share_url(line, "anytls"),
        _ => Err("unsupported node line".into()),
    }
}

fn node_lines_from(body: &str) -> (Vec<Vec<Kv>>, usize) {
    let mut out = Vec::new();
    let mut skipped = 0;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || scheme_of(line).is_empty() {
            continue;
        }
        match parse_node(line) {
            Ok(pairs) => out.push(pairs),
            Err(_) => skipped += 1,
        }
    }
    (out, skipped)
}

// -------------------------------------------------------------- yaml assembly

#[allow(dead_code)] // documents subscription header priority order
pub const HEADER_ORDER: [&str; 10] = [
    "mixed-port",
    "socks-port",
    "port",
    "redir-port",
    "tproxy-port",
    "external-controller",
    "mode",
    "log-level",
    "allow-lan",
    "ipv6",
];

const TOP_LEVEL_PORT_KEYS: [&str; 11] = [
    "mixed-port",
    "socks-port",
    "port",
    "redir-port",
    "tproxy-port",
    "external-controller",
    "mode",
    "log-level",
    "allow-lan",
    "ipv6",
    "secret",
];

/// Device-critical scalar keys, preferring values from the live config file.
pub fn header_map(config_path: &Path) -> Vec<(&'static str, String)> {
    let mut map: Vec<(&'static str, String)> = vec![
        ("mixed-port", "7892".into()),
        ("socks-port", "7891".into()),
        ("port", "0".into()),
        ("redir-port", "7893".into()),
        ("tproxy-port", "7894".into()),
        ("external-controller", "127.0.0.1:9097".into()),
        ("mode", "rule".into()),
        ("log-level", "info".into()),
        ("allow-lan", "false".into()),
        ("ipv6", "true".into()),
    ];
    let Ok(data) = std::fs::read_to_string(config_path) else {
        return map;
    };
    for line in data.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if let Some(slot) = map.iter_mut().find(|(k, _)| *k == key.trim()) {
            slot.1 = value.to_string();
        }
    }
    map
}

pub fn count_top_level_list(text: &str, key: &str) -> usize {
    let mut in_section = false;
    let mut count = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let at_root = !line.starts_with(' ') && !line.starts_with('\t') && !line.starts_with('#');
        if at_root {
            in_section = trimmed.starts_with(&format!("{}:", key));
            continue;
        }
        if in_section && (trimmed.starts_with("- ") || trimmed == "-") {
            count += 1;
        }
    }
    count
}

pub fn looks_like_yaml(text: &str) -> bool {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let at_root = !line.starts_with(' ') && !line.starts_with('\t');
        return at_root && (trimmed.ends_with(':') || trimmed.contains(": "));
    }
    false
}

fn write_header(out: &mut String, header: &[(&'static str, String)]) {
    for (key, value) in header {
        out.push_str(&format!("{}: {}\n", key, value));
    }
}

fn build_node_config(header: &[(&'static str, String)], nodes: Vec<Vec<Kv>>) -> String {
    let mut names: Vec<String> = Vec::with_capacity(nodes.len());
    let mut used: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut proxies = String::new();
    for (i, pairs) in nodes.iter().enumerate() {
        let base = pairs
            .iter()
            .find(|item| item.k == "name")
            .map(|item| item.v.trim_matches('"').to_string())
            .unwrap_or_default();
        let base = if base.is_empty() {
            format!("节点 {}", i + 1)
        } else {
            base
        };
        let counter = used.entry(base.clone()).and_modify(|e| *e += 1).or_insert(1);
        let name = if *counter == 1 {
            base.clone()
        } else {
            format!("{} {}", base, *counter)
        };
        let mut owned = pairs.to_vec();
        if name != base {
            for item in owned.iter_mut() {
                if item.k == "name" {
                    item.v = yaml_str(&name);
                }
            }
        }
        names.push(name);
        proxies.push_str(&emit_node(&owned));
    }

    let quoted: Vec<String> = names.iter().map(|n| yaml_str(n)).collect();
    let mut out = String::new();
    write_header(&mut out, header);
    out.push_str("\ndns:\n");
    out.push_str("  enable: true\n");
    // Default to redir-host (real IPs) rather than fake-ip. The router's
    // transparent proxy hijacks port 53 and feeds the core a DNS listener;
    // real-IP answers let nftables REDIRECT/TPROXY do IP-based splitting and
    // avoid handing LAN clients unreachable 198.18.x addresses.
    out.push_str("  enhanced-mode: redir-host\n");
    out.push_str("  fake-ip-range: 198.18.0.1/16\n");
    out.push_str("  fake-ip-filter:\n");
    out.push_str("    - '*.lan'\n");
    out.push_str("    - '*.local'\n");
    out.push_str("    - '+.msftconnecttest.com'\n");
    out.push_str("    - '+.msftncsi.com'\n");
    out.push_str("  default-nameserver:\n");
    out.push_str("    - 223.5.5.5\n");
    out.push_str("    - 119.29.29.29\n");
    out.push_str("  nameserver:\n");
    out.push_str("    - https://223.5.5.5/dns-query\n");
    out.push_str("    - https://doh.pub/dns-query\n");
    out.push_str("    - tls://120.53.53.53\n");
    out.push_str("\nproxies:\n");
    out.push_str(&proxies);

    out.push_str("\nproxy-groups:\n");
    out.push_str(&emit_node(&[
        kv("name", yaml_str("🚀 节点选择")),
        kv("type", "select"),
        kv(
            "proxies",
            format!(
                "[{}]",
                [yaml_str("♻️ 自动选择"), yaml_str("DIRECT")]
                    .into_iter()
                    .chain(quoted.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    ]));
    out.push_str(&emit_node(&[
        kv("name", yaml_str("♻️ 自动选择")),
        kv("type", "url-test"),
        kv("proxies", format!("[{}]", quoted.join(", "))),
        kv("url", yaml_str("http://www.gstatic.com/generate_204")),
        kv("interval", "300"),
        kv("tolerance", "50"),
    ]));
    out.push_str(&emit_node(&[
        kv("name", yaml_str("🐟 漏网之鱼")),
        kv("type", "select"),
        kv("proxies", format!("[{}, {}]", yaml_str("🚀 节点选择"), yaml_str("DIRECT"))),
    ]));

    out.push_str("\nrules:\n");
    out.push_str("  - IP-CIDR,192.168.0.0/16,DIRECT,no-resolve\n");
    out.push_str("  - IP-CIDR,10.0.0.0/8,DIRECT,no-resolve\n");
    out.push_str("  - IP-CIDR,172.16.0.0/12,DIRECT,no-resolve\n");
    out.push_str("  - IP-CIDR,127.0.0.0/8,DIRECT,no-resolve\n");
    out.push_str("  - GEOIP,CN,DIRECT\n");
    out.push_str("  - MATCH,🐟 漏网之鱼\n");
    out
}

fn normalize_yaml_profile(header: &[(&'static str, String)], body: &str) -> Result<String, String> {
    let mut kept: Vec<&str> = Vec::new();
    let mut has_proxies = false;
    for line in body.lines() {
        let trimmed = line.trim();
        let at_root = !trimmed.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && trimmed.contains(':');
        if at_root {
            let key = trimmed.split(':').next().unwrap_or("").trim();
            if key == "proxies" {
                has_proxies = true;
            }
            if TOP_LEVEL_PORT_KEYS.contains(&key) {
                continue;
            }
        }
        kept.push(line);
    }
    if !has_proxies {
        return Err("配置缺少 proxies 段,不是有效的 Clash/mihomo YAML".into());
    }
    let mut out = String::new();
    write_header(&mut out, header);
    out.push('\n');
    for line in kept {
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

/// Converts any supported profile body into mihomo YAML. Mirrors
/// convertSubscription() of the Go bridge.
pub fn convert_subscription(config_path: &Path, body: &str) -> Result<String, String> {
    let header = header_map(config_path);
    let text = body.trim();
    if text.is_empty() {
        return Err("订阅内容为空".into());
    }

    // classic v2ray subscription: one base64 blob of node lines
    if scheme_of(text).is_empty() && !text.starts_with('{') {
        if let Ok(decoded) = b64_decode(text) {
            let candidate = String::from_utf8_lossy(&decoded).trim().to_string();
            if !scheme_of(&candidate).is_empty() {
                let (nodes, skipped) = node_lines_from(&candidate);
                if nodes.is_empty() {
                    return Err(format!("未解析到任何节点(跳过 {} 行)", skipped));
                }
                return Ok(build_node_config(&header, nodes));
            }
        }
    }

    if !scheme_of(text).is_empty() {
        let (nodes, skipped) = node_lines_from(text);
        if nodes.is_empty() {
            return Err(format!("未解析到任何节点(跳过 {} 行)", skipped));
        }
        return Ok(build_node_config(&header, nodes));
    }

    if text.starts_with('{') || looks_like_yaml(text) {
        return normalize_yaml_profile(&header, text);
    }

    if let Ok(decoded) = b64_decode(text) {
        let candidate = String::from_utf8_lossy(&decoded).trim().to_string();
        if !scheme_of(&candidate).is_empty() {
            let (nodes, _) = node_lines_from(&candidate);
            if !nodes.is_empty() {
                return Ok(build_node_config(&header, nodes));
            }
        }
        if !candidate.is_empty() && looks_like_yaml(&candidate) {
            return normalize_yaml_profile(&header, &candidate);
        }
    }
    Err("无法识别的订阅格式(既不是 Clash YAML,也不是节点列表)".into())
}

pub struct ProfileMeta {
    pub proxies: usize,
    pub groups: usize,
    pub rules: usize,
    pub used: i64,
    pub total: i64,
    pub expire: i64,
}

/// Downloads a subscription through curl and converts it to mihomo YAML.
pub async fn fetch_profile(config_path: &Path, url: &str) -> Result<(String, ProfileMeta), String> {
    let (headers, body) = curl_fetch(url, 45).await?;
    if body.is_empty() {
        return Err("下载到的配置为空".into());
    }
    let text = String::from_utf8_lossy(&body).into_owned();
    let yaml = convert_subscription(config_path, &text)?;

    let mut meta = ProfileMeta { proxies: 0, groups: 0, rules: 0, used: 0, total: 0, expire: 0 };
    if let Some(raw) = header_value(&headers, "subscription-userinfo") {
        for part in raw.split(';') {
            let part = part.trim();
            if let Some((k, v)) = part.split_once('=') {
                let v: i64 = v.trim().parse().unwrap_or(0);
                match k.trim().to_ascii_lowercase().as_str() {
                    "upload" => meta.used += v,
                    "download" => meta.used += v,
                    "total" => meta.total = v,
                    "expire" => meta.expire = v,
                    _ => {}
                }
            }
        }
    }
    meta.proxies = count_top_level_list(&yaml, "proxies");
    meta.groups = count_top_level_list(&yaml, "proxy-groups");
    meta.rules = count_top_level_list(&yaml, "rules");
    Ok((yaml, meta))
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Vec<(&'static str, String)> {
        vec![
            ("mixed-port", "7892".into()),
            ("socks-port", "7891".into()),
            ("port", "7892".into()),
            ("external-controller", "127.0.0.1:9097".into()),
            ("mode", "rule".into()),
            ("log-level", "info".into()),
            ("allow-lan", "false".into()),
            ("ipv6", "true".into()),
        ]
    }

    #[test]
    fn test_b64_variants() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(b64_decode("aGV s bG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
    }

    #[test]
    fn test_parse_sip002() {
        // method:password = aes-128-gcm:pass123 (base64)
        let cred = b64_decode("YWVzLTEyOC1nY206cGFzczEyMw==").unwrap();
        let cred = String::from_utf8(cred).unwrap();
        let mut userinfo = base64::engine::general_purpose::STANDARD.encode(&cred);
        userinfo = userinfo.replace('+', "-").replace('/', "_").trim_end_matches('=').to_string();
        let link = format!("ss://{}@1.2.3.4:8388#%E9%A6%99%E6%B8%AF%2001", userinfo);
        let pairs = parse_ss(&link).unwrap();
        assert_eq!(pairs[0].v, "\"香港 01\"");
        assert!(pairs.iter().any(|p| p.k == "cipher" && p.v == "\"aes-128-gcm\""));
        assert!(pairs.iter().any(|p| p.k == "port" && p.v == "8388"));
    }

    #[test]
    fn test_parse_legacy_ss() {
        // legacy: whole thing base64 of method:pass@host:port
        let inner = "aes-256-gcm:secret@5.6.7.8:443";
        let encoded = base64::engine::general_purpose::STANDARD.encode(inner);
        let pairs = parse_ss(&format!("ss://{}#node", encoded)).unwrap();
        assert!(pairs.iter().any(|p| p.k == "server" && p.v == "\"5.6.7.8\""));
        assert!(pairs.iter().any(|p| p.k == "password" && p.v == "\"secret\""));
    }

    #[test]
    fn test_parse_trojan() {
        let link = "trojan://pw123@example.com:443?sni=demo.example.com&allowInsecure=1#T1";
        let pairs = parse_share_url(link, "trojan").unwrap();
        assert!(pairs.iter().any(|p| p.k == "sni" && p.v == "\"demo.example.com\""));
        assert!(pairs.iter().any(|p| p.k == "skip-cert-verify" && p.v == "true"));
    }

    #[test]
    fn test_parse_hysteria2() {
        let link = "hysteria2://auth@1.1.1.1:3612/?sni=h2.example.com&obfs=salamander&obfs-password=ob1#H2";
        let pairs = parse_share_url(link, "hysteria2").unwrap();
        assert!(pairs.iter().any(|p| p.k == "type" && p.v == "hysteria2"));
        assert!(pairs.iter().any(|p| p.k == "obfs" && p.v == "\"salamander\""));
    }

    #[test]
    fn test_parse_hysteria_v1_query_auth() {
        // Hysteria v1 puts auth in the query, not in userinfo: no `@`.
        let link = "hysteria://1.2.3.4:12854?protocol=udp&auth=peko&peer=wechat.com&insecure=1&upmbps=50&downmbps=250&alpn=h3#Hys";
        let pairs = parse_hysteria(link).unwrap();
        assert!(pairs.iter().any(|p| p.k == "type" && p.v == "hysteria"));
        assert!(pairs.iter().any(|p| p.k == "auth-str" && p.v == "\"peko\""));
        assert!(pairs.iter().any(|p| p.k == "sni" && p.v == "\"wechat.com\""));
        assert!(pairs.iter().any(|p| p.k == "skip-cert-verify" && p.v == "true"));
        assert!(pairs.iter().any(|p| p.k == "up" && p.v == "\"50 Mbps\""));
        assert!(pairs.iter().any(|p| p.k == "down" && p.v == "\"250 Mbps\""));
        assert!(pairs.iter().any(|p| p.k == "protocol" && p.v == "\"udp\""));
        assert!(pairs.iter().any(|p| p.k == "alpn" && p.v == "[\"h3\"]"));
    }

    #[test]
    fn test_parse_hysteria_v1_userinfo_and_obfs() {
        let link = "hysteria://pw@example.com:443?obfsParam=obfs-pw&insecure=0&sni=s.example.com#H2";
        let pairs = parse_hysteria(link).unwrap();
        assert!(pairs.iter().any(|p| p.k == "auth-str" && p.v == "\"pw\""));
        assert!(pairs.iter().any(|p| p.k == "obfs" && p.v == "\"obfs-pw\""));
        assert!(pairs.iter().any(|p| p.k == "sni" && p.v == "\"s.example.com\""));
        assert!(!pairs.iter().any(|p| p.k == "skip-cert-verify"));
    }

    #[test]
    fn test_parse_anytls() {
        let link = "anytls://s3cret@1.1.1.1:8443?insecure=1&sni=a.example.com&fp=chrome&alpn=h2%2Chttp%2F1.1&pinSHA256=AA%3ABB#A1";
        let pairs = parse_share_url(link, "anytls").unwrap();
        assert!(pairs.iter().any(|p| p.k == "type" && p.v == "anytls"));
        assert!(pairs.iter().any(|p| p.k == "password" && p.v == "\"s3cret\""));
        assert!(pairs.iter().any(|p| p.k == "sni" && p.v == "\"a.example.com\""));
        assert!(pairs.iter().any(|p| p.k == "skip-cert-verify" && p.v == "true"));
        assert!(pairs.iter().any(|p| p.k == "client-fingerprint" && p.v == "\"chrome\""));
        assert!(pairs.iter().any(|p| p.k == "alpn" && p.v == "[\"h2\", \"http/1.1\"]"));
        assert!(pairs.iter().any(|p| p.k == "fingerprint" && p.v == "\"AA:BB\""));
    }

    #[test]
    fn test_parse_anytls_without_userinfo() {
        let link = "anytls://1.1.1.1:8443?password=query-pw&sni=a.example.com#A2";
        let pairs = parse_share_url(link, "anytls").unwrap();
        assert!(pairs.iter().any(|p| p.k == "password" && p.v == "\"query-pw\""));
        assert!(pairs.iter().any(|p| p.k == "sni" && p.v == "\"a.example.com\""));
    }

    #[test]
    fn test_scheme_of_hysteria_and_anytls() {
        assert_eq!(scheme_of("hysteria://1.2.3.4:443#x"), "hysteria");
        assert_eq!(scheme_of("hysteria2://a@1.2.3.4:443#x"), "hysteria2");
        assert_eq!(scheme_of("anytls://pw@1.2.3.4:443#x"), "anytls");
    }

    #[test]
    fn test_convert_base64_nodelist() {
        let nodes = "trojan://pw1@2.3.4.5:443?sni=a.com#N1\nss://YWVzLTEyOC1nY206cHc=@6.7.8.9:1234#N2";
        let encoded = base64::engine::general_purpose::STANDARD.encode(nodes);
        let dir = std::env::temp_dir();
        let yaml = convert_subscription(&dir.join("nonexistent.yaml"), &encoded).unwrap();
        assert!(yaml.contains("proxies:"));
        assert!(yaml.contains("proxy-groups:"));
        assert!(yaml.contains("rules:"));
        assert!(yaml.contains("\"N1\""));
        assert!(yaml.contains("mixed-port: 7892"));
        assert_eq!(count_top_level_list(&yaml, "proxies"), 2);
    }

    #[test]
    fn test_convert_hysteria_and_anytls_nodelist() {
        let nodes = "hysteria://1.2.3.4:12854?auth=peko&peer=wechat.com&upmbps=50&downmbps=250#Hys\nanytls://pw@5.6.7.8:8443?sni=a.example.com&insecure=1#Any";
        let encoded = base64::engine::general_purpose::STANDARD.encode(nodes);
        let dir = std::env::temp_dir();
        let yaml = convert_subscription(&dir.join("nonexistent.yaml"), &encoded).unwrap();
        assert_eq!(count_top_level_list(&yaml, "proxies"), 2);
        assert!(yaml.contains("type: hysteria"));
        assert!(yaml.contains("type: anytls"));
        assert!(yaml.contains("\"Hys\""));
        assert!(yaml.contains("\"Any\""));
    }

    #[test]
    fn test_convert_yaml_passthrough() {
        let body = "port: 7890\nproxies:\n  - name: a\n    type: ss\nrules:\n  - MATCH,DIRECT\n";
        let dir = std::env::temp_dir();
        let yaml = convert_subscription(&dir.join("nonexistent.yaml"), body).unwrap();
        assert!(yaml.starts_with("mixed-port: 7892"));
        assert!(!yaml.contains("port: 7890"));
        assert!(yaml.contains("proxies:"));
        assert!(yaml.contains("MATCH,DIRECT"));
    }

    #[test]
    fn test_rejects_garbage() {
        let dir = std::env::temp_dir();
        assert!(convert_subscription(&dir.join("no.yaml"), "just some text 123").is_err());
    }
}
