//! Profile store: import / update / apply / edit / delete plus the payload
//! rendering with real counters. All writes go through the conversion engine,
//! so every stored profile is core-loadable.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::config::write_atomic;
use crate::convert::{count_top_level_list, fetch_profile};
use crate::state::{human_age, human_bytes, now_secs, AppState};

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "type", default)]
    pub ptype: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default)]
    pub interval: i64,
    #[serde(default)]
    pub used: i64,
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub expire: i64,
    #[serde(default)]
    pub active: bool,
}

pub fn load_profiles(st: &AppState) -> Vec<Profile> {
    match std::fs::read(&st.index_path()) {
        Ok(data) => serde_json::from_slice(&data).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn save_profiles(st: &AppState, list: &[Profile]) -> Result<(), String> {
    std::fs::create_dir_all(st.profiles_dir())
        .map_err(|e| format!("mkdir profiles: {}", e))?;
    let data = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
    std::fs::write(st.index_path(), data).map_err(|e| e.to_string())
}

fn profile_file(st: &AppState, id: &str) -> PathBuf {
    st.profiles_dir().join(format!("{}.yaml", id))
}

/// Renders one profile payload with real counters from its file.
pub fn profile_payload(st: &AppState, item: &Profile) -> Value {
    let expire_days = if item.expire > 0 {
        let remaining = (item.expire - now_secs()) / 86400;
        remaining.max(0)
    } else {
        0
    };
    let (used_share, used, total) = if item.total > 0 {
        let share = (item.used as f64 / item.total as f64).clamp(0.0, 1.0);
        (share, human_bytes(item.used), human_bytes(item.total))
    } else {
        (0.0, "0 B".to_string(), "0 B".to_string())
    };
    let path = profile_file(st, &item.id);
    let (proxies, groups, rules) = match std::fs::read_to_string(&path) {
        Ok(text) => (
            count_top_level_list(&text, "proxies"),
            count_top_level_list(&text, "proxy-groups"),
            count_top_level_list(&text, "rules"),
        ),
        Err(_) => (0, 0, 0),
    };
    json!({
        "id": item.id,
        "name": item.name,
        "type": item.ptype,
        "url": item.url,
        "updatedAt": human_age(item.updated_at),
        "interval": item.interval,
        "active": item.active,
        "expireDays": expire_days,
        "usedShare": used_share,
        "used": used,
        "total": total,
        "proxies": proxies,
        "groups": groups,
        "rules": rules,
    })
}

/// Derives a profile name from its URL (mirrors the Go import).
fn name_from_url(url: &str) -> String {
    let without_query = url.split('?').next().unwrap_or(url);
    let base = Path::new(without_query)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let name = base
        .trim_end_matches(|c| c == '.')
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(base);
    if name.is_empty() {
        "订阅".to_string()
    } else {
        name.to_string()
    }
}

/// Downloads (via curl) and stores a new profile. The stored file is always
/// converted YAML, never the raw subscription body.
pub async fn import_profile(st: &AppState, url: &str) -> Result<Profile, String> {
    let (yaml, meta) = fetch_profile(&st.config_path(), url).await?;
    let id = format!("p{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0));

    std::fs::create_dir_all(st.profiles_dir())
        .map_err(|e| format!("mkdir profiles: {}", e))?;
    std::fs::write(profile_file(st, &id), &yaml)
        .map_err(|e| format!("write profile: {}", e))?;

    let item = Profile {
        id: id.clone(),
        name: name_from_url(url),
        ptype: "url".into(),
        url: url.to_string(),
        updated_at: now_secs(),
        interval: 0,
        used: meta.used,
        total: meta.total,
        expire: meta.expire,
        active: false,
    };
    let mut list = load_profiles(st);
    list.push(item.clone());
    save_profiles(st, &list)?;
    Ok(item)
}

/// Re-downloads a URL profile; re-applies it when active (core is reloaded
/// or started, whichever the situation needs).
pub async fn update_profile(st: &AppState, id: &str) -> Result<Value, String> {
    let list = load_profiles(st);
    let target = list
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("配置不存在: {}", id))?
        .clone();
    if target.url.is_empty() {
        return Err("该配置不是订阅,无法在线更新".into());
    }
    let (yaml, meta) = fetch_profile(&st.config_path(), &target.url).await?;
    std::fs::write(profile_file(st, id), &yaml)
        .map_err(|e| format!("write profile: {}", e))?;

    let mut list = load_profiles(st);
    let mut was_active = false;
    for item in list.iter_mut() {
        if item.id == id {
            item.updated_at = now_secs();
            item.used = meta.used;
            item.total = meta.total;
            item.expire = meta.expire;
            was_active = item.active;
        }
    }
    save_profiles(st, &list)?;

    if was_active {
        write_atomic(&st.config_path(), yaml.as_bytes())?;
        crate::profiles::reload_core(st).await;
    }
    Ok(json!({"id": id, "updatedAt": now_secs(), "used": meta.used, "total": meta.total, "expire": meta.expire}))
}

/// Edits profile metadata (name / url / auto-update interval in seconds).
pub fn patch_profile(st: &AppState, id: &str, patch: &Value) -> Result<(), String> {
    let mut list = load_profiles(st);
    let mut found = false;
    for item in list.iter_mut() {
        if item.id == id {
            found = true;
            if let Some(v) = patch.get("name").and_then(|v| v.as_str()) {
                let v = v.trim();
                if !v.is_empty() {
                    item.name = v.to_string();
                }
            }
            if let Some(v) = patch.get("url").and_then(|v| v.as_str()) {
                item.url = v.trim().to_string();
                if !item.url.is_empty() {
                    item.ptype = "url".into();
                }
            }
            if let Some(v) = patch.get("interval").and_then(|v| v.as_i64()) {
                item.interval = v.max(0);
            }
        }
    }
    if !found {
        return Err(format!("配置不存在: {}", id));
    }
    save_profiles(st, &list)
}

/// Activates a profile: converts (legacy raw profiles are healed here),
/// writes config.yaml atomically, then reloads or starts the core.
pub async fn apply_profile(st: &AppState, id: &str) -> Result<(), String> {
    let path = profile_file(st, id);
    let data = std::fs::read_to_string(&path)
        .map_err(|_| format!("配置不存在: {}", id))?;
    let final_yaml = crate::convert::convert_subscription(&st.config_path(), &data)
        .map_err(|e| format!("应用失败: {}", e))?;
    write_atomic(&st.config_path(), final_yaml.as_bytes())?;

    let mut list = load_profiles(st);
    for item in list.iter_mut() {
        item.active = item.id == id;
    }
    save_profiles(st, &list)?;

    reload_core(st).await;
    Ok(())
}

pub fn delete_profile(st: &AppState, id: &str) -> Result<(), String> {
    let list: Vec<Profile> = load_profiles(st)
        .into_iter()
        .filter(|item| item.id != id)
        .collect();
    save_profiles(st, &list)?;
    let _ = std::fs::remove_file(profile_file(st, id));
    Ok(())
}

/// Applies config.yaml to a running core, or starts the core. This is why
/// "启用配置" works while the proxy is stopped.
pub async fn reload_core(st: &AppState) {
    // Keep the sing-box config in sync with the freshly applied subscription.
    // Conversion is lossless (proxies/groups/rules/DNS/TUN) and cheap; the init
    // script re-runs it on core switch, this just removes the staleness window
    // after an import/update/apply so switching is instant.
    let cfg_path = st.config_path();
    if let Ok(yaml) = std::fs::read_to_string(&cfg_path) {
        if let Ok(json) = crate::singbox::convert(&yaml) {
            let _ = std::fs::write("/tmp/flclash-singbox.json", json);
        }
    }

    let reachable = crate::ctrl::ctrl_req(
        &st.controller,
        st.secret.as_deref(),
        "GET",
        "/version",
        None,
        std::time::Duration::from_secs(2),
    )
    .await;
    match reachable {
        Ok(reply) if reply.status < 500 => {
            let body = serde_json::json!({"path": st.config_path().display().to_string()});
            let put = crate::ctrl::ctrl_req(
                &st.controller,
                st.secret.as_deref(),
                "PUT",
                "/configs",
                Some(&body.to_string()),
                std::time::Duration::from_secs(15),
            )
            .await;
            if !matches!(&put, Ok(r) if r.status < 400) {
                let _ = service(st, "restart").await;
            }
        }
        _ => {
            let _ = service(st, "start").await;
        }
    }
}

/// Invokes the core init script (start/stop/restart).
pub async fn service(st: &AppState, action: &str) -> Result<(), String> {
    if !matches!(action, "start" | "stop" | "restart") {
        return Err(format!("未知操作: {}", action));
    }
    let output = tokio::process::Command::new(&st.init_script)
        .arg(action)
        .output()
        .await
        .map_err(|e| format!("{}: {}", action, e))?;
    if !output.status.success() {
        let text = format!(
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Err(format!("{}: {}", action, text.trim()));
    }
    Ok(())
}

/// Aggregated payload used by GET /profiles.
pub fn list_payload(st: &AppState) -> Value {
    let list = load_profiles(st);
    Value::Array(list.iter().map(|item| profile_payload(st, item)).collect())
}
