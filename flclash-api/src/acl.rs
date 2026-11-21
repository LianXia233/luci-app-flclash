//! LAN ACL for transparent proxy (Nikki-style client allow/block lists).
//!
//! Storage is UCI (`flclash.proxy`), which proxy-rules.sh reads directly when
//! it renders nftables rules. This module owns validation so nothing
//! invalid ever reaches the firewall scripts, and mirrors the exact same
//! rules in shell (`valid_ip_or_cidr` / `valid_mac` / `valid_port`).

use serde_json::{json, Value};

pub const MAX_LIST_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct Acl {
    pub mode: String, // all | allow | block
    pub ips: Vec<String>,
    pub ips6: Vec<String>,
    pub macs: Vec<String>,
    pub bypass_cidrs: Vec<String>,
    pub bypass_cidrs6: Vec<String>,
    pub bypass_ports: Vec<String>,
    pub local_takeover: bool,
}

impl Default for Acl {
    fn default() -> Self {
        Acl {
            mode: "all".into(),
            ips: vec![],
            ips6: vec![],
            macs: vec![],
            bypass_cidrs: vec![],
            bypass_cidrs6: vec![],
            bypass_ports: vec![],
            local_takeover: true,
        }
    }
}

pub fn valid_ip_or_cidr(s: &str) -> bool {
    let (ip, has_mask) = match s.split_once('/') {
        Some((ip, m)) => {
            let Ok(m) = m.parse::<u8>() else { return false };
            if m > 32 {
                return false;
            }
            (ip, true)
        }
        None => (s, false),
    };
    if has_mask && s.ends_with('/') {
        return false;
    }
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|p| {
        !p.is_empty()
            && p.len() <= 3
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u16>().map(|v| v <= 255).unwrap_or(false)
    })
}

pub fn valid_ip6_or_cidr(s: &str) -> bool {
    let (ip, has_mask) = match s.split_once('/') {
        Some((ip, m)) => {
            let Ok(m) = m.parse::<u8>() else { return false };
            if m > 128 {
                return false;
            }
            (ip, true)
        }
        None => (s, false),
    };
    if has_mask && s.ends_with('/') {
        return false;
    }
    ip.parse::<std::net::Ipv6Addr>().is_ok()
}

pub fn valid_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() == 6
        && parts.iter().all(|p| {
            p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit())
        })
}

pub fn valid_port(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 5
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u32>().map(|v| (1..=65535).contains(&v)).unwrap_or(false)
}

/// Parse and validate a PUT payload. Returns a normalized ACL or an error
/// message suitable for a 400 response.
pub fn parse_payload(data: &Value) -> Result<Acl, String> {
    let mut acl = Acl::default();
    if let Some(mode) = data.get("mode").and_then(|v| v.as_str()) {
        if !matches!(mode, "all" | "allow" | "block") {
            return Err("mode 必须是 all / allow / block".into());
        }
        acl.mode = mode.into();
    }
    let read_list = |key: &str, check: fn(&str) -> bool, label: &str| -> Result<Vec<String>, String> {
        let Some(arr) = data.get(key) else { return Ok(vec![]) };
        let arr = arr.as_array().ok_or(format!("{key} 必须是数组"))?;
        if arr.len() > MAX_LIST_LEN {
            return Err(format!("{label} 最多 {MAX_LIST_LEN} 条"));
        }
        let mut out = Vec::new();
        for item in arr {
            let s = item.as_str().unwrap_or("").trim().to_string();
            if s.is_empty() {
                continue;
            }
            if !check(&s) {
                return Err(format!("{label} 含非法条目: {s}"));
            }
            if !out.contains(&s) {
                out.push(s);
            }
        }
        Ok(out)
    };
    acl.ips = read_list("ips", valid_ip_or_cidr, "客户端 IP/CIDR")?;
    acl.ips6 = read_list("ips6", valid_ip6_or_cidr, "客户端 IPv6/CIDR")?;
    acl.macs = read_list("macs", |s| valid_mac(s), "客户端 MAC")
        .map(|v| v.into_iter().map(|m| m.to_ascii_lowercase()).collect())?;
    acl.bypass_cidrs = read_list("bypassCidrs", valid_ip_or_cidr, "排除网段")?;
    acl.bypass_cidrs6 = read_list("bypassCidrs6", valid_ip6_or_cidr, "排除 IPv6 网段")?;
    acl.bypass_ports = read_list("bypassPorts", valid_port, "排除端口")?;
    if let Some(v) = data.get("localTakeover").and_then(|v| v.as_bool()) {
        acl.local_takeover = v;
    }
    if acl.mode == "allow"
        && acl.ips.is_empty()
        && acl.ips6.is_empty()
        && acl.macs.is_empty()
    {
        return Err("白名单模式下客户端列表不能为空（否则没有任何设备走代理）".into());
    }
    Ok(acl)
}

impl Acl {
    pub fn to_json(&self) -> Value {
        json!({
            "mode": self.mode,
            "ips": self.ips,
            "ips6": self.ips6,
            "macs": self.macs,
            "bypassCidrs": self.bypass_cidrs,
            "bypassCidrs6": self.bypass_cidrs6,
            "bypassPorts": self.bypass_ports,
            "localTakeover": self.local_takeover,
        })
    }
}

// ------------------------------------------------------------------ uci glue

fn uci(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("uci").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn split_uci_list(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn uci_list(key: &str) -> Vec<String> {
    // `uci get` prints a UCI list as ONE space-separated line, not one item
    // per line. Using lines() here silently dropped every entry beyond the
    // first (and then the validator dropped the combined string too).
    uci(&["-q", "get", key])
        .map(|s| split_uci_list(&s))
        .unwrap_or_default()
}

pub fn load() -> Acl {
    let mut acl = Acl::default();
    if let Some(mode) = uci(&["-q", "get", "flclash.proxy.acl_mode"]) {
        if matches!(mode.as_str(), "allow" | "block") {
            acl.mode = mode;
        }
    }
    acl.ips = uci_list("flclash.proxy.acl_ip").into_iter().filter(|s| valid_ip_or_cidr(s)).collect();
    acl.ips6 = uci_list("flclash.proxy.acl_ip6").into_iter().filter(|s| valid_ip6_or_cidr(s)).collect();
    acl.macs = uci_list("flclash.proxy.acl_mac").into_iter().filter(|s| valid_mac(s)).collect();
    acl.bypass_cidrs = uci_list("flclash.proxy.bypass_cidr").into_iter().filter(|s| valid_ip_or_cidr(s)).collect();
    acl.bypass_cidrs6 = uci_list("flclash.proxy.bypass_cidr6").into_iter().filter(|s| valid_ip6_or_cidr(s)).collect();
    acl.bypass_ports = uci_list("flclash.proxy.bypass_port").into_iter().filter(|s| valid_port(s)).collect();
    if let Some(v) = uci(&["-q", "get", "flclash.proxy.local_takeover"]) {
        acl.local_takeover = v != "0";
    }
    acl
}

pub fn save(acl: &Acl) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let out = std::process::Command::new("uci")
            .args(args)
            .output()
            .map_err(|e| format!("uci: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!("uci {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
        }
    };
    run(&["-q", "set", "flclash.proxy=proxy"])?;
    run(&["set", &format!("flclash.proxy.acl_mode={}", acl.mode)])?;
    for (key, values) in [
        ("acl_ip", &acl.ips),
        ("acl_ip6", &acl.ips6),
        ("acl_mac", &acl.macs),
        ("bypass_cidr", &acl.bypass_cidrs),
        ("bypass_cidr6", &acl.bypass_cidrs6),
        ("bypass_port", &acl.bypass_ports),
    ] {
        // delete-then-add keeps the list exact (no stale entries).
        let _ = run(&["-q", "delete", &format!("flclash.proxy.{key}")]);
        for v in values {
            run(&["add_list", &format!("flclash.proxy.{key}={v}")])?;
        }
    }
    run(&["set", &format!("flclash.proxy.local_takeover={}", if acl.local_takeover { "1" } else { "0" })])?;
    run(&["commit", "flclash"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uci_lists_are_split_on_whitespace() {
        assert_eq!(split_uci_list("192.168.1.5 192.168.1.6"), vec!["192.168.1.5", "192.168.1.6"]);
        assert_eq!(split_uci_list("  aa:bb:cc:dd:ee:ff  "), vec!["aa:bb:cc:dd:ee:ff"]);
        assert!(split_uci_list("   ").is_empty());
    }

    #[test]
    fn validators_match_shell_rules() {
        assert!(valid_ip_or_cidr("192.168.1.50"));
        assert!(valid_ip_or_cidr("10.0.0.0/8"));
        assert!(!valid_ip_or_cidr("999.1.1.1"));
        assert!(!valid_ip_or_cidr("1.2.3.4;rm -rf /"));
        assert!(!valid_ip_or_cidr("1.2.3.4/33"));
        assert!(!valid_ip_or_cidr("1.2.3.4/"));
        assert!(valid_ip6_or_cidr("2001:db8::1"));
        assert!(valid_ip6_or_cidr("fd00::/8"));
        assert!(!valid_ip6_or_cidr("fd00::/129"));
        assert!(!valid_ip6_or_cidr("192.168.1.1"));
        assert!(!valid_ip6_or_cidr("fd00::/"));
        assert!(valid_mac("aa:bb:cc:dd:ee:ff"));
        assert!(valid_mac("AA:BB:CC:DD:EE:FF"));
        assert!(!valid_mac("zz:bb:cc:dd:ee:ff"));
        assert!(!valid_mac("aa:bb:cc:dd:ee"));
        assert!(valid_port("25"));
        assert!(valid_port("65535"));
        assert!(!valid_port("0"));
        assert!(!valid_port("99999"));
        assert!(!valid_port("abc"));
    }

    #[test]
    fn payload_roundtrip_and_normalization() {
        let acl = parse_payload(&json!({
            "mode": "block",
            "ips": ["192.168.1.50", "192.168.1.50", " "],
            "ips6": ["fd00::/64", "fd00::/64"],
            "macs": ["AA:BB:CC:DD:EE:FF"],
            "bypassCidrs": ["10.10.0.0/16"],
            "bypassCidrs6": ["2001:db8::/32"],
            "bypassPorts": ["25"],
            "localTakeover": false
        }))
        .unwrap();
        assert_eq!(acl.ips, vec!["192.168.1.50"]);
        assert_eq!(acl.ips6, vec!["fd00::/64"]);
        assert_eq!(acl.bypass_cidrs6, vec!["2001:db8::/32"]);
        assert_eq!(acl.macs, vec!["aa:bb:cc:dd:ee:ff"]);
        assert!(!acl.local_takeover);
        assert_eq!(acl.to_json()["mode"], "block");
    }

    #[test]
    fn rejects_bad_payloads() {
        assert!(parse_payload(&json!({"mode": "nope"})).is_err());
        assert!(parse_payload(&json!({"mode": "allow"})).is_err());
        assert!(parse_payload(&json!({"ips": ["1.2.3.4$(reboot)"]})).is_err());
        assert!(parse_payload(&json!({"ips6": ["fd00::/129"]})).is_err());
        assert!(parse_payload(&json!({"bypassPorts": ["70000"]})).is_err());
        assert!(parse_payload(&json!({"mode": "allow", "ips": ["192.168.1.5"]})).is_ok());
        assert!(parse_payload(&json!({"mode": "allow", "ips6": ["fd00::1"]})).is_ok());
    }
}