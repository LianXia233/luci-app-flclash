//! Mixin & config validation (Nikki-style `mixin.yaml` deep merge).
//!
//! Data flow kept explicit:
//!   raw subscription ──convert──► profile yaml ──mixin──► config.yaml ──► core
//!
//! The mixin lives at `<data>/mixin.yaml`. Merge rules:
//!   * mappings merge recursively (unknown keys are preserved);
//!   * `rules`, `rule-providers` prepend the mixin entries so user rules win;
//!   * other sequences replace the base value (mihomo semantics);
//!   * `null` deletes a key.
//!
//! Validation is structural, not a full mihomo schema: required top-level
//! shape, group membership, rule syntax, port ranges and duplicate names.

use std::path::Path;

use serde_json::{json, Value};

pub fn mixin_path(data_dir: &str) -> std::path::PathBuf {
    Path::new(data_dir).join("mixin.yaml")
}

pub fn load_mixin(data_dir: &str) -> Result<Option<Value>, String> {
    let path = mixin_path(data_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Ok(None),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let doc: Value = serde_yaml::from_str(&text).map_err(|e| format!("mixin.yaml 解析失败: {}", e))?;
    if !doc.is_object() {
        return Err("mixin.yaml 顶层必须是映射".into());
    }
    Ok(Some(doc))
}

pub fn save_mixin(data_dir: &str, text: &str) -> Result<(), String> {
    if !text.trim().is_empty() {
        let doc: Value = serde_yaml::from_str(text).map_err(|e| format!("YAML 语法错误: {}", e))?;
        if !doc.is_object() {
            return Err("顶层必须是映射（例如 mode: rule）".into());
        }
    }
    crate::config::write_atomic(&mixin_path(data_dir), text.as_bytes())
}

/// Recursively merges `overlay` into `base`. `rules` / `rule-providers`
/// entries from the overlay come first; other arrays replace.
pub fn deep_merge(base: &mut Value, overlay: &Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(over_map)) => {
            for (key, value) in over_map {
                if value.is_null() {
                    base_map.shift_remove(key);
                    continue;
                }
                if key == "rules" || key == "rule-providers" {
                    prepend_entries(base_map, key, value);
                    continue;
                }
                match base_map.get_mut(key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base_map.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base_slot, overlay_value) => *base_slot = overlay_value.clone(),
    }
}

fn prepend_entries(base_map: &mut serde_json::Map<String, Value>, key: &str, value: &Value) {
    let existing = base_map.get(key).cloned();
    match (existing, value) {
        (Some(Value::Array(mut base_list)), Value::Array(over_list)) => {
            let mut merged = over_list.clone();
            for item in base_list.drain(..) {
                if !merged.contains(&item) {
                    merged.push(item);
                }
            }
            base_map.insert(key.to_string(), Value::Array(merged));
        }
        (Some(Value::Object(mut base_map2)), Value::Object(over_map)) => {
            for (k, v) in over_map {
                base_map2.insert(k.clone(), v.clone());
            }
            base_map.insert(key.to_string(), Value::Object(base_map2));
        }
        _ => {
            base_map.insert(key.to_string(), value.clone());
        }
    }
}

/// Applies the stored mixin (if any) to a converted profile YAML.
pub fn apply_mixin(data_dir: &str, yaml: &str) -> Result<(String, Vec<String>), String> {
    let Some(mixin) = load_mixin(data_dir)? else {
        return Ok((yaml.to_string(), Vec::new()));
    };
    let mut doc: Value = serde_yaml::from_str(yaml).map_err(|e| format!("配置解析失败: {}", e))?;
    let mut touched: Vec<String> = mixin
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    touched.sort();
    deep_merge(&mut doc, &mixin);
    let out = serde_yaml::to_string(&doc).map_err(|e| format!("配置序列化失败: {}", e))?;
    Ok((out, touched))
}

/// Structural validation used by both the editor preview and apply.
/// Returns `(errors, warnings)`; an empty error list means applicable.
pub fn validate(yaml: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let doc: Value = match serde_yaml::from_str(yaml) {
        Ok(doc) => doc,
        Err(err) => {
            errors.push(format!("YAML 语法错误: {}", err));
            return (errors, warnings);
        }
    };
    let Some(map) = doc.as_object() else {
        errors.push("顶层必须是映射".into());
        return (errors, warnings);
    };

    for key in ["port", "socks-port", "mixed-port", "redir-port", "tproxy-port"] {
        if let Some(value) = map.get(key) {
            match value.as_i64() {
                Some(port) if (1..=65535).contains(&port) => {}
                Some(port) => errors.push(format!("{} 端口超出范围: {}", key, port)),
                None => errors.push(format!("{} 必须是整数", key)),
            }
        }
    }
    if let Some(mode) = map.get("mode").and_then(|v| v.as_str()) {
        if !["rule", "global", "direct"].contains(&mode) {
            errors.push(format!("mode 只能是 rule/global/direct，当前: {}", mode));
        }
    }

    let proxies = map.get("proxies").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    if proxies.is_empty() {
        warnings.push("没有 proxies 段：仅作为规则/直连配置可用".into());
    }
    let mut proxy_names: Vec<String> = Vec::new();
    for (index, item) in proxies.iter().enumerate() {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            errors.push(format!("proxies[{}] 缺少 name", index));
        } else if proxy_names.contains(&name.to_string()) {
            errors.push(format!("节点名重复: {}", name));
        } else {
            proxy_names.push(name.to_string());
        }
        if item.get("type").and_then(|v| v.as_str()).unwrap_or("").is_empty() {
            errors.push(format!("节点「{}」缺少 type", name));
        }
    }

    let groups = map.get("proxy-groups").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let mut group_names: Vec<String> = Vec::new();
    for (index, item) in groups.iter().enumerate() {
        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            errors.push(format!("proxy-groups[{}] 缺少 name", index));
        } else if group_names.contains(&name.to_string()) {
            errors.push(format!("策略组名重复: {}", name));
        } else {
            group_names.push(name.to_string());
        }
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind.is_empty() {
            errors.push(format!("策略组「{}」缺少 type", name));
        }
        if let Some(members) = item.get("proxies").and_then(|v| v.as_array()) {
            for member in members {
                let Some(member) = member.as_str() else { continue };
                if member == "DIRECT" || member == "REJECT" || member == "REJECT-DROP" || member == "PASS" {
                    continue;
                }
                if !proxy_names.iter().any(|n| n == member) && !group_names.iter().any(|n| n == member) {
                    // Forward references to later groups are legal; only flag
                    // names that exist nowhere in the document.
                    let later = groups.iter().skip(index + 1).any(|g| {
                        g.get("name").and_then(|v| v.as_str()) == Some(member)
                    });
                    if !later {
                        errors.push(format!("策略组「{}」引用了不存在的节点/组: {}", name, member));
                    }
                }
            }
        }
    }

    if let Some(rules) = map.get("rules").and_then(|v| v.as_array()) {
        for (index, rule) in rules.iter().enumerate() {
            let Some(text) = rule.as_str() else {
                errors.push(format!("rules[{}] 必须是字符串", index));
                continue;
            };
            let parts: Vec<&str> = text.split(',').map(|s| s.trim()).collect();
            if parts.len() < 2 {
                errors.push(format!("规则格式错误: {}", text));
                continue;
            }
            let target = parts[parts.len() - 1];
            let known = ["DIRECT", "REJECT", "REJECT-DROP", "PASS"].contains(&target)
                || proxy_names.iter().any(|n| n == target)
                || group_names.iter().any(|n| n == target);
            if !known {
                errors.push(format!("规则指向不存在的出口「{}」: {}", target, text));
            }
        }
    }
    (errors, warnings)
}

pub fn validate_payload(yaml: &str) -> Value {
    let (errors, warnings) = validate(yaml);
    json!({
        "ok": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_maps_and_prepends_rules() {
        let mut base: Value = serde_yaml::from_str(
            "mode: rule\ndns:\n  enable: true\n  nameserver: [1.1.1.1]\nrules:\n  - MATCH,DIRECT\n",
        )
        .unwrap();
        let overlay: Value = serde_yaml::from_str(
            "mode: global\ndns:\n  ipv6: true\nrules:\n  - DOMAIN-SUFFIX,example.com,Proxy\n",
        )
        .unwrap();
        deep_merge(&mut base, &overlay);
        assert_eq!(base["mode"], "global");
        assert_eq!(base["dns"]["enable"], true);
        assert_eq!(base["dns"]["ipv6"], true);
        assert_eq!(base["dns"]["nameserver"][0], "1.1.1.1");
        assert_eq!(base["rules"][0], "DOMAIN-SUFFIX,example.com,Proxy");
        assert_eq!(base["rules"][1], "MATCH,DIRECT");
    }

    #[test]
    fn null_removes_key_and_arrays_replace() {
        let mut base: Value = serde_yaml::from_str("tun:\n  enable: true\n  mtu: 9000\n").unwrap();
        let overlay: Value = serde_yaml::from_str("tun:\n  mtu: null\n  stack: system\n").unwrap();
        deep_merge(&mut base, &overlay);
        assert!(base["tun"].get("mtu").is_none());
        assert_eq!(base["tun"]["stack"], "system");
    }

    #[test]
    fn validation_catches_bad_members_and_ports() {
        let yaml = "mixed-port: 70000\nmode: weird\nproxies:\n  - name: A\n    type: ss\nproxy-groups:\n  - name: G\n    type: select\n    proxies: [B]\nrules:\n  - MATCH,G\n";
        let (errors, _) = validate(yaml);
        assert!(errors.iter().any(|e| e.contains("端口")));
        assert!(errors.iter().any(|e| e.contains("mode")));
        assert!(errors.iter().any(|e| e.contains("不存在的节点")));
        let ok = "mixed-port: 7892\nproxies:\n  - name: A\n    type: ss\nproxy-groups:\n  - name: G\n    type: select\n    proxies: [A]\nrules:\n  - MATCH,G\n";
        assert!(validate(ok).0.is_empty());
    }
}
