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
}

fn yaml_bool(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

/// Reads the scalar view of config.yaml without the controller — the
/// fallback that keeps the settings page operable while the core is stopped.
pub fn file_config(path: &Path) -> CoreConfigView {
    let mut cfg = CoreConfigView {
        mode: "rule".into(),
        log_level: "info".into(),
        tun_stack: "mixed".into(),
        ..Default::default()
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return cfg;
    };
    let mut in_tun = false;
    let mut in_dns = false;
    let mut in_sniffer = false;
    let mut sniffer_enabled = false;
    let mut dns_key = String::new();
    for line in data.lines() {
        let trimmed = line.trim();
        let at_root = !trimmed.is_empty()
            && !line.starts_with(' ')
            && !line.starts_with('\t')
            && trimmed.contains(':');
        if at_root {
            let (key, value) = trimmed.split_once(':').unwrap();
            let value = value.trim();
            in_tun = key == "tun";
            in_dns = key == "dns";
            in_sniffer = key == "sniffer";
            dns_key.clear();
            match key {
                "port" => cfg.port = value.parse().unwrap_or(0),
                "socks-port" => cfg.socks_port = value.parse().unwrap_or(0),
                "mixed-port" => cfg.mixed_port = value.parse().unwrap_or(0),
                "mode" => cfg.mode = value.to_string(),
                "log-level" => cfg.log_level = value.to_string(),
                "allow-lan" => cfg.allow_lan = value == "true",
                "ipv6" => cfg.ipv6 = value == "true",
                _ => {}
            }
            continue;
        }
        if in_tun {
            if let Some(rest) = trimmed.strip_prefix("enable:") {
                cfg.tun = rest.trim() == "true";
            } else if let Some(rest) = trimmed.strip_prefix("stack:") {
                let stack = rest.trim();
                if matches!(stack, "gvisor" | "system" | "mixed") {
                    cfg.tun_stack = stack.to_string();
                }
            }
            continue;
        }
        if in_sniffer {
            if let Some(rest) = trimmed.strip_prefix("enable:") {
                if rest.trim() == "true" {
                    sniffer_enabled = true;
                }
            }
            continue;
        }
        if in_dns {
            if trimmed.starts_with("- ") {
                let item = trimmed[2..].trim().to_string();
                if dns_key == "nameserver" {
                    cfg.dns_nameservers.push(item);
                } else if dns_key == "default-nameserver" {
                    cfg.dns_default_nameservers.push(item);
                } else if dns_key == "fake-ip-filter" {
                    cfg.dns_fake_ip_filter.push(item);
                }
                continue;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                let value = value.trim();
                match key {
                    "enable" => cfg.dns_enable = value == "true",
                    "enhanced-mode" => cfg.dns_mode = value.to_string(),
                    "fake-ip-range" => cfg.dns_fake_ip_range = value.to_string(),
                    _ => {}
                }
                dns_key = key.to_string();
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

/// PATCHes the running core when reachable; persistence never depends on it.
pub async fn save_config(
    st: &crate::state::AppState,
    patch: &Value,
) -> Result<(), String> {
    let mut runtime = serde_json::Map::new();
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
    if !runtime.is_empty() {
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
    let result = persist_config(&st.config_path(), patch);
    // dns, tun and tunStack patches only take effect from the startup config
    // file, so a stopped core is left alone but a live one is reloaded.
    if result.is_ok()
        && (patch.get("dns").is_some()
            || patch.get("tun").is_some()
            || patch.get("tunStack").is_some())
    {
        crate::profiles::reload_core(st).await;
    }
    result
}

/// Renders the dns: block from a patch object (UI field names accepted).
fn render_dns_block(patch: &Value) -> Option<String> {
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
        .unwrap_or("fake-ip");
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
        match dns.get(key) {
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
            _ if !fallback.is_empty() => fallback.iter().map(|s| s.to_string()).collect(),
            _ => Vec::new(),
        }
    };
    let filters = list("fake-ip-filter", &["*.lan", "*.local", "+.msftconnecttest.com", "+.msftncsi.com"]);
    let defaults = list("default-nameserver", &["223.5.5.5", "119.29.29.29"]);
    let nameservers = list("nameserver", &["https://223.5.5.5/dns-query", "https://doh.pub/dns-query", "tls://120.53.53.53"]);
    out.push_str("  fake-ip-filter:\n");
    for item in &filters {
        out.push_str(&format!("    - '{}'\n", item));
    }
    out.push_str("  default-nameserver:\n");
    for item in &defaults {
        out.push_str(&format!("    - {}\n", item));
    }
    out.push_str("  nameserver:\n");
    for item in &nameservers {
        out.push_str(&format!("    - {}\n", item));
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
    let data = std::fs::read_to_string(path).map_err(|e| format!("read config: {}", e))?;
    let dns_block = render_dns_block(patch);
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
                    if let Some(rest) = trimmed.strip_prefix("enable:") {
                        let _ = rest;
                        out.push(format!("  enable: {}", yaml_bool(tun.unwrap())));
                        continue;
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
    let order = ["mixed-port", "socks-port", "port", "redir-port", "tproxy-port", "mode", "log-level", "allow-lan", "ipv6"];
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

    let final_text = format!("{}\n", out.join("\n"));
    write_atomic(path, final_text.as_bytes())
}

/// Atomically replaces a file (tmp + rename), the pattern that made the old
/// crash loop survivable: readers never see a half-written config.
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
    }
    let tmp = path.with_extension("yaml.tmp");
    std::fs::write(&tmp, data).map_err(|e| format!("write {}: {}", tmp.display(), e))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {}", path.display(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
