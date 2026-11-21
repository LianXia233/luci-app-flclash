//! config.yaml handling: the scalar view the WebUI shows and the persistence
//! layer that mirrors settings patches into the file so they survive core
//! restarts (and work while the core is stopped).

use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct CoreConfigView {
    pub port: i64,
    pub socks_port: i64,
    pub mixed_port: i64,
    pub allow_lan: bool,
    pub mode: String,
    pub log_level: String,
    pub ipv6: bool,
    pub tun: bool,
    pub tun_stack: String,
    pub dns_enable: bool,
    pub dns_mode: String,
    pub dns_nameservers: Vec<String>,
    pub dns_default_nameservers: Vec<String>,
    pub dns_fake_ip_range: String,
    pub dns_fake_ip_filter: Vec<String>,
    // Advanced DNS (mihomo): fallback chain, per-role nameservers, policy map.
    pub dns_fallback: Vec<String>,
    pub dns_proxy_server_ns: Vec<String>,
    pub dns_direct_ns: Vec<String>,
    pub dns_fb_geoip: Option<bool>,
    pub dns_fb_geoip_code: String,
    pub dns_fb_ipcidr: Vec<String>,
    pub dns_fb_domain: Vec<String>,
    pub dns_ns_policy: Vec<(String, Vec<String>)>,
    pub dns_ipv6: Option<bool>,
}

impl CoreConfigView {
    /// The full dns: view the advanced DNS page edits. Keys are the canonical
    /// mihomo names so what the UI shows is exactly what lands in the file.
    pub fn dns_json(&self) -> Value {
        let mut policy = serde_json::Map::new();
        for (k, v) in &self.dns_ns_policy {
            policy.insert(k.clone(), Value::Array(v.iter().map(|s| Value::String(s.clone())).collect()));
        }
        let mut fb_filter = serde_json::Map::new();
        if let Some(g) = self.dns_fb_geoip { fb_filter.insert("geoip".into(), Value::Bool(g)); }
        if !self.dns_fb_geoip_code.is_empty() { fb_filter.insert("geoip-code".into(), Value::String(self.dns_fb_geoip_code.clone())); }
        if !self.dns_fb_ipcidr.is_empty() { fb_filter.insert("ipcidr".into(), json_arr(&self.dns_fb_ipcidr)); }
        if !self.dns_fb_domain.is_empty() { fb_filter.insert("domain".into(), json_arr(&self.dns_fb_domain)); }
        serde_json::json!({
            "enable": self.dns_enable,
            "enhanced-mode": if self.dns_mode.is_empty() { "redir-host" } else { &self.dns_mode },
            "fake-ip-range": if self.dns_fake_ip_range.is_empty() { "198.18.0.1/16" } else { &self.dns_fake_ip_range },
            "fake-ip-filter": self.dns_fake_ip_filter,
            "nameserver": self.dns_nameservers,
            "default-nameserver": self.dns_default_nameservers,
            "fallback": self.dns_fallback,
            "proxy-server-nameserver": self.dns_proxy_server_ns,
            "direct-nameserver": self.dns_direct_ns,
            "fallback-filter": Value::Object(fb_filter),
            "nameserver-policy": Value::Object(policy),
            "ipv6": self.dns_ipv6,
        })
    }
}

fn json_arr(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.clone())).collect())
}

/// Normalises text coming from subscriptions/editors before it reaches any
/// parser: a UTF-8 BOM on the first key would otherwise defeat the top-level
/// key whitelist (and produce a duplicate `mixed-port`), while CRLF breaks the
/// shell-side `grep '^dns:'` / `awk` block editors.
pub fn normalize_yaml_text(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Copies keys that exist in `old` but not in `new` into `new`. Used after a
/// block edit so unknown DNS/TUN keys (hosts, fake-ip-filter-mode,
/// route-address, interface-name, …) survive a settings save.
fn merge_missing(new: &mut Value, old: &Value) {
    match (new, old) {
        (Value::Object(new_map), Value::Object(old_map)) => {
            for (key, old_value) in old_map {
                match new_map.get_mut(key) {
                    Some(new_value) => merge_missing(new_value, old_value),
                    None => {
                        new_map.insert(key.clone(), old_value.clone());
                    }
                }
            }
        }
        _ => {}
    }
}

fn yaml_bool(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

/// Reads the scalar view of config.yaml without the controller — the
/// fallback that keeps the settings page operable while the core is stopped.
fn get_bool(value: Option<&Value>) -> Option<bool> {
    value.and_then(|v| v.as_bool())
}

fn list_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(text)) => text
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Reads the scalar view of config.yaml without the controller — the
/// fallback that keeps the settings page operable while the core is stopped.
/// YAML-aware on purpose: line matching broke on CRLF, BOM, quoted
/// `nameserver-policy` keys and flow-style blocks.
pub fn file_config(path: &Path) -> CoreConfigView {
    let mut cfg = CoreConfigView {
        mode: "rule".into(),
        log_level: "warning".into(),
        tun_stack: "mixed".into(),
        ..Default::default()
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return cfg;
    };
    let data = normalize_yaml_text(&raw);
    let Ok(doc) = serde_yaml::from_str::<Value>(&data) else {
        return cfg;
    };
    let Some(root) = doc.as_object() else {
        return cfg;
    };

    cfg.port = root.get("port").and_then(|v| v.as_i64()).unwrap_or(0);
    cfg.socks_port = root.get("socks-port").and_then(|v| v.as_i64()).unwrap_or(0);
    cfg.mixed_port = root.get("mixed-port").and_then(|v| v.as_i64()).unwrap_or(0);
    if let Some(v) = root.get("mode").and_then(|v| v.as_str()) {
        cfg.mode = v.to_string();
    }
    if let Some(v) = root.get("log-level").and_then(|v| v.as_str()) {
        cfg.log_level = v.to_string();
    }
    cfg.allow_lan = get_bool(root.get("allow-lan")).unwrap_or(false);
    cfg.ipv6 = get_bool(root.get("ipv6")).unwrap_or(false);

    let mut sniffer_enabled = false;
    if let Some(sniffer) = root.get("sniffer").and_then(|v| v.as_object()) {
        sniffer_enabled = get_bool(sniffer.get("enable")).unwrap_or(false);
    }

    if let Some(tun) = root.get("tun").and_then(|v| v.as_object()) {
        cfg.tun = get_bool(tun.get("enable"))
            .or_else(|| get_bool(tun.get("enabled")))
            .unwrap_or(false);
        if let Some(stack) = tun.get("stack").and_then(|v| v.as_str()) {
            if matches!(stack, "gvisor" | "system" | "mixed") {
                cfg.tun_stack = stack.to_string();
            }
        }
    }

    if let Some(dns) = root.get("dns").and_then(|v| v.as_object()) {
        cfg.dns_enable = get_bool(dns.get("enable"))
            .or_else(|| get_bool(dns.get("enabled")))
            .unwrap_or(false);
        if let Some(v) = dns.get("enhanced-mode").and_then(|v| v.as_str()) {
            cfg.dns_mode = v.to_string();
        }
        if let Some(v) = dns.get("fake-ip-range").and_then(|v| v.as_str()) {
            cfg.dns_fake_ip_range = v.to_string();
        }
        cfg.dns_nameservers = list_of(dns.get("nameserver"));
        cfg.dns_default_nameservers = list_of(dns.get("default-nameserver"));
        cfg.dns_fake_ip_filter = list_of(dns.get("fake-ip-filter"));
        cfg.dns_fallback = list_of(dns.get("fallback"));
        cfg.dns_proxy_server_ns = list_of(dns.get("proxy-server-nameserver"));
        cfg.dns_direct_ns = list_of(dns.get("direct-nameserver"));
        cfg.dns_ipv6 = get_bool(dns.get("ipv6"));

        if let Some(fb) = dns.get("fallback-filter").and_then(|v| v.as_object()) {
            cfg.dns_fb_geoip = get_bool(fb.get("geoip"));
            cfg.dns_fb_geoip_code = fb
                .get("geoip-code")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            cfg.dns_fb_ipcidr = list_of(fb.get("ipcidr"));
            cfg.dns_fb_domain = list_of(fb.get("domain"));
        }

        if let Some(policy) = dns.get("nameserver-policy").and_then(|v| v.as_object()) {
            for (domain, servers) in policy {
                cfg.dns_ns_policy.push((domain.clone(), list_of(Some(servers))));
            }
        }
    }

    // A sniffer block combined with fake-ip is how persisted "mixed" mode
    // is represented; surface it back to the UI as mixed, not raw fake-ip.
    if sniffer_enabled && cfg.dns_mode == "fake-ip" {
        cfg.dns_mode = "mixed".to_string();
    }
    cfg
}

fn parse_bool(patch: &Value, key: &str) -> Option<bool> {
    patch.get(key).and_then(|v| v.as_bool())
}

fn parse_i64(patch: &Value, key: &str) -> Option<i64> {
    patch.get(key).and_then(|v| v.as_i64())
}

fn parse_str<'a>(patch: &'a Value, key: &str) -> Option<&'a str> {
    patch.get(key).and_then(|v| v.as_str())
}

/// Settings patches are rendered into config.yaml line-by-line, so a string
/// containing a newline would inject arbitrary YAML keys into the core config
/// (`{"mode":"rule\nallow-lan: true"}`). Validate here instead of trusting the
/// UI: no control characters anywhere, and enumerated values whitelisted.
pub(crate) fn validate_patch(patch: &Value) -> Result<(), String> {    fn scan(value: &Value) -> Result<(), String> {
        match value {
            Value::String(text) => {
                if text.chars().any(|c| c.is_control()) {
                    return Err(format!("设置值包含非法控制字符: {:?}", text));
                }
                if text.len() > 2048 {
                    return Err("设置值过长".into());
                }
            }
            Value::Array(items) => {
                for item in items {
                    scan(item)?;
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    if key.chars().any(|c| c.is_control()) {
                        return Err("设置键包含非法字符".into());
                    }
                    scan(item)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    scan(patch)?;

    if let Some(mode) = patch.get("mode").and_then(|v| v.as_str()) {
        if !matches!(mode, "rule" | "global" | "direct") {
            return Err(format!("mode 只能是 rule/global/direct，当前: {}", mode));
        }
    }
    if let Some(level) = patch.get("logLevel").and_then(|v| v.as_str()) {
        if !matches!(level, "silent" | "error" | "warning" | "info" | "debug") {
            return Err(format!("logLevel 非法: {}", level));
        }
    }
    if let Some(mode) = patch.get("proxyMode").and_then(|v| v.as_str()) {
        if !matches!(mode, "off" | "redirect" | "tproxy" | "tun") {
            return Err(format!("proxyMode 非法: {}", mode));
        }
    }
    if let Some(mode) = patch.get("ipv6Mode").and_then(|v| v.as_str()) {
        if !matches!(mode, "proxy" | "off" | "direct") {
            return Err(format!("ipv6Mode 非法: {}", mode));
        }
    }
    if let Some(mode) = patch.get("quic").and_then(|v| v.as_str()) {
        if !matches!(mode, "proxy" | "reject" | "bypass") {
            return Err(format!("quic 非法: {}", mode));
        }
    }
    for key in ["mixedPort", "socksPort", "httpPort", "redirPort", "tproxyPort"] {
        if let Some(value) = patch.get(key) {
            let Some(port) = value.as_i64() else {
                return Err(format!("{} 必须是整数", key));
            };
            if !(0..=65535).contains(&port) {
                return Err(format!("{} 超出端口范围: {}", key, port));
            }
        }
    }
    if let Some(path) = patch.get("externalUi").and_then(|v| v.as_str()) {
        if path.contains("..") {
            return Err("externalUi 不能包含 ..".into());
        }
    }
    Ok(())
}

/// PATCHes the running core when reachable; persistence never depends on it.
pub async fn save_config(
    st: &crate::state::AppState,
    patch: &Value,
) -> Result<(), String> {
    validate_patch(patch)?;
    let mut runtime = serde_json::Map::new();
    let tun_runtime = file_config(&st.config_path()).tun || parse_bool(patch, "tun") == Some(true);
    if let Some(v) = parse_str(patch, "mode") {
        runtime.insert("mode".into(), Value::String(v.to_string()));
    }
    if let Some(v) = parse_str(patch, "logLevel") {
        runtime.insert("log-level".into(), Value::String(v.to_string()));
    }
    if let Some(v) = parse_i64(patch, "mixedPort") {
        runtime.insert("mixed-port".into(), Value::from(v));
    }
    if let Some(v) = parse_i64(patch, "socksPort") {
        runtime.insert("socks-port".into(), Value::from(v));
    }
    if let Some(v) = parse_i64(patch, "httpPort") {
        runtime.insert("port".into(), Value::from(v));
    }
    if let Some(v) = parse_bool(patch, "allowLan") {
        runtime.insert("allow-lan".into(), Value::Bool(v));
    }
    if let Some(v) = parse_bool(patch, "ipv6") {
        runtime.insert("ipv6".into(), Value::Bool(v));
    }
    // tun is NOT runtime-patched: taking over the router needs the full
    // auto-route/auto-redirect block, which only a config reload applies.
    //
    // Persist + preflight BEFORE any runtime PATCH: a rejected config must
    // leave both the file and the running core untouched.
    let config_path = st.config_path();
    let previous = std::fs::read(&config_path).ok();
    if let Some(previous) = &previous {
        let last_good = config_path.with_file_name("config.yaml.last-good");
        let _ = write_atomic(&last_good, previous);
    }
    let result = persist_config(&config_path, patch);
    if result.is_ok() {
        if let Err(err) = persist_dns_uci(patch) {
            if let Some(previous) = &previous {
                let _ = write_atomic(&config_path, previous);
            }
            return Err(err);
        }
        // Re-apply router-owned DNS split/rules after a settings edit so a
        // basic-page save cannot leave an inconsistent DNS block.
        if let Ok(text) = std::fs::read_to_string(&config_path) {
            if let Ok(enforced) = enforce_invariants(&text) {
                if enforced != text {
                    let _ = write_atomic(&config_path, enforced.as_bytes());
                }
            }
        }
        if let Err(err) = crate::preflight::validate_config(config_path.clone()).await {
            if let Some(previous) = &previous {
                let _ = write_atomic(&config_path, previous);
            }
            return Err(format!("配置预检失败: {err}"));
        }
    }
    if result.is_ok() && !runtime.is_empty() && !tun_runtime {
        // Failure is deliberately non-fatal: a stopped core must not turn
        // "save settings" into an error (connection refused must not leak).
        let body = serde_json::to_string(&Value::Object(runtime)).unwrap_or_default();
        let _ = crate::ctrl::ctrl_req(
            &st.controller,
            st.secret.as_deref(),
            "PATCH",
            "/configs",
            Some(&body),
            std::time::Duration::from_secs(5),
        )
        .await;
    }
    // dns, tun and tunStack patches only take effect from the startup config
    // file, so a stopped core is left alone but a live one is reloaded.
    // mode is included because sing-box has no runtime mode switch: its
    // global/direct/rule behaviour is baked into the generated route at
    // startup, so a mode change must reload the core to take effect.
    if result.is_ok()
        && (patch.get("dns").is_some()
            || patch.get("tun").is_some()
            || patch.get("tunStack").is_some()
            || patch.get("mode").is_some()
            || patch.get("externalUi").is_some())
    {
        if tun_runtime || patch.get("tun").is_some() || patch.get("tunStack").is_some() {
            // FlClash's native IPC hub deliberately skips updateTun during a
            // controller ApplyConfig. A real process restart is required for
            // listener.start to create/remove Meta and its policy routes.
            let _ = crate::profiles::service(st, "restart").await;
        } else {
            crate::profiles::reload_core(st).await;
        }
    }
    if result.is_ok() {
        let mut proxy_touched = false;
        let mut mode: Option<String> = parse_str(patch, "proxyMode").map(|s| s.to_string());
        if let Some(enabled) = parse_bool(patch, "proxyLan") {
            mode = Some(if enabled { "redirect".into() } else { "off".into() });
        }
        if let Some(ipv6) = parse_str(patch, "ipv6Mode") {
            uci_set_key("flclash.proxy.ipv6", ipv6)?;
            proxy_touched = true;
        }
        if let Some(quic) = parse_str(patch, "quic") {
            uci_set_key("flclash.proxy.quic", quic)?;
            proxy_touched = true;
        }
        if let Some(mode) = &mode {
            uci_set_key("flclash.proxy.mode", mode)?;
            uci_set_key(
                "flclash.proxy.enabled",
                if mode == "off" { "0" } else { "1" },
            )?;
            proxy_touched = true;
        }
        if proxy_touched {
            uci_commit_pkg("flclash")?;
        }
        if let Some(mode) = &mode {
            let action = if mode == "off" { "stop" } else { "start" };
            let output = tokio::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
                .arg(action)
                .output()
                .await
                .map_err(|e| format!("proxy-rules.sh {}: {}", action, e))?;
            if !output.status.success() {
                return Err(format!(
                    "透明代理{}失败",
                    if mode == "off" { "关闭" } else { "启用" }
                ));
            }
        } else if proxy_touched && uci_get_key("flclash.proxy.mode") != "off" {
            // IPv6/QUIC change on a live proxy: rebuild rules, do not start a
            // stopped core as a side effect.
            let _ = tokio::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
                .arg("sync")
                .output()
                .await;
        }
    }
    result
}

fn uci_set_key(key: &str, value: &str) -> Result<(), String> {
    let out = std::process::Command::new("uci")
        .args(["set", &format!("{key}={value}")])
        .output()
        .map_err(|e| format!("uci set {key}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "uci set {key}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn patch_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Value::String(text) => text
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn uci_set_list_key(key: &str, values: &[String]) -> Result<(), String> {
    let _ = std::process::Command::new("uci")
        .args(["-q", "delete", key])
        .output();
    for value in values {
        let out = std::process::Command::new("uci")
            .args(["add_list", &format!("{key}={value}")])
            .output()
            .map_err(|e| format!("uci add_list {key}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "uci add_list {key}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
    }
    Ok(())
}

/// WebUI DNS fields are stored in UCI, not only in the generated config.yaml:
/// the invariant rebuilds DNS from UCI on every profile apply, so presets and
/// custom vendors must survive subscription updates.
fn persist_dns_uci(patch: &Value) -> Result<(), String> {
    let Some(dns) = patch.get("dns").and_then(|v| v.as_object()) else {
        return Ok(());
    };
    // Fine on fresh installs and on upgrades from 1.5.3, where the dns
    // section does not exist yet.
    if uci_get_key("flclash.dns").is_empty() {
        uci_set_key("flclash.dns", "flclash")?;
    }
    if uci_get_key("flclash.dns.split").is_empty() {
        uci_set_key("flclash.dns.split", "1")?;
    }
    let mut changed = false;
    for (patch_key, uci_key) in [
        ("nameserver", "flclash.dns.domestic_dns"),
        ("fallback", "flclash.dns.foreign_dns"),
        ("default-nameserver", "flclash.dns.bootstrap_dns"),
        ("direct-nameserver", "flclash.dns.direct_dns"),
        ("proxy-server-nameserver", "flclash.dns.proxy_dns"),
    ] {
        if let Some(value) = dns.get(patch_key) {
            uci_set_list_key(uci_key, &patch_list(value))?;
            changed = true;
        }
    }
    if changed {
        uci_commit_pkg("flclash")?;
    }
    Ok(())
}

fn uci_commit_pkg(pkg: &str) -> Result<(), String> {
    let out = std::process::Command::new("uci")
        .args(["commit", pkg])
        .output()
        .map_err(|e| format!("uci commit {pkg}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "uci commit {pkg}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn uci_get_key(key: &str) -> String {
    std::process::Command::new("uci")
        .args(["-q", "get", key])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn runtime_mode() -> String {
    let mode = uci_get_key("flclash.proxy.mode");
    if matches!(mode.as_str(), "off" | "redirect" | "tproxy" | "tun") {
        return mode;
    }
    if uci_get_key("flclash.proxy.enabled") == "1" {
        "redirect".into()
    } else {
        "off".into()
    }
}

pub fn runtime_ipv6() -> String {
    let mode = uci_get_key("flclash.proxy.ipv6");
    if matches!(mode.as_str(), "proxy" | "off" | "direct") {
        return mode;
    }
    if uci_get_key("flclash.proxy.filter_aaaa") == "1" {
        "off".into()
    } else {
        "proxy".into()
    }
}

const DOMESTIC_DNS: [&str; 3] = [
    "https://223.5.5.5/dns-query",
    "https://doh.pub/dns-query",
    "tls://dot.pub",
];
const FOREIGN_DNS: [&str; 3] = [
    "https://1.1.1.1/dns-query",
    "https://1.0.0.1/dns-query",
    "https://8.8.8.8/dns-query",
];
const BOOTSTRAP_DNS: [&str; 2] = ["223.5.5.5", "119.29.29.29"];

fn dns_array(items: &[&str]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.to_string())).collect())
}

fn dns_array_owned(items: &[String]) -> Value {
    Value::Array(items.iter().map(|s| Value::String(s.clone())).collect())
}

fn dns_or_default(items: &[String], defaults: &[&str]) -> Value {
    if items.is_empty() {
        dns_array(defaults)
    } else {
        dns_array_owned(items)
    }
}

fn dns_list_present(dns: &serde_json::Map<String, Value>, key: &str) -> bool {
    dns.get(key)
        .and_then(|v| v.as_array())
        .map(|items| !items.is_empty())
        .unwrap_or(false)
}

fn uci_list_key(key: &str) -> Vec<String> {
    // `uci get` prints a list as one space-separated line.
    uci_get_key(key)
        .split_whitespace()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Split DNS: Chinese domains resolve through domestic DoH/DoT, non-CN
/// domains through foreign DoH, and node server domains keep the airport's
/// own DNS when the subscription provided one (falling back to domestic only
/// when it did not). Unknown domains resolve domestically first and fall back
/// to foreign when the answer is not a CN address.
///
/// Domestic/foreign/bootstrap lists come from `flclash.dns` UCI, so WebUI
/// presets survive subscription updates. `flclash.dns.split=0` disables the
/// whole layer and keeps a fully custom DNS block.
fn apply_split_dns(dns: &mut serde_json::Map<String, Value>) {
    let domestic = uci_list_key("flclash.dns.domestic_dns");
    let foreign = uci_list_key("flclash.dns.foreign_dns");
    let bootstrap = uci_list_key("flclash.dns.bootstrap_dns");
    let direct = uci_list_key("flclash.dns.direct_dns");
    let proxy_dns = uci_list_key("flclash.dns.proxy_dns");

    dns.insert("nameserver".into(), dns_or_default(&domestic, &DOMESTIC_DNS));
    dns.insert("fallback".into(), dns_or_default(&foreign, &FOREIGN_DNS));
    // Direct/unknown traffic follows the user's domestic choice; the built-in
    // list is only the last resort when UCI is empty.
    if direct.is_empty() {
        dns.insert(
            "direct-nameserver".into(),
            dns_or_default(&domestic, &DOMESTIC_DNS),
        );
    } else {
        dns.insert("direct-nameserver".into(), dns_array_owned(&direct));
    }
    // Airport-provided node DNS wins; a WebUI override is next, and the
    // domestic default is only a fallback when neither exists.
    if !dns_list_present(dns, "proxy-server-nameserver") {
        let fallback = if proxy_dns.is_empty() {
            dns_or_default(&domestic, &DOMESTIC_DNS)
        } else {
            dns_array_owned(&proxy_dns)
        };
        dns.insert("proxy-server-nameserver".into(), fallback);
    }
    if !dns_list_present(dns, "default-nameserver") {
        dns.insert(
            "default-nameserver".into(),
            dns_or_default(&bootstrap, &BOOTSTRAP_DNS),
        );
    }
    let mut fb = dns
        .get("fallback-filter")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    fb.insert("geoip".into(), Value::Bool(true));
    fb.insert("geoip-code".into(), Value::String("CN".into()));
    dns.insert("fallback-filter".into(), Value::Object(fb));
    let mut policy = dns
        .get("nameserver-policy")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    policy.insert(
        "geosite:cn".into(),
        dns_or_default(&domestic, &DOMESTIC_DNS),
    );
    policy.insert(
        "geosite:private".into(),
        dns_or_default(&domestic, &DOMESTIC_DNS),
    );
    policy.insert(
        "geosite:geolocation-!cn".into(),
        dns_or_default(&foreign, &FOREIGN_DNS),
    );
    dns.insert("nameserver-policy".into(), Value::Object(policy));
}

/// Domestic traffic must never be captured by a proxy rule that happened to
/// come first in the subscription (e.g. a broad geosite before `cn`). Put the
/// CN domain/IP bypass at the top; foreign traffic keeps falling through to
/// the subscription's MATCH/selector rules.
fn ensure_cn_bypass_rules(root: &mut serde_json::Map<String, Value>) {
    let Some(rules) = root.get_mut("rules").and_then(|v| v.as_array_mut()) else {
        return;
    };
    let cn = "GEOSITE,cn,DIRECT";
    if rules.first().and_then(|v| v.as_str()) != Some(cn) {
        rules.retain(|r| r.as_str() != Some(cn));
        rules.insert(0, Value::String(cn.into()));
    }
    if !rules
        .iter()
        .any(|r| r.as_str().map(|s| s.starts_with("GEOIP,CN,")).unwrap_or(false))
    {
        rules.insert(1.min(rules.len()), Value::String("GEOIP,CN,DIRECT,no-resolve".into()));
    }
    if !rules
        .iter()
        .any(|r| r.as_str().map(|s| s.starts_with("GEOIP,PRIVATE,")).unwrap_or(false))
    {
        rules.insert(1.min(rules.len()), Value::String("GEOIP,PRIVATE,DIRECT,no-resolve".into()));
    }
}

/// Pick the subscription's manual selector as the default foreign exit:
/// prefer the usual 节点选择/Proxy/选择 names, then any non-GLOBAL select,
/// then the first group. A group name containing a comma cannot be used in a
/// rule and is skipped.
fn primary_proxy_group(root: &serde_json::Map<String, Value>) -> Option<String> {
    let groups = root.get("proxy-groups").and_then(|v| v.as_array())?;
    let mut first_select = None;
    let mut first_group = None;
    for group in groups {
        let Some(name) = group.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        if name == "GLOBAL" || name.contains(',') {
            continue;
        }
        if first_group.is_none() {
            first_group = Some(name.to_string());
        }
        let kind = group.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "select" {
            if name.contains("节点选择")
                || name.contains("选择")
                || name.contains("代理")
                || name.to_ascii_lowercase().contains("proxy")
            {
                return Some(name.to_string());
            }
            if first_select.is_none() {
                first_select = Some(name.to_string());
            }
        }
    }
    first_select.or(first_group)
}

/// Foreign traffic must end at a proxy. If the subscription's MATCH points at
/// DIRECT (or is missing), retarget it to the manual selector; specific rules
/// (Google/Telegram/OneDrive/CN) earlier in the list are left untouched.
fn ensure_foreign_proxy_match(root: &mut serde_json::Map<String, Value>) {
    let Some(group) = primary_proxy_group(root) else {
        return;
    };
    let Some(rules) = root.get_mut("rules").and_then(|v| v.as_array_mut()) else {
        return;
    };
    let mut match_index = None;
    for (index, rule) in rules.iter().enumerate() {
        if let Some(text) = rule.as_str() {
            if text.starts_with("MATCH,") {
                match_index = Some(index);
            }
        }
    }
    let Some(index) = match_index else {
        rules.push(Value::String(format!("MATCH,{group}")));
        return;
    };
    let target = rules[index]
        .as_str()
        .and_then(|text| text.rsplit(',').next())
        .unwrap_or("");
    if matches!(target, "DIRECT" | "REJECT" | "REJECT-DROP") {
        rules[index] = Value::String(format!("MATCH,{group}"));
    }
}

/// Final router-owned layer. Runs after the subscription and the user mixin,
/// so neither can override controller/listener ownership, loop mark, ports,
/// DNS listener or the TUN switch. serde_yaml also quotes scalars safely.
pub fn enforce_invariants(yaml: &str) -> Result<String, String> {
    let mut doc: Value = serde_yaml::from_str(yaml).map_err(|e| format!("配置解析失败: {e}"))?;
    let root = doc
        .as_object_mut()
        .ok_or_else(|| "配置顶层必须是映射".to_string())?;
    let mode = runtime_mode();
    let ipv6 = runtime_ipv6();

    let controller = {
        let value = uci_get_key("flclash.main.external_controller");
        if value.is_empty() {
            "127.0.0.1:9097".to_string()
        } else {
            value
        }
    };
    root.insert("external-controller".into(), Value::String(controller));
    root.insert("routing-mark".into(), Value::from(333));
    root.insert("allow-lan".into(), Value::Bool(mode != "off"));
    root.insert("ipv6".into(), Value::Bool(ipv6 != "off"));
    if let Ok(port) = uci_get_key("flclash.main.mixed_port").parse::<i64>() {
        if port > 0 {
            root.insert("mixed-port".into(), Value::from(port));
        }
    }
    // Client-only inbound set: custom `listeners` could open arbitrary ports
    // outside the fw4 guard.
    root.remove("listeners");
    root.remove("authentication");
    root.remove("bind-address");

    // Subscriptions may omit dns: entirely; the split layer still has to be
    // written so the nft DNS hijack has a listener with the right servers.
    if !root.contains_key("dns") {
        root.insert("dns".into(), Value::Object(serde_json::Map::new()));
    }
    if let Some(dns) = root.get_mut("dns").and_then(|v| v.as_object_mut()) {
        dns.insert("listen".into(), Value::String("0.0.0.0:1053".into()));
        dns.insert("ipv6".into(), Value::Bool(ipv6 != "off"));
        if uci_get_key("flclash.dns.split") != "0"
            && uci_get_key("flclash.proxy.split_dns") != "0"
        {
            apply_split_dns(dns);
        }
    }
    if let Some(tun) = root.get_mut("tun").and_then(|v| v.as_object_mut()) {
        tun.insert("enable".into(), Value::Bool(mode == "tun"));
        tun.remove("enabled");
    }
    ensure_cn_bypass_rules(root);
    ensure_foreign_proxy_match(root);
    serde_yaml::to_string(&doc).map_err(|e| format!("配置序列化失败: {e}"))
}

/// Renders the dns: block from a patch object (UI field names accepted).
/// Keys absent from the patch keep their current file values (`current`),
/// so the basic settings page never wipes advanced DNS fields.
fn render_dns_block(patch: &Value, current: &CoreConfigView) -> Option<String> {
    let dns = patch.get("dns")?.as_object()?;
    let mut out = String::from("dns:\n");
    let enable = dns.get("enable").and_then(|v| v.as_bool()).unwrap_or(true);
    out.push_str(&format!("  enable: {}\n", yaml_bool(enable)));
    // The LAN transparent-proxy takeover hijacks port 53 to 1053 (see
    // proxy-rules.sh). Emit a DNS listener unconditionally so rewriting the
    // dns block from the WebUI never drops it and breaks DNS for LAN clients.
    out.push_str("  listen: 0.0.0.0:1053\n");
    let mode = dns
        .get("enhanced-mode")
        .and_then(|v| v.as_str())
        .unwrap_or("redir-host");
    // "mixed" is a WebUI-level mode: mihomo itself only understands
    // fake-ip / redir-host. Mixed = fake-ip addressing + domain sniffing,
    // which is how FlClash/OpenClash reconcile fake-ip speed with the
    // domain accuracy of redir-host. Emit the canonical value fake-ip here;
    // the sniffer: block is written alongside it by persist_config().
    let canonical_mode = if mode == "mixed" { "fake-ip" } else { mode };
    out.push_str(&format!("  enhanced-mode: {}\n", canonical_mode));
    let range = dns
        .get("fake-ip-range")
        .and_then(|v| v.as_str())
        .unwrap_or("198.18.0.1/16");
    out.push_str(&format!("  fake-ip-range: {}\n", range));
    let list = |key: &str, fallback: &[&str]| -> Vec<String> {
        let items: Vec<String> = match dns.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            Some(Value::String(s)) => s
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        // An empty nameserver/default list would break resolution; fall back
        // to the built-in defaults instead of emitting a null YAML list.
        if items.is_empty() && !fallback.is_empty() {
            return fallback.iter().map(|s| s.to_string()).collect();
        }
        items
    };
    let filters = list("fake-ip-filter", &["*.lan", "*.local", "+.msftconnecttest.com", "+.msftncsi.com"]);
    let defaults = list("default-nameserver", &["223.5.5.5", "119.29.29.29"]);
    let nameservers = list("nameserver", &["https://223.5.5.5/dns-query", "https://doh.pub/dns-query", "tls://dot.pub"]);
    if !filters.is_empty() {
        out.push_str("  fake-ip-filter:\n");
        for item in &filters {
            out.push_str(&format!("    - '{}'\n", item));
        }
    }
    out.push_str("  default-nameserver:\n");
    for item in &defaults {
        out.push_str(&format!("    - {}\n", item));
    }
    out.push_str("  nameserver:\n");
    for item in &nameservers {
        out.push_str(&format!("    - {}\n", item));
    }
    // ---- advanced fields: patch value wins, otherwise keep the file value
    let adv_list = |key: &str, keep: &[String]| -> Vec<String> {
        match dns.get(key) {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
            Some(Value::String(text)) => text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            Some(Value::Null) => Vec::new(),
            _ => keep.to_vec(),
        }
    };
    let emit_list = |out: &mut String, key: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        out.push_str(&format!("  {}:\n", key));
        for item in items {
            out.push_str(&format!("    - '{}'\n", item.replace('\'', "")));
        }
    };
    emit_list(&mut out, "fallback", &adv_list("fallback", &current.dns_fallback));
    emit_list(&mut out, "proxy-server-nameserver", &adv_list("proxy-server-nameserver", &current.dns_proxy_server_ns));
    emit_list(&mut out, "direct-nameserver", &adv_list("direct-nameserver", &current.dns_direct_ns));
    if let Some(v) = dns.get("ipv6").and_then(|v| v.as_bool()).or(current.dns_ipv6) {
        out.push_str(&format!("  ipv6: {}\n", yaml_bool(v)));
    }
    // fallback-filter: merge per key
    let fb = dns.get("fallback-filter").and_then(|v| v.as_object());
    let fb_geoip = fb.and_then(|m| m.get("geoip")).and_then(|v| v.as_bool()).or(current.dns_fb_geoip);
    let fb_code = fb.and_then(|m| m.get("geoip-code")).and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| current.dns_fb_geoip_code.clone());
    let fb_ipcidr = match fb.and_then(|m| m.get("ipcidr")) {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
        _ => current.dns_fb_ipcidr.clone(),
    };
    let fb_domain = match fb.and_then(|m| m.get("domain")) {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(text)) => text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        _ => current.dns_fb_domain.clone(),
    };
    if fb_geoip.is_some() || !fb_code.is_empty() || !fb_ipcidr.is_empty() || !fb_domain.is_empty() {
        out.push_str("  fallback-filter:\n");
        if let Some(g) = fb_geoip {
            out.push_str(&format!("    geoip: {}\n", yaml_bool(g)));
        }
        if !fb_code.is_empty() {
            out.push_str(&format!("    geoip-code: {}\n", fb_code));
        }
        if !fb_ipcidr.is_empty() {
            out.push_str("    ipcidr:\n");
            for item in &fb_ipcidr {
                out.push_str(&format!("      - '{}'\n", item.replace('\'', "")));
            }
        }
        if !fb_domain.is_empty() {
            out.push_str("    domain:\n");
            for item in &fb_domain {
                out.push_str(&format!("      - '{}'\n", item.replace('\'', "")));
            }
        }
    }
    // nameserver-policy: patch map replaces, absent keeps file map
    let policy: Vec<(String, Vec<String>)> = match dns.get("nameserver-policy") {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                let servers = match v {
                    Value::Array(items) => items.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).filter(|s| !s.is_empty()).collect(),
                    Value::String(text) => text.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                    _ => Vec::new(),
                };
                (k.clone(), servers)
            })
            .filter(|(k, v)| !k.trim().is_empty() && !v.is_empty())
            .collect(),
        _ => current.dns_ns_policy.clone(),
    };
    if !policy.is_empty() {
        out.push_str("  nameserver-policy:\n");
        for (domain, servers) in &policy {
            let key = domain.replace('\'', "");
            if servers.len() == 1 {
                out.push_str(&format!("    '{}': '{}'\n", key, servers[0].replace('\'', "")));
            } else {
                out.push_str(&format!("    '{}':\n", key));
                for server in servers {
                    out.push_str(&format!("      - '{}'\n", server.replace('\'', "")));
                }
            }
        }
    }
    Some(out)
}


/// Renders the tun: block. When enabled it follows the desktop client's
/// takeover shape (auto-route + auto-redirect + dns-hijack), so enabling the
/// switch puts the whole router and its LAN clients behind the core without
/// any external firewall rules.
fn render_tun_block(patch: &Value) -> Option<String> {
    let tun = patch.get("tun")?;
    let enable = match tun {
        Value::Bool(v) => *v,
        Value::Object(map) => map.get("enable").and_then(|v| v.as_bool()).unwrap_or(false),
        _ => return None,
    };
    let mut out = String::from("tun:\n");
    out.push_str(&format!("  enable: {}\n", yaml_bool(enable)));
    if !enable {
        return Some(out);
    }
    let stack = match tun {
        Value::Object(map) => map
            .get("stack")
            .and_then(|v| v.as_str())
            .unwrap_or("mixed")
            .to_string(),
        _ => patch
            .get("tunStack")
            .and_then(|v| v.as_str())
            .unwrap_or("mixed")
            .to_string(),
    };
    let stack = if matches!(stack.as_str(), "gvisor" | "system" | "mixed") {
        stack
    } else {
        "mixed".to_string()
    };
    out.push_str("  stack: ");
    out.push_str(&stack);
    out.push('\n');
    out.push_str("  auto-route: true\n");
    out.push_str("  auto-redirect: true\n");
    out.push_str("  auto-detect-interface: true\n");
    // Keep management and directly connected networks out of the tunnel.
    // Without these exclusions LAN/WAN SSH and LuCI replies may be routed back
    // into Meta, especially when the router has a public WAN address.
    out.push_str("  route-exclude-address:\n");
    for cidr in [
        "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16",
        "172.16.0.0/12", "192.0.0.0/24", "192.168.0.0/16", "224.0.0.0/4",
        "240.0.0.0/4", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8",
    ] {
        out.push_str(&format!("    - {}\n", cidr));
    }
    out.push_str("  dns-hijack:\n");
    out.push_str("    - any:53\n");
    out.push_str("  mtu: 1500\n");
    Some(out)
}

/// Renders the sniffer: block for the WebUI "mixed" DNS mode. Mixed mode
/// keeps fake-ip fast-path addressing but recovers the real domain from
/// HTTP/TLS traffic, matching the behaviour users expect from FlClash's
/// mixed DNS handling. When the mode is not "mixed" this returns None so
/// any previously written sniffer block is left untouched.
fn render_sniffer_block(patch: &Value) -> Option<String> {
    let dns = patch.get("dns")?.as_object()?;
    let mode = dns.get("enhanced-mode").and_then(|v| v.as_str()).unwrap_or("");
    if mode != "mixed" {
        return None;
    }
    let mut out = String::from("sniffer:\n");
    out.push_str("  enable: true\n");
    out.push_str("  sniff:\n");
    out.push_str("    HTTP:\n");
    out.push_str("      ports: [80, 8080-8880]\n");
    out.push_str("    TLS:\n");
    out.push_str("      ports: [443, 8443]\n");
    out.push_str("  sniff-by-dns: false\n");
    Some(out)
}

/// Mirrors a settings patch into config.yaml. Top-level scalar keys are
/// line-edited in place; tun, dns and sniffer are block-edited.
pub fn persist_config(path: &Path, patch: &Value) -> Result<(), String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read config: {}", e))?;
    let data = normalize_yaml_text(&raw);
    let current = file_config(path);
    let dns_block = render_dns_block(patch, &current);
    let tun_block = render_tun_block(patch);
    let sniffer_block = render_sniffer_block(patch);
    // Whenever the UI explicitly sets a DNS mode, reconcile the sniffer:
    // block — write it for "mixed", strip a stale one for the other modes.
    let dns_mode_set = patch
        .get("dns")
        .and_then(|d| d.get("enhanced-mode"))
        .and_then(|v| v.as_str())
        .is_some();

    let mut scalars: Vec<(&str, String)> = Vec::new();
    if let Some(v) = parse_str(patch, "mode") {
        scalars.push(("mode", v.to_string()));
    }
    if let Some(v) = parse_str(patch, "logLevel") {
        scalars.push(("log-level", v.to_string()));
    }
    if let Some(v) = parse_i64(patch, "mixedPort") {
        if v > 0 {
            scalars.push(("mixed-port", v.to_string()));
        }
    }
    if let Some(v) = parse_i64(patch, "socksPort") {
        if v > 0 {
            scalars.push(("socks-port", v.to_string()));
        }
    }
    if let Some(v) = parse_i64(patch, "httpPort") {
        if v > 0 {
            scalars.push(("port", v.to_string()));
        }
    }
    if let Some(v) = parse_i64(patch, "redirPort") {
        if v > 0 {
            scalars.push(("redir-port", v.to_string()));
        }
    }
    if let Some(v) = parse_i64(patch, "tproxyPort") {
        if v > 0 {
            scalars.push(("tproxy-port", v.to_string()));
        }
    }
    if let Some(v) = parse_bool(patch, "allowLan") {
        scalars.push(("allow-lan", yaml_bool(v).to_string()));
    }
    if let Some(v) = parse_bool(patch, "ipv6") {
        scalars.push(("ipv6", yaml_bool(v).to_string()));
    }
    if let Some(v) = parse_str(patch, "externalUi") {
        scalars.push(("external-ui", v.to_string()));
    }
    let tun = parse_bool(patch, "tun");

    let mut out: Vec<String> = Vec::new();
    let mut in_tun = false;
    let mut tun_done = false;
    let mut in_dns = false;
    let mut dns_done = false;
    let mut in_sniffer = false;
    let mut sniffer_done = false;
    let mut appended: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in data.lines() {
        let trimmed = line.trim();
        let at_root = !trimmed.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && trimmed.contains(':');
        let mut key = String::new();
        if at_root {
            key = trimmed.split(':').next().unwrap_or("").trim().to_string();
        }
        if dns_block.is_some() {
            if at_root && key == "dns" {
                in_dns = true;
                dns_done = true;
                if let Some(block) = &dns_block {
                    for bline in block.lines() {
                        out.push(bline.to_string());
                    }
                }
                continue;
            }
            if in_dns {
                if line.starts_with(' ') || line.starts_with('\t') || trimmed.is_empty() {
                    continue; // skip old dns block lines
                }
                in_dns = false;
            }
        }
        // Mixed DNS mode writes a canonical sniffer: block; when switching
        // away from mixed we drop any stale one so sniffing does not linger.
        if dns_mode_set {
            if at_root && key == "sniffer" {
                sniffer_done = true;
                if let Some(block) = &sniffer_block {
                    for bline in block.lines() {
                        out.push(bline.to_string());
                    }
                }
                in_sniffer = true;
                continue;
            }
            if in_sniffer {
                if line.starts_with(' ') || line.starts_with('\t') || trimmed.is_empty() {
                    continue; // skip old sniffer block lines
                }
                in_sniffer = false;
            }
        }
        if tun_block.is_some() {
            if at_root && key == "tun" {
                in_tun = true;
                tun_done = true;
                if let Some(block) = &tun_block {
                    for bline in block.lines() {
                        out.push(bline.to_string());
                    }
                }
                continue;
            }
            if in_tun {
                if line.starts_with(' ') || line.starts_with('\t') || trimmed.is_empty() {
                    continue; // skip old tun block lines
                }
                in_tun = false;
            }
        }
        if at_root {
            if at_root && key == "tun" {
                in_tun = true;
                tun_done = true;
                out.push(line.to_string());
                continue;
            }
            if in_tun {
                if line.starts_with(' ') || line.starts_with('\t') {
                    // Only rewrite `enable` when the patch actually carries a
                    // `tun` value. Calling tun.unwrap() unconditionally made
                    // every patch without `tun` (e.g. {"mode":"global"})
                    // panic on a config that already had a tun: block.
                    if trimmed.strip_prefix("enable:").is_some() {
                        if let Some(enabled) = tun {
                            let indent = line.len() - line.trim_start().len();
                            out.push(format!(
                                "{}enable: {}",
                                " ".repeat(indent),
                                yaml_bool(enabled)
                            ));
                            continue;
                        }
                    }
                } else {
                    in_tun = false;
                }
            }
        }
        if at_root {
            if let Some((_, value)) = scalars.iter().find(|(k, _)| *k == key) {
                out.push(format!("{}: {}", key, value));
                appended.insert(key);
                continue;
            }
        }
        out.push(line.to_string());
    }
    let order = ["mixed-port", "socks-port", "port", "redir-port", "tproxy-port", "mode", "log-level", "allow-lan", "ipv6", "external-ui"];
    for key in order {
        if let Some((_, value)) = scalars.iter().find(|(k, _)| *k == key) {
            if !appended.contains(key) {
                out.push(format!("{}: {}", key, value));
            }
        }
    }
    if tun.is_some() && !tun_done {
        if let Some(block) = &tun_block {
            for bline in block.lines() {
                out.push(bline.to_string());
            }
        } else {
            out.push("tun:".to_string());
            out.push(format!("  enable: {}", yaml_bool(tun.unwrap())));
        }
    }
    if dns_block.is_some() && !dns_done {
        if let Some(block) = &dns_block {
            for bline in block.lines() {
                out.push(bline.to_string());
            }
        }
    }
    if let Some(block) = &sniffer_block {
        if !sniffer_done {
            for bline in block.lines() {
                out.push(bline.to_string());
            }
        }
    }

    let mut final_text = format!("{}\n", out.join("\n"));
    // The line-based DNS/TUN renderers only know their modelled keys. Before
    // publishing, merge every unknown key from the previous document back in
    // (hosts, fake-ip-filter-mode, respect-rules, route-address,
    // interface-name, …), and let serde_yaml quote scalars safely (#, ', ::1).
    if dns_block.is_some() || tun_block.is_some() || sniffer_block.is_some() {
        if let (Ok(mut new_doc), Ok(old_doc)) = (
            serde_yaml::from_str::<Value>(&final_text),
            serde_yaml::from_str::<Value>(&data),
        ) {
            for key in ["dns", "tun", "sniffer"] {
                if let (Some(new_value), Some(old_value)) =
                    (new_doc.get_mut(key), old_doc.get(key))
                {
                    merge_missing(new_value, old_value);
                }
            }
            if let Ok(serialized) = serde_yaml::to_string(&new_doc) {
                final_text = serialized;
            }
        }
    }
    write_atomic(path, final_text.as_bytes())
}

/// Atomically replaces a file (tmp + rename), the pattern that made the old
/// crash loop survivable: readers never see a half-written config.
///
/// The temp name includes the pid and a counter: two concurrent writers used
/// to share `<file>.yaml.tmp`, so one could rename the other's partial body
/// into place.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("invalid path: {}", path.display()))?;
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{}.{}.{}.tmp", name, std::process::id(), seq));
    // 0600 on purpose: config.yaml, profiles, mixin and backups all carry
    // subscription credentials or the API secret-adjacent material. OpenWrt's
    // default umask is 022, so a plain fs::write would publish them world-wide.
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| format!("write {}: {}", tmp.display(), e))?;
        use std::io::Write;
        file.write_all(data)
            .map_err(|e| format!("write {}: {}", tmp.display(), e))?;
        // fsync before rename: a power cut must not leave a zero-length file
        // where the live config used to be.
        file.sync_all()
            .map_err(|e| format!("fsync {}: {}", tmp.display(), e))?;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("rename {}: {}", path.display(), e));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn advanced_dns_roundtrip() {
        let dir = std::env::temp_dir().join(format!("flclash-dns-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "mixed-port: 7892\nmode: rule\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n  nameserver:\n    - https://223.5.5.5/dns-query\n").unwrap();
        let patch = serde_json::json!({"dns": {
            "enable": true,
            "enhanced-mode": "fake-ip",
            "nameserver": ["https://223.5.5.5/dns-query"],
            "fallback": ["https://1.1.1.1/dns-query", "tls://8.8.4.4"],
            "proxy-server-nameserver": ["https://223.5.5.5/dns-query"],
            "direct-nameserver": ["223.5.5.5"],
            "fallback-filter": {"geoip": true, "geoip-code": "CN", "ipcidr": ["240.0.0.0/4"], "domain": ["+.google.com"]},
            "nameserver-policy": {"+.internal.example.com": ["10.0.0.1"], "geosite:cn": ["223.5.5.5", "119.29.29.29"]},
            "ipv6": false
        }});
        persist_config(&path, &patch).unwrap();
        let cfg = file_config(&path);
        assert_eq!(cfg.dns_fallback, vec!["https://1.1.1.1/dns-query", "tls://8.8.4.4"]);
        assert_eq!(cfg.dns_proxy_server_ns, vec!["https://223.5.5.5/dns-query"]);
        assert_eq!(cfg.dns_direct_ns, vec!["223.5.5.5"]);
        assert_eq!(cfg.dns_fb_geoip, Some(true));
        assert_eq!(cfg.dns_fb_geoip_code, "CN");
        assert_eq!(cfg.dns_fb_ipcidr, vec!["240.0.0.0/4"]);
        assert_eq!(cfg.dns_fb_domain, vec!["+.google.com"]);
        assert_eq!(cfg.dns_ns_policy.len(), 2);
        assert!(cfg.dns_ns_policy.iter().any(|(k, v)| k == "+.internal.example.com" && v == &vec!["10.0.0.1".to_string()]));
        assert!(cfg.dns_ns_policy.iter().any(|(k, v)| k == "geosite:cn" && v.len() == 2));
        assert_eq!(cfg.dns_ipv6, Some(false));
        // A later basic-settings patch must NOT wipe the advanced fields.
        let basic = serde_json::json!({"dns": {"enable": true, "enhanced-mode": "fake-ip", "nameserver": ["https://doh.pub/dns-query"]}});
        persist_config(&path, &basic).unwrap();
        let cfg2 = file_config(&path);
        assert_eq!(cfg2.dns_fallback.len(), 2);
        assert_eq!(cfg2.dns_ns_policy.len(), 2);
        assert_eq!(cfg2.dns_fb_geoip, Some(true));
        assert_eq!(cfg2.dns_nameservers, vec!["https://doh.pub/dns-query"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    use super::*;

    #[test]
    fn test_persist_scalars_and_tun() {
        let dir = std::env::temp_dir().join(format!("flclash-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "mixed-port: 7892\nmode: rule\ntun:\n  enable: false\n  stack: gvisor\nproxies:\n  - name: a\n",
        )
        .unwrap();

        let patch = serde_json::json!({"mode": "global", "mixedPort": 7893, "tun": true, "allowLan": true});
        persist_config(&path, &patch).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mixed-port: 7893"));
        assert!(text.contains("mode: global"));
        assert!(text.contains("allow-lan: true"));
        // Enabling tun rewrites the whole block into the takeover shape.
        assert!(text.contains("enable: true"));
        assert!(text.contains("stack: mixed"));
        assert!(text.contains("auto-route: true"));
        assert!(text.contains("auto-redirect: true"));
        assert!(text.contains("route-exclude-address:"));
        assert!(text.contains("192.168.0.0/16"));
        assert!(text.contains("fc00::/7"));
        assert!(text.contains("dns-hijack:"));
        assert!(text.contains("proxies:"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_mixed_dns_and_tun_stack() {
        let dir = std::env::temp_dir().join(format!("flclash-test3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "mixed-port: 7892\nmode: rule\ndns:\n  enhanced-mode: fake-ip\n").unwrap();

        let patch = serde_json::json!({
            "dns": {"enhanced-mode": "mixed"},
            "tunStack": "gvisor"
        });
        persist_config(&path, &patch).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("enhanced-mode: fake-ip"), "mixed maps to canonical fake-ip");
        assert!(text.contains("sniffer:"), "mixed writes a sniffer block");
        assert!(text.contains("enable: true"), "sniffer enabled");

        // Reading it back must surface "mixed" again (fake-ip + sniffer).
        let cfg = file_config(&path);
        assert_eq!(cfg.dns_mode, "mixed", "round-trip preserves mixed mode");

        // Switching back to fake-ip strips the stale sniffer block.
        persist_config(&path, &serde_json::json!({"dns": {"enhanced-mode": "fake-ip"}})).unwrap();
        let text2 = std::fs::read_to_string(&path).unwrap();
        assert!(!text2.contains("sniffer:"), "leaving mixed removes sniffer block");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_tun_stack_persist() {
        let dir = std::env::temp_dir().join(format!("flclash-test4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "mixed-port: 7892\n").unwrap();
        let patch = serde_json::json!({"tun": true, "tunStack": "system"});
        persist_config(&path, &patch).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("stack: system"), "tun stack is persisted");
        let cfg = file_config(&path);
        assert_eq!(cfg.tun_stack, "system", "tun stack read back");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_file_config_view() {
        let dir = std::env::temp_dir().join(format!("flclash-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "mixed-port: 7892\nsocks-port: 7891\nmode: rule\nlog-level: info\nallow-lan: false\nipv6: true\ntun:\n  enable: true\n").unwrap();
        let cfg = file_config(&path);
        assert_eq!(cfg.mixed_port, 7892);
        assert_eq!(cfg.socks_port, 7891);
        assert!(cfg.tun);
        assert_eq!(cfg.mode, "rule");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn patch_validation_blocks_yaml_injection() {
        // A newline in a scalar used to become a new config.yaml line.
        assert!(validate_patch(&serde_json::json!({"mode": "rule\nallow-lan: true"})).is_err());
        assert!(validate_patch(&serde_json::json!({"dns": {"nameserver": ["1.1.1.1\nx: y"]}})).is_err());
        assert!(validate_patch(&serde_json::json!({"mode": "global", "logLevel": "debug"})).is_ok());
        assert!(validate_patch(&serde_json::json!({"mode": "weird"})).is_err());
        assert!(validate_patch(&serde_json::json!({"logLevel": "verbose"})).is_err());
        assert!(validate_patch(&serde_json::json!({"mixedPort": 70000})).is_err());
        assert!(validate_patch(&serde_json::json!({"mixedPort": 7892})).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_writes_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("flclash-mode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("secret.yaml");
        write_atomic(&path, b"a: 1\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "credentials must not be world-readable");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn patch_without_tun_does_not_panic_on_tun_block() {
        let dir = std::env::temp_dir().join(format!("flclash-tun-panic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "mode: rule\ntun:\n  enable: false\n  interface-name: mytun\n",
        )
        .unwrap();
        // Before the fix this hit tun.unwrap() with None and, with
        // panic=abort, killed the whole API process.
        persist_config(&path, &serde_json::json!({"mode": "global"})).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mode: global"));
        assert!(text.contains("interface-name: mytun"));
        assert!(text.contains("enable: false"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dns_save_preserves_unknown_keys() {
        let dir = std::env::temp_dir().join(format!("flclash-dns-keep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "mixed-port: 7892\ndns:\n  enable: true\n  enhanced-mode: fake-ip\n  hosts:\n    router.lan: 192.168.1.1\n  fake-ip-filter-mode: blacklist\n  nameserver:\n    - 223.5.5.5\n",
        )
        .unwrap();
        persist_config(
            &path,
            &serde_json::json!({"dns": {"enable": true, "enhanced-mode": "fake-ip", "nameserver": ["1.1.1.1"]}}),
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("hosts:"));
        assert!(text.contains("router.lan"));
        assert!(text.contains("fake-ip-filter-mode"));
        assert!(text.contains("1.1.1.1"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn normalize_strips_bom_and_crlf() {
        let text = normalize_yaml_text("\u{feff}mixed-port: 7892\r\ndns:\r\n  enable: true\r\n");
        assert!(text.starts_with("mixed-port: 7892\n"));
        assert!(!text.contains('\r'));
        assert!(!text.contains('\u{feff}'));
    }

    #[test]
    fn invariants_override_subscription_control_plane() {
        let out = enforce_invariants(
            "external-controller: 0.0.0.0:9097\nrouting-mark: 1\nlisteners:\n  - type: socks\n    port: 1080\ntun:\n  enable: true\n",
        )
        .unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["external-controller"], "127.0.0.1:9097");
        assert_eq!(doc["routing-mark"], 333);
        assert!(doc.get("listeners").is_none());
        assert_eq!(doc["tun"]["enable"], false, "mode off must disable TUN");
    }

    #[test]
    fn invariants_split_dns_and_keep_airport_node_dns() {
        let out = enforce_invariants(
            "dns:\n  nameserver: ['https://airport.example/dns-query']\n  proxy-server-nameserver: ['https://airport.example/dns-query']\nproxy-groups:\n  - name: 节点选择\n    type: select\n    proxies: [A]\nproxies:\n  - name: A\n    type: ss\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        let ns = doc["dns"]["nameserver"].as_array().unwrap();
        assert!(ns.iter().any(|v| v.as_str() == Some("https://223.5.5.5/dns-query")));
        // Airport-provided node DNS wins over the domestic default.
        assert_eq!(
            doc["dns"]["proxy-server-nameserver"][0],
            "https://airport.example/dns-query"
        );
        assert!(doc["dns"]["nameserver-policy"]["geosite:cn"].is_array());
        assert!(doc["dns"]["nameserver-policy"]["geosite:geolocation-!cn"].is_array());
        assert_eq!(doc["rules"][0], "GEOSITE,cn,DIRECT");
        assert_eq!(doc["rules"].as_array().unwrap().last().unwrap(), "MATCH,节点选择");
    }

    #[test]
    fn invariants_create_dns_block_when_subscription_omits_it() {
        let out = enforce_invariants("proxies: []\nrules:\n  - MATCH,DIRECT\n").unwrap();
        let doc: Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(doc["dns"]["listen"], "0.0.0.0:1053");
        assert!(!doc["dns"]["nameserver"].as_array().unwrap().is_empty());
        assert!(!doc["dns"]["fallback"].as_array().unwrap().is_empty());
        assert!(doc["dns"]["nameserver-policy"]["geosite:cn"].is_array());
        assert!(doc["dns"]["nameserver-policy"]["geosite:geolocation-!cn"].is_array());
    }
}
