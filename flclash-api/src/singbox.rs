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
//!   - Clash rules -> route rules, fake-ip DNS -> sing-box fakeip, TUN -> tun;
//!   - the core also serves the LAN DNS listener that `proxy-rules.sh` hijacks
//!     to port 1053 (a `direct` inbound + `hijack-dns` route action), so DNS
//!     and fake-ip behave the same as under mihomo;
//!   - `route.default_mark` mirrors mihomo's `routing-mark: 333`, which lets
//!     the nft OUTPUT chain exempt the core's own traffic (local takeover).
//!
//! Unknown rule kinds and node types are skipped (with a direct fallback)
//! rather than producing an invalid sing-box config. Every outbound reference
//! is checked against the emitted tags before it reaches `route.final` or a
//! rule, because a single dangling tag makes `sing-box check` reject the whole
//! config and the core never starts.

use serde_json::{json, Value as J};
use serde_yaml::Value as Y;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Where geo rule-set binaries are provisioned (core-update.sh / geo-update.sh).
const GEO_DIR: &str = "/etc/flclash";
/// mihomo `routing-mark` / nft CORE_MARK (0x14d). Emitted as `route.default_mark`.
const ROUTE_MARK: u64 = 333;
const DNS_PORT_DEFAULT: u16 = 1053;
/// LAN DNS listener inside the core; `proxy-rules.sh` hijacks client :53 here.
const DNS_INBOUND_TAG: &str = "dns-in";

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
    ybool_opt(y, key).unwrap_or(false)
}

fn ybool_opt(y: &Y, key: &str) -> Option<bool> {
    match y.get(key) {
        Some(Y::Bool(b)) => Some(*b),
        Some(Y::String(s)) => match s.as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        Some(Y::Number(n)) => Some(n.as_i64().unwrap_or(0) != 0),
        _ => None,
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

/// Like `ystrings`, but also splits comma-separated values (`alpn: h2,http/1.1`).
fn ycsv(y: &Y, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in ystrings(y, key) {
        for part in item.split(',') {
            let part = part.trim();
            if !part.is_empty() {
                out.push(part.to_string());
            }
        }
    }
    out
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

/// "1000-2000,3000" -> ["1000:2000", "3000"] (sing-box PortRange syntax).
fn server_ports(y: &Y) -> Option<Vec<String>> {
    let raw = ystr_any(y, &["ports", "port-range"]);
    if raw.is_empty() {
        return None;
    }
    let list: Vec<String> = raw
        .split(',')
        .map(|item| item.trim().replace('-', ":"))
        .filter(|item| !item.is_empty() && item.chars().all(|c| c.is_ascii_digit() || c == ':'))
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

// ------------------------------------------------------------- geo resources

/// Presence of the local binary rule-sets. References are only emitted when
/// the file exists: sing-box treats a missing local rule-set as a fatal error,
/// while a skipped rule still leaves the rest of the route working.
struct GeoResources {
    dir: PathBuf,
    geoip_cn: bool,
    geosite_cn: bool,
}

impl GeoResources {
    fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            geoip_cn: dir.join("geoip-cn.srs").is_file(),
            geosite_cn: dir.join("geosite-cn.srs").is_file(),
        }
    }

    fn path(&self, tag: &str) -> Option<PathBuf> {
        match tag {
            "geoip-cn" if self.geoip_cn => Some(self.dir.join("geoip-cn.srs")),
            "geosite-cn" if self.geosite_cn => Some(self.dir.join("geosite-cn.srs")),
            _ => None,
        }
    }
}

// ------------------------------------------------------------- TLS/transport

fn reality_object(ro: &serde_yaml::Mapping) -> J {
    let mut out = serde_json::Map::new();
    if let Some(v) = ro.get("public-key").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            out.insert("public_key".into(), json!(v));
        }
    }
    if let Some(v) = ro.get("short-id").and_then(|v| v.as_str()) {
        if !v.is_empty() {
            out.insert("short_id".into(), json!(v));
        }
    }
    J::Object(out)
}

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
        || !ystr_any(y, &["fingerprint", "client-fingerprint"]).is_empty()
        || y.get("reality-opts")
            .map(|v| v.is_mapping())
            .unwrap_or(false);
    let alpn = ycsv(y, "alpn");
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
                Y::Sequence(seq) => seq
                    .iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect(),
                Y::String(s) => s
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect(),
                _ => nested_alpn,
            };
        }
        if let Some(v) = tls_map.get("fingerprint").and_then(|v| v.as_str()) {
            fingerprint = v.to_string();
        }
        if let Some(Y::Mapping(ro)) = tls_map.get("reality-opts") {
            reality = Some(reality_object(ro));
        }
    }
    // Some subscriptions put reality-opts at the node's top level.
    if reality.is_none() {
        if let Some(Y::Mapping(ro)) = y.get("reality-opts") {
            reality = Some(reality_object(ro));
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
        obj.insert(
            "utls".into(),
            json!({"enabled": true, "fingerprint": fingerprint}),
        );
    }
    if let Some(r) = reality {
        // An empty reality object still tells sing-box to use REALITY; only
        // attach it when a public key was actually present.
        if r.as_object().map(|m| !m.is_empty()).unwrap_or(false) {
            obj.insert("reality".into(), r);
        }
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
                if let Some(v) = opts.get("max-early-data").and_then(|v| v.as_i64()) {
                    ws.insert("max_early_data".into(), json!(v));
                }
            }
            ws.insert(
                "path".into(),
                json!(if path.is_empty() {
                    "/".to_string()
                } else {
                    path
                }),
            );
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

/// sing-box rejects `tls` / `transport` on outbound types that do not model
/// them, so only attach the objects the protocol actually supports.
fn supports_tls(typ: &str) -> bool {
    matches!(
        typ,
        "vmess" | "vless" | "trojan" | "hysteria" | "hysteria2" | "tuic" | "anytls" | "http"
    )
}

fn supports_transport(typ: &str) -> bool {
    matches!(typ, "vmess" | "vless" | "trojan")
}

fn common(typ: &str, name: &str, y: &Y) -> J {
    let mut o = json!({
        "type": typ,
        "tag": name,
        "server": ystr(y, "server").unwrap_or_default(),
        "server_port": port_of(y),
    });
    {
        let obj = o.as_object_mut().unwrap();
        if supports_tls(typ) {
            if let Some(tls) = tls_object(y) {
                obj.insert("tls".into(), tls);
            }
        }
        if supports_transport(typ) {
            if let Some(tr) = transport_object(y) {
                obj.insert("transport".into(), tr);
            }
        }
    }
    o
}

/// Clash SIP003 plugin options are a string in sing-box; the previous object
/// form failed `sing-box check` ("cannot unmarshal object into string").
fn apply_ss_plugin(m: &mut serde_json::Map<String, J>, p: &Y) {
    let Some(plugin) = ystr(p, "plugin") else {
        return;
    };
    let plugin_l = plugin.to_ascii_lowercase();
    let opts = p.get("plugin-opts");
    if plugin_l == "obfs" || plugin_l.starts_with("obfs-local") {
        m.insert("plugin".into(), json!("obfs-local"));
        let mut parts: Vec<String> = Vec::new();
        if let Some(opts) = opts {
            let mode = ystr_any(opts, &["mode", "obfs"]);
            if !mode.is_empty() {
                parts.push(format!("obfs={mode}"));
            }
            let host = ystr_any(opts, &["host", "obfs-host"]);
            if !host.is_empty() {
                parts.push(format!("obfs-host={host}"));
            }
        }
        if !parts.is_empty() {
            m.insert("plugin_opts".into(), json!(parts.join(";")));
        }
    } else if plugin_l.starts_with("v2ray-plugin") {
        m.insert("plugin".into(), json!("v2ray-plugin"));
        let mut parts: Vec<String> = Vec::new();
        if let Some(opts) = opts {
            let mode = ystr(opts, "mode").unwrap_or_default();
            if !mode.is_empty() {
                parts.push(format!("mode={mode}"));
            }
            if ybool(opts, "tls") {
                parts.push("tls".into());
            }
            let host = ystr_any(opts, &["host"]);
            if !host.is_empty() {
                parts.push(format!("host={host}"));
            }
            let path = ystr(opts, "path").unwrap_or_default();
            if !path.is_empty() {
                parts.push(format!("path={path}"));
            }
        }
        if !parts.is_empty() {
            m.insert("plugin_opts".into(), json!(parts.join(";")));
        }
    }
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
                let raw_method = ystr_any(p, &["cipher", "method"]);
                let method = match raw_method.to_ascii_lowercase().as_str() {
                    "chacha20" | "chacha20-ietf" => "chacha20-ietf-poly1305".to_string(),
                    _ => raw_method,
                };
                m.insert("method".into(), json!(method));
                m.insert(
                    "password".into(),
                    json!(ystr(p, "password").unwrap_or_default()),
                );
                // sing-box enables both transports by default; honour an
                // explicit `udp: false` so UDP is not attempted on a node that
                // does not carry it.
                if ybool_opt(p, "udp") == Some(false) {
                    m.insert("network".into(), json!(["tcp"]));
                }
                apply_ss_plugin(m, p);
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
                m.insert(
                    "password".into(),
                    json!(ystr(p, "password").unwrap_or_default()),
                );
            }
            o
        }
        "hysteria" => {
            let mut o = common("hysteria", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                // sing-box requires up/down; Clash defaults are 50/100 Mbps.
                let up = first_port(&ystr_any(p, &["up", "up_mbps"]));
                let down = first_port(&ystr_any(p, &["down", "down_mbps"]));
                m.insert("up_mbps".into(), json!(if up > 0 { up } else { 50 }));
                m.insert("down_mbps".into(), json!(if down > 0 { down } else { 100 }));
                let auth = ystr_any(p, &["auth-str", "password", "auth"]);
                if !auth.is_empty() {
                    // `auth` means base64 in sing-box; a Clash auth string is
                    // plain text, so it belongs in `auth_str`.
                    m.insert("auth_str".into(), json!(auth));
                }
                let obfs = ystr_any(p, &["obfs"]);
                if !obfs.is_empty() {
                    m.insert("obfs".into(), json!(obfs));
                }
                if let Some(ports) = server_ports(p) {
                    m.remove("server_port");
                    m.insert("server_ports".into(), json!(ports));
                }
                let hop = ystr_any(p, &["hop-interval", "hop_interval"]);
                if !hop.is_empty() {
                    m.insert("hop_interval".into(), json!(hop));
                }
            }
            o
        }
        "hysteria2" | "hy2" => {
            let mut o = common("hysteria2", &name, p);
            {
                let m = o.as_object_mut().unwrap();
                m.insert(
                    "password".into(),
                    json!(ystr(p, "password").unwrap_or_default()),
                );
                // Clash spells obfuscation as `obfs: salamander` +
                // `obfs-password: <pw>`; older links carry the password in
                // `obfs` itself.
                let obfs_type = ystr(p, "obfs").unwrap_or_default();
                let obfs_pw = ystr_any(p, &["obfs-password", "obfs_password"]);
                let password = if !obfs_pw.is_empty() {
                    obfs_pw
                } else if !obfs_type.is_empty() && obfs_type != "salamander" {
                    obfs_type
                } else {
                    String::new()
                };
                if !password.is_empty() {
                    m.insert(
                        "obfs".into(),
                        json!({"type": "salamander", "password": password}),
                    );
                }
                if let Some(ports) = server_ports(p) {
                    m.remove("server_port");
                    m.insert("server_ports".into(), json!(ports));
                }
                let hop = ystr_any(p, &["hop-interval", "hop_interval"]);
                if !hop.is_empty() {
                    m.insert("hop_interval".into(), json!(hop));
                }
                let up = first_port(&ystr_any(p, &["up", "up_mbps"]));
                let down = first_port(&ystr_any(p, &["down", "down_mbps"]));
                if up > 0 {
                    m.insert("up_mbps".into(), json!(up));
                }
                if down > 0 {
                    m.insert("down_mbps".into(), json!(down));
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
                let relay = ystr_any(p, &["udp-relay-mode", "udp_relay_mode"]);
                if matches!(relay.as_str(), "native" | "quic") {
                    m.insert("udp_relay_mode".into(), json!(relay));
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
                m.insert(
                    "password".into(),
                    json!(ystr(p, "password").unwrap_or_default()),
                );
            }
            o
        }
        "direct" => json!({"type": "direct", "tag": name}),
        "reject" => return None, // sing-box rejects via route action, not outbound
        _ => return None,
    };

    let obj = out.as_object_mut().unwrap();
    obj.entry("tag".to_string()).or_insert(json!(name));
    // Hysteria / Hysteria2 / TUIC / anytls are TLS-only; sing-box rejects them
    // without an explicit tls object. Supply a minimal one when Clash didn't.
    if matches!(
        typ.as_str(),
        "hysteria" | "hysteria2" | "hy2" | "tuic" | "anytls"
    ) && !obj.contains_key("tls")
    {
        obj.insert("tls".into(), json!({"enabled": true}));
    }
    Some(out)
}

// ------------------------------------------------------------- group mapping

struct GroupDef {
    name: String,
    kind: String,
    members: Vec<String>,
    url: String,
    interval: i64,
    tolerance: i64,
}

/// Raw group graph, used to break reference cycles before sing-box sees them.
fn group_reaches(
    graph: &HashMap<String, Vec<String>>,
    from: &str,
    target: &str,
    seen: &mut HashSet<String>,
) -> bool {
    if from == target {
        return true;
    }
    if !seen.insert(from.to_string()) {
        return false;
    }
    graph
        .get(from)
        .map(|members| {
            members
                .iter()
                .any(|member| group_reaches(graph, member, target, seen))
        })
        .unwrap_or(false)
}

fn group_to_outbound(def: &GroupDef, members: Vec<String>) -> Option<J> {
    if members.is_empty() {
        return None;
    }
    // A selector's default must be a usable proxy, not DIRECT/REJECT and not
    // a metadata/placeholder node. Some subscriptions put DIRECT first and
    // include fake "traffic remaining"/"expiry" entries that can never
    // connect. Prefer an auto-select (urltest) group first — it probes and
    // picks a live node — then the first plausible real node.
    let is_placeholder = |m: &str| -> bool {
        let lower = m.to_ascii_lowercase();
        m == "DIRECT"
            || m == "REJECT"
            || lower.contains("remaining")
            || lower.contains("expire")
            || lower.contains("剩余")
            || lower.contains("过期")
            || lower.contains("流量")
            || lower.contains("到期")
    };
    let default_out = members
        .iter()
        .find(|m| m.to_ascii_lowercase().contains("自动选择"))
        .or_else(|| members.iter().find(|m| !is_placeholder(m)))
        .cloned()
        .unwrap_or_else(|| members[0].clone());

    match def.kind.as_str() {
        "select" => Some(json!({
            "type": "selector",
            "tag": def.name,
            "outbounds": members,
            "default": default_out,
        })),
        "url-test" | "fallback" | "load-balance" => {
            // sing-box has a single `urltest` outbound (no distinct fallback).
            let url = if def.url.is_empty() {
                "http://www.gstatic.com/generate_204".to_string()
            } else {
                def.url.clone()
            };
            let mut o = json!({
                "type": "urltest",
                "tag": def.name,
                "outbounds": members,
                "url": url,
                "interval": format!("{}s", def.interval.max(1)),
            });
            if def.tolerance > 0 {
                o["tolerance"] = json!(def.tolerance);
            }
            Some(o)
        }
        "relay" => {
            // sing-box has no first-class relay group; emit a urltest over the
            // chain members so the group still routes through the same nodes.
            Some(json!({
                "type": "urltest",
                "tag": def.name,
                "outbounds": members,
            }))
        }
        _ => None,
    }
}

// ------------------------------------------------------------- rule mapping

#[derive(Clone, PartialEq)]
enum Target {
    Reject,
    Direct,
    Tag(String),
    Skip,
}

/// Refuse to emit an outbound reference that does not exist: one dangling tag
/// makes sing-box reject the entire config.
fn policy_target(policy: &str, known: &HashSet<String>) -> Target {
    let policy = policy.trim().trim_matches('"');
    match policy.to_ascii_uppercase().as_str() {
        "REJECT" | "REJECT-DROP" => Target::Reject,
        "DIRECT" | "COMPATIBLE" | "GLOBAL" => Target::Direct,
        // PASS rules intentionally fall through; they have no outbound.
        "PASS" => Target::Skip,
        _ => {
            if known.contains(policy) {
                Target::Tag(policy.to_string())
            } else {
                Target::Direct
            }
        }
    }
}

fn apply_target(rule: &mut serde_json::Map<String, J>, target: &Target) -> bool {
    match target {
        Target::Reject => {
            rule.insert("action".into(), json!("reject"));
        }
        Target::Direct => {
            rule.insert("outbound".into(), json!("DIRECT"));
        }
        Target::Tag(tag) => {
            rule.insert("outbound".into(), json!(tag));
        }
        Target::Skip => return false,
    }
    true
}

/// Split a Clash port value ("443" or "1000-2000") into a sing-box matcher.
fn port_rule(value: &str) -> Option<(&'static str, J)> {
    let value = value.trim();
    if value.chars().all(|c| c.is_ascii_digit()) {
        let port: u16 = value.parse().ok()?;
        return Some(("port", json!([port])));
    }
    let (start, end) = value.split_once('-')?;
    if !start.chars().all(|c| c.is_ascii_digit()) || !end.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(("port_range", json!([format!("{start}:{end}")])))
}

/// Translate a Clash rule line into a sing-box route rule. Returns the final
/// outbound name via `final_out` when the rule is MATCH.
fn rule_to_route(
    raw: &str,
    known: &HashSet<String>,
    geo: &GeoResources,
    used_sets: &mut HashSet<String>,
    final_out: &mut Option<String>,
) -> Option<J> {
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.is_empty() || parts[0].is_empty() || parts[0].starts_with('#') {
        return None;
    }
    let kind = parts[0].to_uppercase();
    let value = parts.get(1).copied().unwrap_or("").trim_matches('"');
    let policy = parts.get(2).copied().unwrap_or("DIRECT");
    let target = policy_target(policy, known);

    let mut rule = serde_json::Map::new();
    match kind.as_str() {
        "DOMAIN" | "DOMAIN-SUFFIX" | "DOMAIN-KEYWORD" | "DOMAIN-REGEX" => {
            if value.is_empty() {
                return None;
            }
            let key = match kind.as_str() {
                "DOMAIN" => "domain",
                "DOMAIN-SUFFIX" => "domain_suffix",
                "DOMAIN-KEYWORD" => "domain_keyword",
                _ => "domain_regex",
            };
            rule.insert(key.into(), json!([value]));
        }
        "IP-CIDR" | "IP-CIDR6" | "SRC-IP-CIDR" => {
            if value.is_empty() {
                return None;
            }
            let key = if kind == "SRC-IP-CIDR" {
                "source_ip_cidr"
            } else {
                "ip_cidr"
            };
            rule.insert(key.into(), json!([value]));
        }
        "GEOIP" => {
            let code = value.to_lowercase();
            if code == "private" || code == "lan" {
                rule.insert("ip_is_private".into(), json!(true));
            } else if code == "cn" && geo.geoip_cn {
                used_sets.insert("geoip-cn".into());
                rule.insert("rule_set".into(), json!("geoip-cn"));
            } else {
                // No local rule-set: dropping the rule keeps the core bootable.
                return None;
            }
        }
        "GEOSITE" => {
            let code = value.to_lowercase();
            if code == "private" {
                rule.insert(
                    "domain_suffix".into(),
                    json!(["lan", "local", "arpa", "home.arpa"]),
                );
            } else if code == "cn" && geo.geosite_cn {
                used_sets.insert("geosite-cn".into());
                rule.insert("rule_set".into(), json!("geosite-cn"));
            } else {
                return None;
            }
        }
        "DST-PORT" | "SOURCE-PORT" | "SRC-PORT" => {
            let (key, value) = port_rule(value)?;
            let key = if kind == "DST-PORT" {
                key
            } else if key == "port" {
                "source_port"
            } else {
                "source_port_range"
            };
            rule.insert(key.into(), value);
        }
        "NETWORK" => {
            let networks: Vec<String> = value
                .split('/')
                .map(|n| n.trim().to_lowercase())
                .filter(|n| matches!(n.as_str(), "tcp" | "udp"))
                .collect();
            if networks.is_empty() {
                return None;
            }
            rule.insert("network".into(), json!(networks));
        }
        "PROCESS-NAME" => {
            if value.is_empty() {
                return None;
            }
            rule.insert("process_name".into(), json!([value]));
        }
        "PROCESS-PATH" => {
            if value.is_empty() {
                return None;
            }
            rule.insert("process_path".into(), json!([value]));
        }
        "MATCH" | "FINAL" => {
            // Clash MATCH/FINAL is two segments: `MATCH,POLICY` (policy is
            // parts[1]), unlike normal `TYPE,VALUE,POLICY` rules.
            match policy_target(value, known) {
                Target::Tag(tag) => *final_out = Some(tag),
                Target::Reject => return Some(json!({"action": "reject"})),
                Target::Direct | Target::Skip => *final_out = Some("DIRECT".into()),
            }
            return None;
        }
        // RULE-SET / IP-ASN / logical rules and unknown kinds are deliberately
        // skipped: emitting them would break `sing-box check`. The `final`
        // outbound still keeps traffic flowing.
        _ => return None,
    }

    if !apply_target(&mut rule, &target) {
        return None;
    }
    Some(J::Object(rule))
}

// ------------------------------------------------------------- DNS

fn is_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// Splits "host:port" (bracketed IPv6 aware) into (host, port). A bare IPv6
/// literal without brackets is returned untouched: its colon prefix can never
/// parse as a port, so the host-side colon test keeps it whole.
fn split_host_port_str(s: &str) -> (String, Option<u16>) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let host = rest[..end].to_string();
            let port = rest[end + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse::<u16>().ok());
            return (host, port);
        }
    }
    if let Some(i) = s.rfind(':') {
        if !s[..i].contains(':') {
            if let Ok(port) = s[i + 1..].parse::<u16>() {
                return (s[..i].to_string(), Some(port));
            }
        }
    }
    (s.to_string(), None)
}

/// Parse a Clash DNS address string into sing-box 1.12+'s typed server object.
/// 1.14 removed the legacy `address` field in favour of `type` + `server`
/// (+ `server_port`): an embedded ":port" used to be left inside `server`,
/// which sing-box reads as a literal hostname and the upstream never resolves.
fn parse_dns_addr(addr: &str) -> (&'static str, String, Option<u16>) {
    let addr = addr.trim();
    let (typ, hostpart) = if let Some(rest) = addr.strip_prefix("https://") {
        ("https", rest.split('/').next().unwrap_or(rest))
    } else if let Some(rest) = addr.strip_prefix("h3://") {
        ("h3", rest.split('/').next().unwrap_or(rest))
    } else if let Some(rest) = addr.strip_prefix("tls://") {
        ("tls", rest)
    } else if let Some(rest) = addr.strip_prefix("tcp://") {
        ("tcp", rest)
    } else if let Some(rest) = addr.strip_prefix("quic://") {
        ("quic", rest)
    } else if let Some(rest) = addr.strip_prefix("udp://") {
        ("udp", rest)
    } else {
        // Bare IP / hostname defaults to UDP. Addresses with an embedded path
        // (legacy DoH without scheme) are reduced to their host.
        ("udp", addr.split('/').next().unwrap_or(addr))
    };
    let (host, port) = split_host_port_str(hostpart);
    (typ, host, port)
}

fn dns_server(addr: &str) -> J {
    let (typ, host, port) = parse_dns_addr(addr);
    let mut s = json!({"type": typ, "server": host});
    if let Some(p) = port {
        s["server_port"] = json!(p);
    }
    s
}

fn dns_server_tagged(addr: &str, tag: &str, bootstrap: &str, detour: Option<&str>) -> J {
    let mut s = dns_server(addr);
    s["tag"] = json!(tag);
    let host = s
        .get("server")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !is_ip(&host) {
        s["domain_resolver"] = json!(bootstrap);
    }
    if let Some(detour) = detour {
        s["detour"] = json!(detour);
    }
    s
}

fn dns_enabled(y: &Y) -> bool {
    let Some(dns) = y.get("dns") else {
        return false;
    };
    if !dns.is_mapping() {
        return false;
    }
    // A subscription may omit `enable` entirely; the router-owned dns block
    // written by enforce_invariants does too, and mihomo treats the section as
    // enabled. Only an explicit false disables the module.
    ybool_opt(dns, "enable").unwrap_or(true) && ybool_opt(dns, "enabled").unwrap_or(true)
}

fn dns_listen(y: &Y, allow_lan: bool) -> (String, u16) {
    let raw = y
        .get("dns")
        .and_then(|dns| ystr(dns, "listen"))
        .unwrap_or_default();
    let (host, port) = split_host_port_str(&raw);
    let port = port.filter(|p| *p > 0).unwrap_or(DNS_PORT_DEFAULT);
    let host = if host.is_empty() || host == "0.0.0.0" || host == "::" || host == "[::]" {
        if allow_lan {
            "0.0.0.0".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    } else {
        host
    };
    (host, port)
}

struct DnsOut {
    object: J,
    resolver: String,
    strategy: String,
}

/// Build the sing-box DNS module from the Clash `dns:` block.
///
/// Split DNS parity:
///   - `nameserver` / `direct-nameserver` are direct upstreams;
///   - `fallback` servers dial through the main proxy group (`detour`);
///   - `nameserver-policy` domain keys become DNS rules, `geosite:cn`
///     resolves through `geosite-cn.srs` when that local rule-set exists;
///   - fake-ip gets an explicit A/AAAA rule (sing-box forbids fakeip as
///     `final`), fake-ip-filter entries are answered by the real resolver.
fn dns_object(
    y: &Y,
    group_names: &[String],
    geo: &GeoResources,
    used_sets: &mut HashSet<String>,
    ipv6: bool,
) -> Option<DnsOut> {
    let dns = y.get("dns")?;
    if !dns.is_mapping() || !dns_enabled(y) {
        return None;
    }

    let fakeip = ystr(dns, "enhanced-mode").as_deref() == Some("fake-ip") || ybool(dns, "fake-ip");
    let range = ystr(dns, "fake-ip-range").unwrap_or_else(|| "198.18.0.1/16".into());
    let dns_ipv6 = ybool_opt(dns, "ipv6").unwrap_or(true);
    let strategy = if ipv6 && dns_ipv6 {
        "prefer_ipv4"
    } else {
        "ipv4_only"
    };

    let mut servers: Vec<J> = Vec::new();
    let mut rules: Vec<J> = Vec::new();

    // 1) Bootstrap: plain-IP resolvers used for every hostname upstream. A
    //    hostname bootstrap pointing at itself can never resolve.
    let mut bootstrap_tag = String::new();
    for (idx, addr) in ystrings(dns, "default-nameserver").iter().enumerate() {
        let (_, host, _) = parse_dns_addr(addr);
        if !is_ip(&host) {
            continue;
        }
        let tag = format!("dns-bootstrap-{idx}");
        servers.push(dns_server_tagged(addr, &tag, "", None));
        if bootstrap_tag.is_empty() {
            bootstrap_tag = tag;
        }
    }
    if bootstrap_tag.is_empty() {
        let mut direct_ip = None;
        for key in ["proxy-server-nameserver", "nameserver", "direct-nameserver"] {
            for addr in ystrings(dns, key) {
                let (_, host, _) = parse_dns_addr(&addr);
                if is_ip(&host) {
                    direct_ip = Some(addr);
                    break;
                }
            }
            if direct_ip.is_some() {
                break;
            }
        }
        bootstrap_tag = "dns-bootstrap".to_string();
        match direct_ip {
            Some(addr) => servers.push(dns_server_tagged(&addr, &bootstrap_tag, "", None)),
            None => servers.push(json!({
                "type": "udp", "server": "223.5.5.5", "tag": bootstrap_tag,
            })),
        }
    }

    // 2) Domestic (directly resolved) upstreams.
    let mut domestic_tags: Vec<String> = Vec::new();
    for (idx, addr) in ystrings(dns, "nameserver").iter().enumerate() {
        let tag = format!("dns-domestic-{idx}");
        servers.push(dns_server_tagged(addr, &tag, &bootstrap_tag, None));
        domestic_tags.push(tag);
    }
    if domestic_tags.is_empty() {
        let tag = "dns-domestic-0".to_string();
        servers.push(json!({
            "type": "udp", "server": "223.5.5.5", "tag": tag,
        }));
        domestic_tags.push(tag);
    }
    let domestic = domestic_tags[0].clone();

    // 3) direct-nameserver: used for fake-ip-filter entries.
    let mut direct_tags: Vec<String> = Vec::new();
    for (idx, addr) in ystrings(dns, "direct-nameserver").iter().enumerate() {
        let tag = format!("dns-direct-{idx}");
        servers.push(dns_server_tagged(addr, &tag, &bootstrap_tag, None));
        direct_tags.push(tag);
    }
    let real_tag = direct_tags
        .first()
        .cloned()
        .unwrap_or_else(|| domestic.clone());

    // 4) Foreign upstreams: dial through the main proxy group.
    let detour = group_names.first().map(|s| s.as_str());
    let mut foreign_tags: Vec<String> = Vec::new();
    for (idx, addr) in ystrings(dns, "fallback").iter().enumerate() {
        let tag = format!("dns-foreign-{idx}");
        servers.push(dns_server_tagged(addr, &tag, &bootstrap_tag, detour));
        foreign_tags.push(tag);
    }

    // 5) nameserver-policy: custom domain keys become DNS rules. geosite keys
    //    that have no local rule-set are skipped (the final server covers them).
    let mut policy_cache: HashMap<String, String> = HashMap::new();
    let mut policy_seq = 0usize;
    let mut add_policy_server = |servers: &mut Vec<J>, addr: &str| -> Option<String> {
        if let Some(tag) = policy_cache.get(addr) {
            return Some(tag.clone());
        }
        let (_, host, _) = parse_dns_addr(addr);
        if host.is_empty() {
            return None;
        }
        let tag = format!("dns-policy-{policy_seq}");
        policy_seq += 1;
        servers.push(dns_server_tagged(addr, &tag, &bootstrap_tag, None));
        policy_cache.insert(addr.to_string(), tag.clone());
        Some(tag)
    };
    if let Some(policy) = dns.get("nameserver-policy").and_then(|v| v.as_mapping()) {
        for (key, value) in policy {
            let Some(key) = key.as_str() else {
                continue;
            };
            let addrs: Vec<String> = match value {
                Y::Sequence(seq) => seq
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                Y::String(s) => s
                    .split(',')
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect(),
                _ => Vec::new(),
            };
            if addrs.is_empty() {
                continue;
            }
            let lower = key.to_ascii_lowercase();
            if lower == "geosite:cn" || lower == "geosite:private" {
                if geo.geosite_cn && lower == "geosite:cn" {
                    used_sets.insert("geosite-cn".into());
                    rules.push(json!({"rule_set": "geosite-cn", "server": domestic}));
                }
                continue;
            }
            if lower.starts_with("geosite:") || lower.starts_with("rule-set:") {
                continue;
            }
            let Some(server) = addrs
                .iter()
                .find_map(|addr| add_policy_server(&mut servers, addr))
            else {
                continue;
            };
            let mut rule = serde_json::Map::new();
            if let Some(domain) = key.strip_prefix("+.").or_else(|| key.strip_prefix("*.")) {
                rule.insert("domain_suffix".into(), json!([domain]));
            } else if let Some(domain) = key.strip_prefix('+') {
                rule.insert("domain_suffix".into(), json!([domain]));
            } else if let Some(regex) = key.strip_prefix("regex:") {
                rule.insert("domain_regex".into(), json!([regex]));
            } else {
                rule.insert("domain".into(), json!([key]));
            }
            rule.insert("server".into(), json!(server));
            rules.push(J::Object(rule));
        }
    }

    // 6) fake-ip-filter: answer through the real resolver so LAN/router names
    //    and captive-portal probes keep working.
    for item in ystrings(dns, "fake-ip-filter") {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(code) = item.strip_prefix("geosite:") {
            if code.eq_ignore_ascii_case("cn") && geo.geosite_cn {
                used_sets.insert("geosite-cn".into());
                rules.push(json!({"rule_set": "geosite-cn", "server": real_tag}));
            }
            continue;
        }
        if let Some(regex) = item.strip_prefix("regex:") {
            rules.push(json!({"domain_regex": [regex], "server": real_tag}));
        } else if let Some(domain) = item.strip_prefix("+.").or_else(|| item.strip_prefix("*.")) {
            rules.push(json!({"domain_suffix": [domain], "server": real_tag}));
        } else if let Some(domain) = item.strip_prefix('+') {
            rules.push(json!({"domain_suffix": [domain], "server": real_tag}));
        } else {
            rules.push(json!({"domain": [item], "server": real_tag}));
        }
    }

    // 7) CN domains resolve domestically even under fake-ip; everything else
    //    gets a fake address and is routed by the subscription's domain rules.
    if geo.geosite_cn {
        used_sets.insert("geosite-cn".into());
        rules.push(json!({"rule_set": "geosite-cn", "server": domestic}));
    }

    if fakeip {
        let mut f = json!({
            "type": "fakeip",
            "tag": "dns-fakeip",
            "inet4_range": range,
        });
        if strategy != "ipv4_only" {
            f["inet6_range"] = json!("fc00::/18");
        }
        servers.push(f);
        // sing-box forbids a fakeip server as the default (`final`) resolver;
        // route A/AAAA queries to it explicitly instead.
        rules.push(json!({
            "query_type": ["A", "AAAA"],
            "server": "dns-fakeip",
        }));
    }

    // Under fake-ip every answer is synthesised locally, so the domestic
    // resolver is the right default. Under redir-host the real answer matters:
    // send unmatched names through the proxied foreign upstream when present.
    let final_tag = if fakeip {
        domestic.clone()
    } else if let Some(foreign) = foreign_tags.first() {
        foreign.clone()
    } else {
        domestic.clone()
    };

    Some(DnsOut {
        object: json!({
            "servers": servers,
            "rules": rules,
            "final": final_tag,
            "strategy": strategy,
            "reverse_mapping": true,
        }),
        resolver: bootstrap_tag,
        strategy: strategy.to_string(),
    })
}

fn tun_inbound(y: &Y, ipv6: bool) -> Option<J> {
    let tun = y.get("tun")?;
    if !tun.is_mapping() || !(ybool(tun, "enable") || ybool(tun, "enabled")) {
        return None;
    }
    let stack = ystr_any(tun, &["stack"]);
    let stack = match stack.as_str() {
        "gvisor" | "system" | "mixed" => stack,
        _ => "mixed".to_string(),
    };
    let mtu = yint(tun, "mtu", 1500).clamp(1280, 9000);
    let mut address = vec!["172.19.0.1/30"];
    let mut route_address = vec!["0.0.0.0/0"];
    if ipv6 {
        address.push("fdfe:dcba:9876::1/126");
        route_address.push("::/0");
    }
    let mut tun_obj = json!({
        "type": "tun",
        "tag": "tun-in",
        "address": address,
        "mtu": mtu,
        "auto_route": true,
        // Native nftables redirect for LAN traffic; `proxy-rules.sh` does not
        // install its own TUN rules, so this is what makes TUN mode work.
        "auto_redirect": true,
        "stack": stack,
        "strict_route": false,
        "route_address": route_address,
        "route_exclude_address": [
            "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
            "172.16.0.0/12", "192.0.0.0/24", "192.168.0.0/16", "224.0.0.0/4",
            "240.0.0.0/4", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8"
        ],
    });
    if let Some(name) = ystr(tun, "interface-name") {
        if !name.is_empty() {
            tun_obj["interface_name"] = json!(name);
        }
    }
    Some(tun_obj)
}

// ------------------------------------------------------------- main convert

/// mihomo log levels are not sing-box levels: `warning` is invalid in sing-box
/// and made every core start fail with "unknown log level: warning".
fn singbox_log_level(level: &str) -> &'static str {
    match level.to_ascii_lowercase().as_str() {
        "silent" => "error",
        "warning" | "warn" => "warn",
        "error" => "error",
        "debug" => "debug",
        "trace" => "trace",
        _ => "info",
    }
}

/// Pick the manual selector used as the global/direct-mode exit.
fn main_proxy_group(
    defs: &[GroupDef],
    emitted: &HashSet<String>,
    final_tag: &str,
) -> Option<String> {
    if emitted.contains(final_tag)
        && defs
            .iter()
            .any(|d| d.name == final_tag && d.kind == "select")
    {
        return Some(final_tag.to_string());
    }
    for def in defs {
        if !emitted.contains(&def.name) || def.kind != "select" {
            continue;
        }
        let lower = def.name.to_ascii_lowercase();
        if def.name.contains("节点选择")
            || def.name.contains("选择")
            || def.name.contains("代理")
            || lower.contains("proxy")
        {
            return Some(def.name.clone());
        }
    }
    defs.iter()
        .find(|d| emitted.contains(&d.name) && d.kind == "select")
        .map(|d| d.name.clone())
        .or_else(|| {
            defs.iter()
                .find(|d| emitted.contains(&d.name))
                .map(|d| d.name.clone())
        })
}

/// Parse a mihomo/Clash config.yaml and produce a sing-box 1.14 JSON config.
pub fn convert(yaml: &str) -> Result<String, String> {
    convert_with_geo(yaml, Path::new(GEO_DIR))
}

fn convert_with_geo(yaml: &str, geo_dir: &Path) -> Result<String, String> {
    let doc: Y = serde_yaml::from_str(yaml).map_err(|e| format!("YAML 解析失败: {}", e))?;
    if !doc.is_mapping() {
        return Err("config.yaml 顶层必须是映射".into());
    }
    let geo = GeoResources::new(geo_dir);

    let mixed_port: i64 = first_port(&ystr(&doc, "mixed-port").unwrap_or_else(|| "7892".into()));
    let mixed_port = if mixed_port > 0 { mixed_port } else { 7892 };
    let controller = ystr_any(&doc, &["external-controller", "external_controller"]);
    let controller = if controller.is_empty() {
        "127.0.0.1:9097".to_string()
    } else {
        controller
    };
    let mode = ystr(&doc, "mode").unwrap_or_else(|| "rule".into());
    let mode = match mode.as_str() {
        "global" | "direct" | "rule" => mode,
        _ => "rule".to_string(),
    };
    let log_level = ystr(&doc, "log-level").unwrap_or_else(|| "info".into());
    let allow_lan = ybool(&doc, "allow-lan");
    let ipv6 = ybool_opt(&doc, "ipv6").unwrap_or(false);

    // 1) proxies -> outbounds. Keep a set of valid tags for group filtering.
    let mut outbounds: Vec<J> = vec![json!({"type": "direct", "tag": "DIRECT"})];
    let mut proxy_tags: HashSet<String> = HashSet::new();
    proxy_tags.insert("DIRECT".to_string());

    if let Some(Y::Sequence(proxies)) = doc.get("proxies") {
        for p in proxies {
            if !p.is_mapping() {
                continue;
            }
            let name = ystr(p, "name").unwrap_or_default();
            match proxy_to_outbound(p) {
                Some(o) => {
                    if !name.is_empty() {
                        proxy_tags.insert(name.clone());
                    }
                    outbounds.push(o);
                }
                None => {
                    // Unsupported node type: map to a direct outbound under the
                    // same tag so groups/rules referencing it still resolve.
                    if !name.is_empty() {
                        proxy_tags.insert(name.clone());
                        outbounds.push(json!({"type": "direct", "tag": name}));
                    }
                }
            }
        }
    }

    // 2) groups -> selector/urltest. Two passes over the raw graph keep
    //    forward references (a group listing a group defined later) intact;
    //    back-edges are dropped so sing-box never sees a cyclic dependency.
    let mut defs: Vec<GroupDef> = Vec::new();
    if let Some(Y::Sequence(groups)) = doc.get("proxy-groups") {
        for g in groups {
            if !g.is_mapping() {
                continue;
            }
            let name = ystr(g, "name").unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            defs.push(GroupDef {
                name,
                kind: ystr(g, "type").unwrap_or_default().to_ascii_lowercase(),
                members: ystrings(g, "proxies"),
                url: ystr(g, "url").unwrap_or_default(),
                interval: yint(g, "interval", 300).max(1),
                tolerance: yint(g, "tolerance", 0),
            });
        }
    }
    let graph: HashMap<String, Vec<String>> = defs
        .iter()
        .map(|d| (d.name.clone(), d.members.clone()))
        .collect();
    let all_groups: HashSet<String> = defs.iter().map(|d| d.name.clone()).collect();
    let mut group_names: Vec<String> = Vec::new();
    let mut emitted_groups: HashSet<String> = HashSet::new();
    for def in &defs {
        let mut members: Vec<String> = Vec::new();
        for member in &def.members {
            if member == &def.name {
                continue;
            }
            let is_proxy = proxy_tags.contains(member);
            let is_group = all_groups.contains(member)
                && !group_reaches(&graph, member, &def.name, &mut HashSet::new());
            if (is_proxy || is_group) && !members.contains(member) {
                members.push(member.clone());
            }
        }
        if let Some(o) = group_to_outbound(def, members) {
            emitted_groups.insert(def.name.clone());
            group_names.push(def.name.clone());
            outbounds.push(o);
        }
    }

    let mut known = proxy_tags.clone();
    known.extend(emitted_groups.iter().cloned());

    // 3) rules -> route rules.
    let mut final_out: Option<String> = Some("DIRECT".to_string());
    let mut clash_rules: Vec<J> = Vec::new();
    let mut used_sets: HashSet<String> = HashSet::new();
    if let Some(Y::Sequence(rules)) = doc.get("rules") {
        for r in rules {
            if let Some(s) = r.as_str() {
                if let Some(route) = rule_to_route(s, &known, &geo, &mut used_sets, &mut final_out)
                {
                    clash_rules.push(route);
                }
            }
        }
    }

    // 4) DNS first: it decides whether a DNS inbound/hijack rule is needed.
    let detour_groups = {
        let mut list: Vec<String> = Vec::new();
        let final_tag = final_out.clone().unwrap_or_default();
        // A single-node subscription has no proxy-group; dialing the fallback
        // DNS through the selected node is still better than leaking it direct.
        if final_tag != "DIRECT" && known.contains(&final_tag) {
            list.push(final_tag);
        }
        for def in &defs {
            if def.kind == "select"
                && emitted_groups.contains(&def.name)
                && !list.contains(&def.name)
            {
                list.push(def.name.clone());
            }
        }
        for name in &group_names {
            if !list.contains(name) {
                list.push(name.clone());
            }
        }
        list
    };
    let dns = dns_object(&doc, &detour_groups, &geo, &mut used_sets, ipv6);
    let mut inbounds: Vec<J> = Vec::new();
    let mut pre_rules: Vec<J> = Vec::new();
    // When allow-lan is false the core must not expose the proxy/DNS listener
    // to the LAN; proxy-rules.sh forces allow-lan=true before a takeover
    // start, so REDIRECT/TPROXY traffic is unaffected.
    let mixed_listen = if allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    inbounds.push(json!({
        "type": "mixed",
        "tag": "mixed-in",
        "listen": mixed_listen,
        "listen_port": mixed_port,
    }));
    // proxy-rules.sh redirects LAN TCP to 7893 and TPROXYs UDP to 7894.
    // Both must listen on the wildcard: prerouting REDIRECT delivers to the
    // incoming interface's address (192.168.x.1) and TPROXY's transparent
    // socket lookup needs to match non-loopback original destinations.
    inbounds.push(json!({
        "type": "redirect",
        "tag": "redir-in",
        "listen": "0.0.0.0",
        "listen_port": 7893,
    }));
    inbounds.push(json!({
        "type": "tproxy",
        "tag": "tproxy-in",
        "listen": "0.0.0.0",
        "listen_port": 7894,
    }));
    if dns.is_some() {
        // `proxy-rules.sh` hijacks client :53 to this listener; the
        // `hijack-dns` action turns it into a normal sing-box DNS query.
        let (listen, port) = dns_listen(&doc, allow_lan);
        inbounds.push(json!({
            "type": "direct",
            "tag": DNS_INBOUND_TAG,
            "listen": listen,
            "listen_port": port,
        }));
        // TUN traffic that dials a DNS server directly (hard-coded :53) must
        // not leak outside the resolver either.
        if tun_inbound(&doc, ipv6).is_some() {
            pre_rules.push(json!({
                "inbound": [DNS_INBOUND_TAG, "tun-in"],
                "action": "hijack-dns",
            }));
        } else {
            pre_rules.push(json!({"inbound": DNS_INBOUND_TAG, "action": "hijack-dns"}));
        }
    }
    // Recover the real domain for redirected/TUN connections whose rules are
    // domain-based; fake-ip alone does not cover hard-coded IP traffic.
    pre_rules.push(json!({"action": "sniff"}));
    if let Some(tun) = tun_inbound(&doc, ipv6) {
        inbounds.push(tun);
    }

    // 5) Route: the mode is baked in because sing-box's clash_api has no
    //    runtime global/direct switch (mode-list is always ["rule"]).
    let mut final_tag = final_out.unwrap_or_else(|| "DIRECT".into());
    if !known.contains(&final_tag) {
        final_tag = "DIRECT".into();
    }
    let mut route_rules = pre_rules;
    if mode == "global" || mode == "direct" {
        route_rules.push(json!({
            "ip_cidr": ["0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8",
                        "169.254.0.0/16", "172.16.0.0/12", "192.0.0.0/24",
                        "192.168.0.0/16", "224.0.0.0/4", "240.0.0.0/4"],
            "outbound": "DIRECT",
        }));
        if mode == "direct" {
            final_tag = "DIRECT".into();
        } else if let Some(main) = main_proxy_group(&defs, &emitted_groups, &final_tag) {
            final_tag = main;
        }
    } else {
        route_rules.extend(clash_rules);
    }

    // 6) Resolve geoip/geosite references into local binary rule-sets. A
    //    missing file means the referencing rule was already skipped, so the
    //    core still boots on a device without downloaded geo data.
    let mut rule_sets: Vec<J> = Vec::new();
    for tag in ["geoip-cn", "geosite-cn"] {
        if !used_sets.contains(tag) {
            continue;
        }
        if let Some(path) = geo.path(tag) {
            rule_sets.push(json!({
                "tag": tag,
                "type": "local",
                "format": "binary",
                "path": path.to_string_lossy(),
            }));
        }
    }

    let mut cfg = json!({
        "log": {"level": singbox_log_level(&log_level)},
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": {
            "rules": route_rules,
            "final": final_tag,
            "auto_detect_interface": true,
            // Matches mihomo's routing-mark / nft CORE_MARK, so the OUTPUT
            // chain can exempt the core's own traffic (local takeover).
            "default_mark": ROUTE_MARK,
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
        cfg["route"]["default_domain_resolver"] =
            json!({"server": dns.resolver, "strategy": dns.strategy});
        cfg["dns"] = dns.object;
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
log-level: warning
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

    fn parse(json: &str) -> J {
        serde_json::from_str(json).unwrap()
    }

    fn temp_geo(tag_files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flclash-geo-{}-{}",
            std::process::id(),
            tag_files.join("-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for name in tag_files {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn converts_proxies_groups_rules() {
        let v = parse(&convert(YAML).unwrap());
        assert_eq!(v["inbounds"][0]["listen_port"], 7892);
        // transparent-proxy inbounds must be present for LAN takeover
        let ins = v["inbounds"].as_array().unwrap();
        assert!(ins
            .iter()
            .any(|i| i["type"] == "redirect" && i["listen_port"] == 7893));
        assert!(ins
            .iter()
            .any(|i| i["type"] == "tproxy" && i["listen_port"] == 7894));
        let outs = v["outbounds"].as_array().unwrap();
        assert!(outs
            .iter()
            .any(|o| o["tag"] == "ss1" && o["type"] == "shadowsocks"));
        assert!(outs
            .iter()
            .any(|o| o["tag"] == "vm1" && o["type"] == "vmess"));
        assert!(outs
            .iter()
            .any(|o| o["tag"] == "PROXY" && o["type"] == "selector"));
        let vm = outs.iter().find(|o| o["tag"] == "vm1").unwrap();
        assert_eq!(vm["transport"]["type"], "ws");
        assert_eq!(vm["transport"]["path"], "/ws");
        assert_eq!(vm["tls"]["server_name"], "example.com");
        // mihomo's `warning` is not a sing-box level; it must become `warn`.
        assert_eq!(v["log"]["level"], "warn");
    }

    #[test]
    fn dns_serves_lan_and_marks_default_route() {
        let v = parse(&convert(YAML).unwrap());
        let ins = v["inbounds"].as_array().unwrap();
        let dns_in = ins.iter().find(|i| i["tag"] == "dns-in").unwrap();
        assert_eq!(dns_in["type"], "direct");
        assert_eq!(dns_in["listen_port"], 1053);
        assert_eq!(
            dns_in["listen"], "127.0.0.1",
            "allow-lan=false keeps it local"
        );
        let rules = v["route"]["rules"].as_array().unwrap();
        assert!(rules
            .iter()
            .any(|r| r["inbound"] == "dns-in" && r["action"] == "hijack-dns"));
        assert!(rules.iter().any(|r| r["action"] == "sniff"));
        assert_eq!(v["route"]["default_mark"], 333);
    }

    #[test]
    fn allow_lan_binds_wildcard() {
        let yaml = YAML.replace("mixed-port: 7892", "mixed-port: 7892\nallow-lan: true");
        let v = parse(&convert(&yaml).unwrap());
        let ins = v["inbounds"].as_array().unwrap();
        let dns_in = ins.iter().find(|i| i["tag"] == "dns-in").unwrap();
        assert_eq!(dns_in["listen"], "0.0.0.0");
        let mixed = ins.iter().find(|i| i["tag"] == "mixed-in").unwrap();
        assert_eq!(mixed["listen"], "0.0.0.0");
    }

    #[test]
    fn converts_rules_and_fakeip() {
        // Use a private, empty geo dir so the presence of /etc/flclash on a
        // development host cannot change the expected rule set.
        let dir = temp_geo(&[]);
        let v = parse(&convert_with_geo(YAML, &dir).unwrap());
        assert_eq!(v["route"]["final"], "PROXY");
        let rules = v["route"]["rules"].as_array().unwrap();
        assert!(rules
            .iter()
            .any(|r| r["domain_suffix"] == json!(["google.com"])));
        // Without a local .srs, GEOIP rule_set rules are dropped so the core
        // never FATALs trying to download a remote set on a blocked network.
        assert!(!rules.iter().any(|r| r["rule_set"] == json!("geoip-cn")));
        assert!(v["route"].get("rule_set").is_none());
        let servers = v["dns"]["servers"].as_array().unwrap();
        assert!(servers.iter().any(|s| s["type"] == "fakeip"));
        // The fake-ip filter must use the real resolver, not the fakeip server.
        let dns_rules = v["dns"]["rules"].as_array().unwrap();
        assert!(dns_rules.iter().any(
            |r| r["domain_suffix"] == json!(["lan"]) && r["server"] == json!("dns-domestic-0")
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn geosite_and_geoip_use_local_rule_sets() {
        let dir = temp_geo(&["geoip-cn.srs", "geosite-cn.srs"]);
        let yaml = format!(
            "{}\nrules2: []\n",
            YAML.replace(
                "  - GEOIP,CN,DIRECT",
                "  - GEOSITE,cn,DIRECT\n  - GEOIP,CN,DIRECT\n  - GEOSITE,private,DIRECT"
            )
        );
        let v = parse(&convert_with_geo(&yaml, &dir).unwrap());
        let rules = v["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|r| r["rule_set"] == json!("geosite-cn")));
        assert!(rules.iter().any(|r| r["rule_set"] == json!("geoip-cn")));
        assert!(rules.iter().any(|r| r["domain_suffix"]
            .as_array()
            .map(|suffixes| suffixes.contains(&json!("lan")))
            .unwrap_or(false)));
        let sets = v["route"]["rule_set"].as_array().unwrap();
        assert!(sets.iter().any(|s| s["tag"] == "geosite-cn"));
        assert!(sets.iter().any(|s| s["tag"] == "geoip-cn"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_policy_falls_back_to_direct() {
        let yaml = YAML.replace(
            "  - DOMAIN-SUFFIX,google.com,PROXY",
            "  - DOMAIN-SUFFIX,google.com,GONE\n  - DOMAIN-SUFFIX,example.org,PASS",
        );
        let v = parse(&convert(&yaml).unwrap());
        let rules = v["route"]["rules"].as_array().unwrap();
        let gone = rules
            .iter()
            .find(|r| r["domain_suffix"] == json!(["google.com"]))
            .unwrap();
        assert_eq!(gone["outbound"], "DIRECT");
        assert!(!rules
            .iter()
            .any(|r| r["domain_suffix"] == json!(["example.org"])));
    }

    #[test]
    fn forward_group_references_and_cycles_are_resolved() {
        let yaml = r#"
mixed-port: 7892
proxies:
  - {name: a, type: ss, server: 1.1.1.1, port: 80, cipher: aes-128-gcm, password: x}
proxy-groups:
  - {name: G1, type: select, proxies: [G2, a]}
  - {name: G2, type: select, proxies: [a]}
  - {name: LOOP1, type: select, proxies: [LOOP2]}
  - {name: LOOP2, type: select, proxies: [LOOP1]}
rules:
  - MATCH,G1
"#;
        let v = parse(&convert(yaml).unwrap());
        let outs = v["outbounds"].as_array().unwrap();
        let g1 = outs.iter().find(|o| o["tag"] == "G1").unwrap();
        assert!(g1["outbounds"].as_array().unwrap().contains(&json!("G2")));
        assert!(!outs.iter().any(|o| o["tag"] == "LOOP1"));
        assert!(!outs.iter().any(|o| o["tag"] == "LOOP2"));
        assert_eq!(v["route"]["final"], "G1");
    }

    #[test]
    fn tun_excludes_management_networks() {
        let yaml = format!("{}\ntun:\n  enable: true\n  stack: gvisor\n", YAML);
        let v = parse(&convert(&yaml).unwrap());
        let tun = v["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "tun")
            .unwrap();
        assert_eq!(tun["auto_route"], true);
        assert_eq!(tun["auto_redirect"], true);
        assert_eq!(tun["stack"], "gvisor");
        let excluded = tun["route_exclude_address"].as_array().unwrap();
        assert!(excluded.contains(&json!("192.168.0.0/16")));
        assert!(excluded.contains(&json!("fc00::/7")));
        // DNS dialled through TUN must also be answered by the core resolver.
        let hijack = v["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["action"] == "hijack-dns")
            .unwrap();
        assert_eq!(hijack["inbound"], json!(["dns-in", "tun-in"]));
    }

    #[test]
    fn tun_adds_ipv6_only_when_enabled() {
        let yaml = format!("{}\nipv6: true\ntun:\n  enable: true\n", YAML);
        let v = parse(&convert(&yaml).unwrap());
        let tun = v["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "tun")
            .unwrap();
        assert!(tun["address"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a.as_str().unwrap().contains(':')));
        assert!(tun["route_address"]
            .as_array()
            .unwrap()
            .contains(&json!("::/0")));
    }

    #[test]
    fn rejects_non_mapping() {
        assert!(convert("- a\n- b\n").is_err());
    }

    #[test]
    fn dns_server_splits_embedded_ports() {
        let s = dns_server("https://dns.example.com:8443/dns-query");
        assert_eq!(s["server"], "dns.example.com");
        assert_eq!(s["server_port"], 8443);
        let s = dns_server("tls://8.8.8.8:853");
        assert_eq!(s["server"], "8.8.8.8");
        assert_eq!(s["server_port"], 853);
        let s = dns_server("223.5.5.5");
        assert_eq!(s["server"], "223.5.5.5");
        assert!(s.get("server_port").is_none());
        // Bare IPv6 without brackets must survive as the host.
        let s = dns_server("2001:db8::1");
        assert_eq!(s["server"], "2001:db8::1");
    }

    #[test]
    fn transparent_inbounds_listen_on_wildcard() {
        let v = parse(&convert(YAML).unwrap());
        let redir = v["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "redirect")
            .unwrap();
        assert_eq!(redir["listen"], "0.0.0.0");
        let tproxy = v["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "tproxy")
            .unwrap();
        assert_eq!(tproxy["listen"], "0.0.0.0");
    }

    #[test]
    fn all_hostname_upstreams_still_get_a_resolver() {
        // No plain-IP nameserver and no default-nameserver: the bootstrap
        // fallback must appear, and every hostname upstream must point at it.
        let yaml = "mixed-port: 7892\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n  nameserver:\n    - https://doh.example.com/dns-query\n    - https://alt.example.com:5353/dns-query\n";
        let v = parse(&convert(yaml).unwrap());
        let servers = v["dns"]["servers"].as_array().unwrap();
        let bootstrap = servers
            .iter()
            .find(|s| s["tag"] == "dns-bootstrap")
            .unwrap();
        assert_eq!(bootstrap["server"], "223.5.5.5");
        for s in servers
            .iter()
            .filter(|s| s["tag"] != "dns-bootstrap" && s.get("server").is_some())
        {
            assert_eq!(s["domain_resolver"], "dns-bootstrap");
        }
        assert_eq!(
            v["route"]["default_domain_resolver"]["server"],
            "dns-bootstrap"
        );
        // A port embedded in the DoH URL lands in server_port, not in server.
        let alt = servers
            .iter()
            .find(|s| s["tag"] == "dns-domestic-1")
            .unwrap();
        assert_eq!(alt["server"], "alt.example.com");
        assert_eq!(alt["server_port"], 5353);
    }

    #[test]
    fn ss_plugin_opts_are_sip003_strings() {
        let yaml = r#"
mixed-port: 7892
proxies:
  - name: obfs
    type: ss
    server: 1.2.3.4
    port: 8388
    cipher: aes-128-gcm
    password: x
    plugin: obfs
    plugin-opts:
      mode: tls
      host: bing.com
rules:
  - MATCH,DIRECT
"#;
        let v = parse(&convert(yaml).unwrap());
        let node = v["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["tag"] == "obfs")
            .unwrap();
        assert_eq!(node["plugin"], "obfs-local");
        assert_eq!(node["plugin_opts"], "obfs=tls;obfs-host=bing.com");
    }

    #[test]
    fn hysteria_uses_auth_str_and_required_rates() {
        let yaml = r#"
mixed-port: 7892
proxies:
  - name: hy
    type: hysteria
    server: 1.2.3.4
    port: 443
    auth-str: secret
    up: 20
    down: 50
rules:
  - MATCH,DIRECT
"#;
        let v = parse(&convert(yaml).unwrap());
        let node = v["outbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["tag"] == "hy")
            .unwrap();
        assert_eq!(node["auth_str"], "secret");
        assert!(node.get("auth").is_none());
        assert_eq!(node["up_mbps"], 20);
        assert_eq!(node["down_mbps"], 50);
    }
}
