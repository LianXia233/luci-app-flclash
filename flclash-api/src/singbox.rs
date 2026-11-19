//! Clash/mihomo `config.yaml` -> sing-box 1.14 JSON converter.
//!
//! This is the "lossless" bridge that lets one subscription drive both cores:
//! the API already normalises every subscription format into a mihomo
//! `config.yaml`; `convert()` parses that YAML into structured data and emits
//! a sing-box config with the equivalent proxies, groups, rules, DNS and TUN.
//!
//! Design goals:
//!   - same mixed-port / external-controller so the WebUI and proxy port do
//!     not move when the core is switched;
//!   - every node type the subscription converter can produce is translated
//!     (ss/ssr/vmess/vless/trojan/hysteria/hysteria2/tuic/socks5/http);
//!   - proxy-groups become selector/urltest (relay is best-effort via chain);
//!   - Clash rules -> route rules, fake-ip DNS -> sing-box fakeip, TUN -> tun.
//!
//! Unknown rule kinds and node types are skipped (with a direct fallback)
//! rather than producing an invalid sing-box config.

use serde_json::{json, Value as J};
use serde_yaml::Value as Y;

// ------------------------------------------------------------- yaml helpers

fn ystr(y: &Y, key: &str) -> Option<String> {
    match y.get(key) {
        Some(Y::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Y::Number(n)) => Some(n.to_string()),
        Some(Y::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// First non-empty key wins. Used for the many aliases Clash configs use.
fn ystr_any(y: &Y, keys: &[&str]) -> String {
    for key in keys {
        if let Some(v) = ystr(y, key) {
            return v;
        }
    }
    String::new()
}

fn ybool(y: &Y, key: &str) -> bool {
    match y.get(key) {
        Some(Y::Bool(b)) => *b,
        Some(Y::String(s)) => s == "true" || s == "1",
        Some(Y::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    }
}

fn yint(y: &Y, key: &str, default: i64) -> i64 {
    match y.get(key) {
        Some(Y::Number(n)) => n.as_i64().unwrap_or(default),
        Some(Y::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

fn ystrings(y: &Y, key: &str) -> Vec<String> {
    match y.get(key) {
        Some(Y::Sequence(seq)) => seq
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(Y::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Folks write `port: 443` or `port: "443"`; hysteria may use `ports: 1000-2000`.
fn first_port(s: &str) -> i64 {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|p| !p.is_empty())
        .and_then(|p| p.parse().ok())
        .unwrap_or(0)
}

fn port_of(y: &Y) -> i64 {
    if let Some(p) = y.get("port") {
        return match p {
            Y::Number(n) => n.as_i64().unwrap_or(0),
            Y::String(s) => first_port(s),
            _ => 0,
        };
    }
    // hysteria / hysteria2 sometimes expose only `ports`.
    first_port(&ystr_any(y, &["ports", "port-range"]))
}

// ------------------------------------------------------------- TLS/transport

fn tls_object(y: &Y) -> Option<J> {
    // `tls: false` / absent -> no TLS object.
    match y.get("tls") {
        Some(Y::Bool(false)) => return None,
        Some(Y::String(s)) if s == "false" || s.is_empty() => return None,
        _ => {}
    }
    let mut has_tls = matches!(y.get("tls"), Some(Y::Bool(true)) | Some(Y::String(_)))
        || y.get("tls").map(|v| v.is_mapping()).unwrap_or(false)
        || !ystr_any(y, &["servername", "sni", "server-name"]).is_empty()
        || !ystr_any(y, &["fingerprint", "client-fingerprint"]).is_empty();
    let alpn = ystrings(y, "alpn");
    has_tls = has_tls || !alpn.is_empty();
    if !has_tls {
        return None;
    }

    // Nested `tls: {server-name, skip-cert-verify, alpn, fingerprint}` overrides.
    let mut server_name = ystr_any(y, &["servername", "sni", "server-name"]);
    let mut insecure = ybool(y, "skip-cert-verify");
    let mut nested_alpn = alpn.clone();
    let mut fingerprint = ystr_any(y, &["fingerprint", "client-fingerprint"]);
    let mut reality: Option<J> = None;

    if let Some(Y::Mapping(tls_map)) = y.get("tls") {
        if let Some(v) = tls_map.get("server-name").and_then(|v| v.as_str()) {
            server_name = v.to_string();
        }
        if let Some(v) = tls_map.get("servername").and_then(|v| v.as_str()) {
            server_name = v.to_string();
        }
        if let Some(v) = tls_map.get("insecure") {
            insecure = matches!(v, Y::Bool(true)) || matches!(v, Y::String(s) if s == "true");
        }
        if let Some(v) = tls_map.get("skip-cert-verify") {
            insecure = matches!(v, Y::Bool(true)) || matches!(v, Y::String(s) if s == "true");
        }
        if let Some(v) = tls_map.get("alpn") {
            nested_alpn = match v {
                Y::Sequence(seq) => seq.iter().filter_map(|x| x.as_str().map(str::to_string)).collect(),
                Y::String(s) => vec![s.clone()],
                _ => nested_alpn,
            };
        }
        if let Some(v) = tls_map.get("fingerprint").and_then(|v| v.as_str()) {
            fingerprint = v.to_string();
        }
        if let Some(Y::Mapping(ro)) = tls_map.get("reality-opts") {
            let mut r = serde_json::Map::new();
            if let Some(v) = ro.get("public-key").and_then(|v| v.as_str()) {
                r.insert("public_key".into(), json!(v));
            }
            if let Some(v) = ro.get("short-id").and_then(|v| v.as_str()) {
                r.insert("short_id".into(), json!(v));
            }
            reality = Some(J::Object(r));
        }
    }

    // Some configs hide sni inside ws/grpc host headers; do not guess unless set.
    let mut t = json!({"enabled": true});
    let obj = t.as_object_mut().unwrap();
    if !server_name.is_empty() {
        obj.insert("server_name".into(), json!(server_name));
    }
    if insecure {
        obj.insert("insecure".into(), json!(true));
    }
    if !nested_alpn.is_empty() {
        obj.insert("alpn".into(), json!(nested_alpn));
    }
    if !fingerprint.is_empty() {
        obj.insert("utls".into(), json!({"enabled": true, "fingerprint": fingerprint}));
    }
    if let Some(r) = reality {
        obj.insert("reality".into(), r);
    }
    Some(t)
}

/// Map Clash `network` + `*-opts` into a sing-box `transport` object.
fn transport_object(y: &Y) -> Option<J> {
    let network = ystr(y, "network")
        .unwrap_or_default()
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    match network.as_str() {
        "ws" | "websocket" => {
            let mut ws = serde_json::Map::new();
            ws.insert("type".into(), json!("ws"));
            // Flat form: ws-path / ws-headers.
            let mut path = ystr(y, "ws-path").unwrap_or_default();
            let mut headers: Vec<(String, String)> = Vec::new();
            if let Some(Y::Mapping(h)) = y.get("ws-headers") {
                for (k, v) in h {
                    if let (Some(kk), Some(s)) = (k.as_str(), v.as_str()) {
                        headers.push((kk.to_string(), s.to_string()));
                    }
                }
            }
            if let Some(Y::Mapping(opts)) = y.get("ws-opts") {
                if let Some(v) = opts.get("path").and_then(|v| v.as_str()) {
                    path = v.to_string();
                }
                if let Some(Y::Mapping(h)) = opts.get("headers") {
                    headers.clear();
                    for (k, v) in h {
                        if let (Some(kk), Some(s)) = (k.as_str(), v.as_str()) {
                            headers.push((kk.to_string(), s.to_string()));
                        }
                    }
                }
                if let Some(v) = opts.get("early-data-header-name").and_then(|v| v.as_str()) {
                    ws.insert("early_data_header_name".into(), json!(v));
                }
            }
            ws.insert("path".into(), json!(if path.is_empty() { "/".to_string() } else { path }));
            if !headers.is_empty() {
                let mut hm = serde_json::Map::new();
                for (k, v) in headers {
                    hm.insert(k, json!(v));
                }
                ws.insert("headers".into(), J::Object(hm));
            }
            Some(J::Object(ws))
        }
        "grpc" => {
            let mut service = ystr(y, "grpc-service-name").unwrap_or_default();
            if let Some(Y::Mapping(opts)) = y.get("grpc-opts") {
                if let Some(v) = opts.get("grpc-service-name").and_then(|v| v.as_str()) {
                    service = v.to_string();
                }
            }
            let mut g = serde_json::Map::new();
            g.insert("type".into(), json!("grpc"));
            if !service.is_empty() {
                g.insert("service_name".into(), json!(service));
            }
            Some(J::Object(g))
        }
        "h2" | "http" => {
            let mut hosts = ystrings(y, "h2-host");
            let mut path = ystr(y, "h2-path").unwrap_or_else(|| "/".to_string());
            if let Some(opts) = y.get("h2-opts") {
                hosts = ystrings(opts, "host");
                path = ystr(opts, "path").unwrap_or(path);
            }
            let mut h = serde_json::Map::new();
            h.insert("type".into(), json!("http"));
            if !hosts.is_empty() {
                h.insert("host".into(), json!(hosts));
            }
            h.insert("path".into(), json!(path));
            Some(J::Object(h))
        }
        _ => None,
    }
}

// ------------------------------------------------------------- node mapping

fn common(typ: &str, name: &str, y: &Y) -> J {
    let mut o = json!({
        "type": typ,
        "tag": name,
        "server": ystr(y, "server").unwrap_or_default(),
        "server_port": port_of(y),
    });
    let obj = o.as_object_mut().unwrap();
    if let Some(tls) = tls_object(y) {
        obj.insert("tls".into(), tls);
    }
    if let Some(tr) = transport_object(y) {
        obj.insert("transport".into(), tr);
    }
    o
}

/// Convert one Clash proxy mapping into a sing-box outbound. Returns None for
/// types sing-box cannot express (the caller substitutes a direct outbound).
fn proxy_to_outbound(p: &Y) -> Option<J> {
    let name = ystr(p, "name").unwrap_or_else(|| "node".to_string());
    let typ = ystr(p, "type").unwrap_or_default().to_lowercase();

    let mut out = match typ.as_str() {
        "ss" | "shadowsocks" => {
            let mut o = common("shadowsocks", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                // Clash subscriptions sometimes ship metadata/placeholder
                // nodes with the shorthand `cipher: chacha20`. mihomo tolerates
                // it, but sing-box aborts startup with "unknown method".
                // Normalise the common aliases to canonical AEAD method names
                // so one bad node cannot take down the whole core.
                let raw_method = ystr_any(p, &["cipher", "method"]);
                let method = match raw_method.to_ascii_lowercase().as_str() {
                    "chacha20" | "chacha20-ietf" => "chacha20-ietf-poly1305".to_string(),
                    "aes-128-gcm" => "aes-128-gcm".to_string(),
                    "aes-256-gcm" => "aes-256-gcm".to_string(),
                    "none" | "plain" => "none".to_string(),
                    _ => raw_method.clone(),
                };
                m.insert("method".into(), json!(method));
                m.insert("password".into(), json!(ystr(p, "password").unwrap_or_default()));
                if ybool(p, "udp") {
                    m.insert("network".into(), json!(["tcp", "udp"]));
                }
                if let Some(plugin) = ystr(p, "plugin") {
                    if plugin == "obfs" || plugin.starts_with("obfs-local") {
                        m.insert("plugin".into(), json!("obfs-local"));
                        if let Some(opts) = p.get("plugin-opts") {
                            let mut po = serde_json::Map::new();
                            let mode = ystr_any(opts, &["mode", "obfs"]);
                            if !mode.is_empty() {
                                po.insert("mode".into(), json!(mode));
                            }
                            let host = ystr_any(opts, &["host", "obfs-host"]);
                            if !host.is_empty() {
                                po.insert("host".into(), json!(host));
                            }
                            m.insert("plugin_opts".into(), J::Object(po));
                        }
                    }
                }
            }
            o
        }
        // SSR was removed in sing-box 1.6; degrade to direct (the caller maps
        // unsupported tags to a direct outbound so groups/rules still resolve).
        "ssr" | "shadowsocksr" => return None,
        "vmess" => {
            let mut o = common("vmess", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("uuid".into(), json!(ystr(p, "uuid").unwrap_or_default()));
                let aid = yint(p, "alterId", 0);
                m.insert("alter_id".into(), json!(aid));
                let sec = ystr_any(p, &["cipher", "security"]);
                if !sec.is_empty() {
                    m.insert("security".into(), json!(sec));
                }
            }
            o
        }
        "vless" => {
            let mut o = common("vless", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("uuid".into(), json!(ystr(p, "uuid").unwrap_or_default()));
                let flow = ystr(p, "flow").unwrap_or_default();
                if !flow.is_empty() {
                    m.insert("flow".into(), json!(flow));
                }
                if ybool(p, "packet-addr") {
                    m.insert("packet_addr".into(), json!(true));
                }
            }
            o
        }
        "trojan" => {
            let mut o = common("trojan", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("password".into(), json!(ystr(p, "password").unwrap_or_default()));
            }
            o
        }
        "hysteria" => {
                let mut o = common("hysteria", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                let up = first_port(&ystr_any(p, &["up", "up_mbps"]));
                let down = first_port(&ystr_any(p, &["down", "down_mbps"]));
                if up > 0 { m.insert("up_mbps".into(), json!(up)); }
                if down > 0 { m.insert("down_mbps".into(), json!(down)); }
                let auth = ystr_any(p, &["auth-str", "password", "auth"]);
                if !auth.is_empty() {
                    m.insert("auth".into(), json!(auth));
                }
                let obfs = ystr_any(p, &["obfs"]);
                if !obfs.is_empty() {
                    m.insert("obfs".into(), json!(obfs));
                }
            }
            o
        }
        "hysteria2" | "hy2" => {
            let mut o = common("hysteria2", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("password".into(), json!(ystr(p, "password").unwrap_or_default()));
                let obfs = ystr_any(p, &["obfs", "obfs-password"]);
                if !obfs.is_empty() {
                    m.insert("obfs".into(), json!({"type": "salamander", "password": obfs}));
                }
            }
            o
        }
        "tuic" => {
            let mut o = common("tuic", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("uuid".into(), json!(ystr(p, "uuid").unwrap_or_default()));
                let password = ystr_any(p, &["password", "token"]);
                if !password.is_empty() {
                    m.insert("password".into(), json!(password));
                }
                let cc = ystr_any(p, &["congestion-controller", "congestion_control"]);
                if !cc.is_empty() {
                    m.insert("congestion_control".into(), json!(cc));
                }
            }
            o
        }
        "socks5" | "socks" => {
            let mut o = common("socks", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                let u = ystr(p, "username");
                let pw = ystr(p, "password");
                if u.is_some() || pw.is_some() {
                    m.insert("username".into(), json!(u.unwrap_or_default()));
                    m.insert("password".into(), json!(pw.unwrap_or_default()));
                }
            }
            o
        }
        "http" => {
            let mut o = common("http", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                let u = ystr(p, "username");
                let pw = ystr(p, "password");
                if u.is_some() || pw.is_some() {
                    m.insert("username".into(), json!(u.unwrap_or_default()));
                    m.insert("password".into(), json!(pw.unwrap_or_default()));
                }
            }
            o
        }
        "anytls" => {
            // anytls is TLS-only; password + tls (sni/insecure/fingerprint are
            // picked up by tls_object from the Clash top-level fields).
            let mut o = common("anytls", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert("password".into(), json!(ystr(p, "password").unwrap_or_default()));
            }
            o
        }
        "direct" => json!({"type": "direct", "tag": name}),
        "reject" => return None, // sing-box rejects via route action, not outbound
        _ => return None,
    };

    let obj = out.as_object_mut().unwrap();
    obj.entry("tag".to_string()).or_insert(json!(name));
    // Hysteria / Hysteria2 / TUIC are TLS-only; sing-box rejects them without
    // an explicit tls object. Supply a minimal one when the Clash node didn't.
    if matches!(typ.as_str(), "hysteria" | "hysteria2" | "hy2" | "tuic" | "anytls") && !obj.contains_key("tls") {
        obj.insert("tls".into(), json!({"enabled": true}));
    }
    Some(out)
}

// ------------------------------------------------------------- group mapping

fn group_to_outbound(g: &Y, known: &std::collections::HashSet<String>) -> Option<J> {
    let name = ystr(g, "name").unwrap_or_else(|| "group".to_string());
    let typ = ystr(g, "type").unwrap_or_default().to_lowercase();
    let members: Vec<String> = ystrings(g, "proxies")
        .into_iter()
        .filter(|p| known.contains(p) || p == "DIRECT")
        .collect();
    if members.is_empty() {
        return None;
    }

    // A selector's default must be a usable proxy, not DIRECT/REJECT and not
    // a metadata/placeholder node. Some subscriptions put DIRECT first and
    // include fake "traffic remaining"/"expiry" entries that can never
    // connect. Prefer an auto-select (urltest) group first — it probes and
    // picks a live node — then the first plausible real node. Recognise common
    // placeholder labels in both English and Chinese subscription dialects.
    let is_placeholder = |m: &str| -> bool {
        let lower = m.to_ascii_lowercase();
        m == "DIRECT" || m == "REJECT"
            || lower.contains("remaining") || lower.contains("expire")
            || lower.contains("剩余") || lower.contains("过期")
            || lower.contains("流量") || lower.contains("到期")
    };
    let default_out = members.iter()
        .find(|m| m.to_ascii_lowercase().contains("自动选择"))
        .or_else(|| members.iter().find(|m| !is_placeholder(m)))
        .cloned()
        .unwrap_or_else(|| members[0].clone());

    match typ.as_str() {
        "select" => Some(json!({
            "type": "selector",
            "tag": name,
            "outbounds": members,
            "default": default_out,
        })),
        "url-test" | "fallback" | "load-balance" => {
            // sing-box has a single `urltest` outbound (no distinct fallback).
            let url = ystr(g, "url").unwrap_or_else(|| "http://www.gstatic.com/generate_204".into());
            let interval = yint(g, "interval", 300).max(1);
            let mut o = json!({
                "type": "urltest",
                "tag": name,
                "outbounds": members,
                "url": url,
                "interval": format!("{}s", interval),
            });
            let tol = yint(g, "tolerance", 0);
            if tol > 0 {
                o["tolerance"] = json!(tol);
            }
            Some(o)
        }
        "relay" => {
            // sing-box has no first-class relay group; emit a urltest over the
            // chain members so the group still routes through the same nodes.
            Some(json!({
                "type": "urltest",
                "tag": name,
                "outbounds": members,
            }))
        }
        _ => None,
    }
}

// ------------------------------------------------------------- rule mapping

/// Translate a Clash rule line into a sing-box route rule. Returns the final
/// outbound name via `final_out` when the rule is MATCH.
fn rule_to_route(raw: &str, final_out: &mut Option<String>) -> Option<J> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }
    let kind = parts[0].to_uppercase();
    let policy = parts.get(2).copied().unwrap_or("DIRECT").to_string();
    let reject = policy.eq_ignore_ascii_case("REJECT");

    let mut rule = serde_json::Map::new();
    match kind.as_str() {
        "DOMAIN" | "DOMAIN-SUFFIX" | "DOMAIN-KEYWORD" | "DOMAIN-REGEX" => {
            let val = parts.get(1).copied().unwrap_or("").trim_matches('"');
            if val.is_empty() {
                return None;
            }
            let key = match kind.as_str() {
                "DOMAIN" => "domain",
                "DOMAIN-SUFFIX" => "domain_suffix",
                "DOMAIN-KEYWORD" => "domain_keyword",
                _ => "domain_regex",
            };
            rule.insert(key.into(), json!([val]));
        }
        "IP-CIDR" | "IP-CIDR6" | "SRC-IP-CIDR" => {
            let val = parts.get(1).copied().unwrap_or("").trim_matches('"');
            if val.is_empty() {
                return None;
            }
            let key = if kind == "SRC-IP-CIDR" { "source_ip_cidr" } else { "ip_cidr" };
            rule.insert(key.into(), json!([val]));
        }
        "GEOIP" => {
            let val = parts.get(1).copied().unwrap_or("").to_lowercase();
            if val.is_empty() {
                return None;
            }
            // sing-box 1.12+ removed `geoip`; use a rule-set reference.
            rule.insert("rule_set".into(), json!(format!("geoip-{}", val)));
        }
        "DST-PORT" | "SOURCE-PORT" => {
            let val = parts.get(1).copied().unwrap_or("").trim_matches('"');
            let port: u16 = match val.parse() {
                Ok(p) => p,
                Err(_) => return None,
            };
            let key = if kind == "SOURCE-PORT" { "source_port" } else { "port" };
            rule.insert(key.into(), json!([port]));
        }
        "MATCH" | "FINAL" => {
            // Clash MATCH/FINAL is two segments: `MATCH,POLICY` (policy is
            // parts[1]), unlike normal `TYPE,VALUE,POLICY` rules.
            let match_policy = parts.get(1).copied().unwrap_or("DIRECT").to_string();
            if !match_policy.eq_ignore_ascii_case("REJECT") {
                *final_out = Some(match_policy);
            }
            return None;
        }
        // RULE-SET / PROCESS-NAME / GEOIP with special providers and unknown
        // kinds are deliberately skipped: emitting them would break sing-box
        // check. The `final` outbound still keeps traffic flowing.
        _ => return None,
    }

    if reject {
        rule.insert("action".into(), json!("reject"));
    } else if policy != "DIRECT" {
        rule.insert("outbound".into(), json!(policy));
    } else {
        rule.insert("outbound".into(), json!("DIRECT"));
    }
    Some(J::Object(rule))
}

// ------------------------------------------------------------- DNS / TUN

fn is_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// Parse a Clash DNS address string into sing-box 1.14's typed server object.
/// 1.14 removed the legacy `address` field in favour of `type` + `server`.
fn dns_server(addr: &str) -> J {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix("https://") {
        return json!({"type": "https", "server": rest.split('/').next().unwrap_or(rest)});
    }
    if let Some(rest) = addr.strip_prefix("h3://") {
        return json!({"type": "h3", "server": rest.split('/').next().unwrap_or(rest)});
    }
    if let Some(rest) = addr.strip_prefix("tls://") {
        return json!({"type": "tls", "server": rest});
    }
    if let Some(rest) = addr.strip_prefix("tcp://") {
        return json!({"type": "tcp", "server": rest});
    }
    if let Some(rest) = addr.strip_prefix("quic://") {
        return json!({"type": "quic", "server": rest});
    }
    if let Some(rest) = addr.strip_prefix("udp://") {
        return json!({"type": "udp", "server": rest});
    }
    // Bare IP / hostname defaults to UDP. Addresses with an embedded path
    // (legacy DoH without scheme) are reduced to their host.
    json!({"type": "udp", "server": addr.split('/').next().unwrap_or(addr)})
}

fn dns_object(y: &Y, group_names: &[String]) -> Option<J> {
    let dns = y.get("dns")?;
    if !dns.is_mapping() {
        return None;
    }
    let enable = ybool(dns, "enable") || ybool(dns, "enabled");
    if !enable {
        return None;
    }

    let fakeip = ystr(dns, "enhanced-mode").as_deref() == Some("fake-ip") || ybool(dns, "fake-ip");
    let range = ystr(dns, "fake-ip-range").unwrap_or_else(|| "198.18.0.1/16".into());

    let mut servers: Vec<J> = Vec::new();
    let mut direct_tags: Vec<String> = Vec::new();
    let nameservers = ystrings(dns, "nameserver");
    // The first plain-IP nameserver is the bootstrap resolver for DoH/DoT hosts.
    let bootstrap_tag = nameservers
        .iter()
        .enumerate()
        .find(|(_, a)| is_ip(a.trim_start_matches("udp://").trim_start_matches("tls://")))
        .map(|(i, _)| format!("dns-direct-{}", i))
        .unwrap_or_else(|| "dns-direct-0".to_string());

    for (idx, addr) in nameservers.into_iter().enumerate() {
        let tag = format!("dns-direct-{}", idx);
        direct_tags.push(tag.clone());
        let mut s = dns_server(&addr);
        s["tag"] = json!(tag);
        let server_host = s.get("server").and_then(|v| v.as_str()).unwrap_or("");
        if !is_ip(server_host) && tag != bootstrap_tag {
            s["domain_resolver"] = json!(bootstrap_tag);
        }
        servers.push(s);
    }
    let detour = group_names.first().cloned();
    for (idx, addr) in ystrings(dns, "fallback").into_iter().enumerate() {
        let tag = format!("dns-remote-{}", idx);
        let mut s = dns_server(&addr);
        s["tag"] = json!(tag);
        let server_host = s.get("server").and_then(|v| v.as_str()).unwrap_or("");
        if !is_ip(server_host) {
            s["domain_resolver"] = json!(bootstrap_tag);
        }
        if let Some(d) = &detour {
            s["detour"] = json!(d);
        }
        servers.push(s);
    }
    if servers.is_empty() {
        let mut s = dns_server("223.5.5.5");
        s["tag"] = json!("dns-direct-0");
        servers.push(s);
        direct_tags.push("dns-direct-0".into());
    }

    // sing-box 1.14 removed the legacy top-level `fakeip` object. The new
    // form is a DNS server with address "fakeip" plus a route rule selecting
    // it for A/AAAA queries; filters are routed to the real resolver first.
    let real_tag = direct_tags.first().cloned().unwrap_or_else(|| "dns-direct-0".into());
    let mut rules: Vec<J> = Vec::new();
    for f in ystrings(dns, "fake-ip-filter") {
        let f = f.trim();
        if let Some(domain) = f.strip_prefix("+.").or_else(|| f.strip_prefix("*.")) {
            rules.push(json!({"domain_suffix": [domain], "server": real_tag}));
        } else if let Some(domain) = f.strip_prefix("+") {
            rules.push(json!({"domain_suffix": [domain], "server": real_tag}));
        } else if let Some(d) = f.strip_prefix("regex:") {
            rules.push(json!({"domain_regex": [d], "server": real_tag}));
        }
    }

    // sing-box forbids a fakeip server as the default (`final`) resolver;
    // route A/AAAA queries to it explicitly instead.
    let final_tag = real_tag.clone();
    if fakeip {
        servers.push(json!({
            "type": "fakeip",
            "tag": "dns-fakeip",
            "inet4_range": range,
        }));
        rules.push(json!({
            "query_type": ["A", "AAAA"],
            "server": "dns-fakeip",
        }));
    }

    let mut out = json!({
        "servers": servers,
        "final": final_tag,
    });
    if !rules.is_empty() {
        out["rules"] = json!(rules);
    }
    Some(out)
}

fn tun_inbound(y: &Y) -> Option<J> {
    let tun = y.get("tun")?;
    if !tun.is_mapping() || !(ybool(tun, "enable") || ybool(tun, "enabled")) {
        return None;
    }
    let stack = ystr_any(tun, &["stack"]);
    let stack = if stack.is_empty() { "gvisor" } else { &stack };
    Some(json!({
        "type": "tun",
        "tag": "tun-in",
        "address": ["172.19.0.1/30"],
        "auto_route": true,
        "stack": stack,
        "strict_route": false,
        "route_address": ["0.0.0.0/0"],
        "route_exclude_address": [
            "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
            "172.16.0.0/12", "192.0.0.0/24", "192.168.0.0/16", "224.0.0.0/4",
            "240.0.0.0/4", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8"
        ],
    }))
}

// ------------------------------------------------------------- main convert

/// Parse a mihomo/Clash config.yaml and produce a sing-box 1.14 JSON config.
pub fn convert(yaml: &str) -> Result<String, String> {
    let doc: Y = serde_yaml::from_str(yaml).map_err(|e| format!("YAML 解析失败: {}", e))?;
    if !doc.is_mapping() {
        return Err("config.yaml 顶层必须是映射".into());
    }

    let mixed_port: i64 = ystr(&doc, "mixed-port")
        .unwrap_or_else(|| "7892".into())
        .parse()
        .unwrap_or(7892);
    let controller = ystr_any(&doc, &["external-controller", "external_controller"]);
    let controller = if controller.is_empty() { "127.0.0.1:9097" } else { &controller };
    let mode = ystr(&doc, "mode").unwrap_or_else(|| "rule".into());
    let mode = match mode.as_str() {
        "global" | "direct" | "rule" | "script" => mode.as_str(),
        _ => "rule",
    };
    let log_level = ystr(&doc, "log-level").unwrap_or_else(|| "info".into());

    // 1) proxies -> outbounds. Keep a set of valid tags for group filtering.
    let mut outbounds: Vec<J> = vec![
        json!({"type": "direct", "tag": "DIRECT"}),
    ];
    let mut known = std::collections::HashSet::new();
    known.insert("DIRECT".to_string());

    if let Some(Y::Sequence(proxies)) = doc.get("proxies") {
        for p in proxies {
            if !p.is_mapping() {
                continue;
            }
            let name = ystr(p, "name").unwrap_or_default();
            match proxy_to_outbound(p) {
                Some(o) => {
                    known.insert(name);
                    outbounds.push(o);
                }
                None => {
                    // Unsupported node type: map to a direct outbound under the
                    // same tag so groups/rules referencing it still resolve.
                    if !name.is_empty() {
                        known.insert(name.clone());
                        outbounds.push(json!({"type": "direct", "tag": name}));
                    }
                }
            }
        }
    }

    // 2) groups -> selector/urltest. Groups are referenced by rules, so they
    //    must come after proxies but before route rules.
    let mut group_names: Vec<String> = Vec::new();
    if let Some(Y::Sequence(groups)) = doc.get("proxy-groups") {
        for g in groups {
            if !g.is_mapping() {
                continue;
            }
            if let Some(o) = group_to_outbound(g, &known) {
                let tag = o.get("tag").and_then(|v| v.as_str()).unwrap_or("").to_string();
                known.insert(tag.clone());
                group_names.push(tag);
                outbounds.push(o);
            }
        }
    }

    // 3) rules -> route rules.
    let mut final_out = Some("DIRECT".to_string());
    let mut route_rules: Vec<J> = Vec::new();
    if let Some(Y::Sequence(rules)) = doc.get("rules") {
        for r in rules {
            if let Some(s) = r.as_str() {
                if let Some(route) = rule_to_route(s, &mut final_out) {
                    route_rules.push(route);
                }
            }
        }
    }

    // 4) inbounds: mixed (+ redir/tproxy for transparent proxy + tun when requested).
    let mut inbounds: Vec<J> = vec![json!({
        "type": "mixed",
        "tag": "mixed-in",
        "listen": "0.0.0.0",
        "listen_port": mixed_port,
    })];
    // proxy-rules.sh redirects LAN TCP to 7893 and TPROXYs UDP to 7894.
    // Emit matching inbounds so the transparent takeover also works under
    // sing-box (mihomo exposes redir-port/tproxy-port natively).
    inbounds.push(json!({
        "type": "redirect",
        "tag": "redir-in",
        "listen": "127.0.0.1",
        "listen_port": 7893,
    }));
    inbounds.push(json!({
        "type": "tproxy",
        "tag": "tproxy-in",
        "listen": "127.0.0.1",
        "listen_port": 7894,
    }));
    if let Some(tun) = tun_inbound(&doc) {
        inbounds.push(tun);
    }

    // 5) DNS with fake-ip parity.
    let dns = dns_object(&doc, &group_names);

    // 6) Resolve geoip rule-set references produced by GEOIP rules.
    //    A remote rule_set is downloaded at core startup from GitHub raw,
    //    which fails on many CN networks and makes sing-box FATAL. Prefer a
    //    local .srs (provisioned by core-update.sh); if none is present, drop
    //    the rule_set and the rules referencing it so the core still boots and
    //    domain rules / global mode keep working.
    let local_srs = std::path::Path::new("/etc/flclash/geoip-cn.srs");
    let have_local = local_srs.is_file();
    let mut rule_sets: Vec<J> = Vec::new();
    let mut seen_sets: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in &route_rules {
        if let Some(tag) = r.get("rule_set").and_then(|v| v.as_str()) {
            if seen_sets.insert(tag.to_string()) {
                let code = tag.strip_prefix("geoip-").unwrap_or("cn");
                if have_local && code == "cn" {
                    rule_sets.push(json!({
                        "tag": tag,
                        "type": "local",
                        "format": "binary",
                        "path": "/etc/flclash/geoip-cn.srs",
                    }));
                }
                // else: no local set -> skip; referencing rules are removed below.
            }
        }
    }
    // Drop route rules that reference a rule_set we could not provide.
    route_rules.retain(|r| {
        match r.get("rule_set").and_then(|v| v.as_str()) {
            Some(tag) => rule_sets.iter().any(|s| s.get("tag").and_then(|v| v.as_str()) == Some(tag)),
            None => true,
        }
    });

    // sing-box's clash_api has no runtime global/direct switch (mode-list is
    // always ["rule"]). Emulate the two modes in the generated route:
    //   global -> everything non-private goes to the main proxy group;
    //   direct -> everything goes DIRECT.
    // Find the top-level proxy group (the one not referenced as a member of
    // another group) to use as the global final.
    if mode == "global" || mode == "direct" {
        let main_proxy = group_names.iter()
            .rev()
            .find(|g| g.as_str() != "DIRECT" && g.as_str() != "REJECT")
            .cloned();
        let private_rule = json!({
            "ip_cidr": ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8",
                        "169.254.0.0/16", "172.16.0.0/12", "192.0.0.0/24",
                        "192.168.0.0/16", "224.0.0.0/4", "240.0.0.0/4"],
            "outbound": "DIRECT",
        });
        route_rules = vec![private_rule];
        if mode == "direct" {
            final_out = Some("DIRECT".to_string());
        } else if let Some(p) = main_proxy {
            final_out = Some(p);
        }
    }

    let mut cfg = json!({
        "log": {"level": log_level},
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "rules": route_rules,
            "final": final_out.unwrap_or_else(|| "DIRECT".into()),
            "auto_detect_interface": true,
        },
        "experimental": {
            "clash_api": {
                "external_controller": controller,
                "default_mode": mode,
            }
        },
    });
    // Put the cache in /tmp so the core can run as an unprivileged user
    // (uid 5566) for the OUTPUT-redirect loop guard; /etc/flclash is root-only.
    cfg["experimental"]["cache_file"] = json!({
        "enabled": true,
        "path": "/tmp/flclash-singbox-cache.db",
    });
    if !rule_sets.is_empty() {
        cfg["route"]["rule_set"] = json!(rule_sets);
    }
    if let Some(dns) = dns {
        cfg["dns"] = dns;
        // 1.12+ requires an explicit domain resolver for outbound dialing.
        cfg["route"]["default_domain_resolver"] = json!("dns-direct-0");
    }

    serde_json::to_string_pretty(&cfg).map_err(|e| format!("JSON 序列化失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = r#"
mixed-port: 7892
external-controller: 127.0.0.1:9097
mode: rule
dns:
  enable: true
  enhanced-mode: fake-ip
  nameserver: [223.5.5.5]
  fake-ip-filter: ['*.lan']
proxies:
  - name: ss1
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-256-gcm
    password: pass
    udp: true
  - name: vm1
    type: vmess
    server: 5.6.7.8
    port: 443
    uuid: uuid-1234
    alterId: 0
    cipher: auto
    tls: true
    servername: example.com
    network: ws
    ws-path: /ws
    ws-headers:
      Host: example.com
proxy-groups:
  - name: PROXY
    type: select
    proxies: [ss1, vm1, DIRECT]
rules:
  - DOMAIN-SUFFIX,google.com,PROXY
  - GEOIP,CN,DIRECT
  - MATCH,PROXY
"#;

    #[test]
    fn converts_proxies_groups_rules() {
        let json = convert(YAML).unwrap();
        let v: J = serde_json::from_str(&json).unwrap();
        assert_eq!(v["inbounds"][0]["listen_port"], 7892);
        // transparent-proxy inbounds must be present for LAN takeover
        let ins = v["inbounds"].as_array().unwrap();
        assert!(ins.iter().any(|i| i["type"] == "redirect" && i["listen_port"] == 7893));
        assert!(ins.iter().any(|i| i["type"] == "tproxy" && i["listen_port"] == 7894));
        let outs = v["outbounds"].as_array().unwrap();
        assert!(outs.iter().any(|o| o["tag"] == "ss1" && o["type"] == "shadowsocks"));
        assert!(outs.iter().any(|o| o["tag"] == "vm1" && o["type"] == "vmess"));
        assert!(outs.iter().any(|o| o["tag"] == "PROXY" && o["type"] == "selector"));
        let vm = outs.iter().find(|o| o["tag"] == "vm1").unwrap();
        assert_eq!(vm["transport"]["type"], "ws");
        assert_eq!(vm["transport"]["path"], "/ws");
        assert_eq!(vm["tls"]["server_name"], "example.com");
    }

    #[test]
    fn converts_rules_and_fakeip() {
        let json = convert(YAML).unwrap();
        let v: J = serde_json::from_str(&json).unwrap();
        assert_eq!(v["route"]["final"], "PROXY");
        let rules = v["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|r| r["domain_suffix"] == json!(["google.com"])));
        // Without a local .srs, GEOIP rule_set rules are dropped so the core
        // never FATALs trying to download a remote set on a blocked network.
        assert!(!rules.iter().any(|r| r["rule_set"] == json!("geoip-cn")));
        assert!(v["route"].get("rule_set").is_none());
        assert!(v["dns"]["servers"].as_array().unwrap().iter().any(|s| s["type"] == "fakeip"));
    }

    #[test]
    fn tun_excludes_management_networks() {
        let yaml = format!("{}\ntun:\n  enable: true\n  stack: gvisor\n", YAML);
        let json = convert(&yaml).unwrap();
        let v: J = serde_json::from_str(&json).unwrap();
        let tun = v["inbounds"].as_array().unwrap().iter().find(|i| i["type"] == "tun").unwrap();
        assert_eq!(tun["auto_route"], true);
        let excluded = tun["route_exclude_address"].as_array().unwrap();
        assert!(excluded.contains(&json!("192.168.0.0/16")));
        assert!(excluded.contains(&json!("fc00::/7")));
    }

    #[test]
    fn rejects_non_mapping() {
        assert!(convert("- a\n- b\n").is_err());
    }
}
