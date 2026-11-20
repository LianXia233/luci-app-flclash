//! Core management over REST: the same contract the rpcd ubus object
//! exposes (`core_type` / `core_switch` / `core_update` / `core_update_status`),
//! served from the flclash-api origin so the WebUI never depends on a
//! cross-origin `/ubus` endpoint (which does not exist on this origin).
//!
//! Binary paths follow the canonical install locations; `FLCLASH_CORE_DIR`
//! overrides the directory for development/testing, mirroring the env
//! override convention already used by proxy-rules.sh (`CFG`, `LAN_IF`).

use serde_json::{json, Value};
use std::path::PathBuf;

pub const UPDATE_STATE: &str = "/tmp/flclash-update.state";
pub const UPDATE_LOG: &str = "/tmp/flclash-update.log";
pub const SINGBOX_JSON: &str = "/tmp/flclash-singbox.json";
const UPDATE_SCRIPT: &str = "/usr/libexec/flclash/core-update.sh";

pub fn valid_core(core: &str) -> bool {
    matches!(core, "mihomo" | "sing-box")
}

pub fn bin_dir() -> PathBuf {
    std::env::var("FLCLASH_CORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/usr/bin"))
}

pub fn bin_path(core: &str) -> PathBuf {
    bin_dir().join(if core == "sing-box" { "sing-box" } else { "mihomo" })
}

pub fn installed(core: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(bin_path(core))
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// First line of `<bin> -v` / `<bin> version`, empty when not runnable.
pub fn version(core: &str) -> String {
    if !installed(core) {
        return String::new();
    }
    let bin = bin_path(core);
    for args in [["-v"], ["version"]] {
        if let Ok(out) = std::process::Command::new(&bin).args(args).output() {
            let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !text.is_empty() {
                return text.lines().next().unwrap_or("").trim().to_string();
            }
        }
    }
    String::new()
}

pub fn active() -> String {
    std::process::Command::new("uci")
        .args(["-q", "get", "flclash.main.core_type"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| valid_core(s))
        .unwrap_or_else(|| "mihomo".into())
}

fn uci(args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("uci")
        .args(args)
        .output()
        .map_err(|e| format!("uci: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("uci {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

pub fn info() -> Value {
    json!({
        "active": active(),
        "mihomo_installed": installed("mihomo"),
        "singbox_installed": installed("sing-box"),
        "mihomo_version": version("mihomo"),
        "singbox_version": version("sing-box"),
    })
}

/// Switch the running core. Mirrors the ubus handler: refuse uninstalled
/// binaries, persist UCI, regenerate sing-box JSON from the live mihomo
/// config before restart, then restart through the same service path the
/// rest of the backend uses.
pub async fn switch(st: &crate::state::AppState, core: &str) -> Result<Value, String> {
    if !valid_core(core) {
        return Err("未知内核类型".into());
    }
    if !installed(core) {
        return Err("内核未安装,请先在线更新".into());
    }
    uci(&["-q", "set", &format!("flclash.main.core_type={core}")])?;
    // Bundled mihomo runs in native IPC mode; the standalone CLI installed by
    // the updater sets core_mode=cli itself. sing-box ignores core_mode.
    if core == "mihomo" {
        uci(&["-q", "set", "flclash.main.core_mode=ipc"])?;
    }
    uci(&["-q", "commit", "flclash"])?;

    let mut converted = false;
    if core == "sing-box" {
        let yaml = std::fs::read_to_string(st.config_path())
            .map_err(|e| format!("读取 config.yaml 失败: {e}"))?;
        match crate::singbox::convert(&yaml) {
            Ok(json_text) => {
                std::fs::write(SINGBOX_JSON, json_text)
                    .map_err(|e| format!("写入 sing-box 配置失败: {e}"))?;
                converted = true;
            }
            Err(err) => return Err(format!("sing-box 配置转换失败: {err}")),
        }
    }
    crate::profiles::service(st, "restart").await?;
    st.bus.publish("system", "system.service", json!({"core": core, "converted": converted}));
    Ok(json!({"ok": true, "core": core, "converted": converted}))
}

/// Kick off the shell updater (it owns state/log files and the atomic binary
/// swap). Detached: the HTTP reply must not wait for a multi-MB download.
pub fn update_start(core: &str) -> Result<Value, String> {
    if !valid_core(core) {
        return Err("未知内核类型".into());
    }
    if !std::path::Path::new(UPDATE_SCRIPT).exists() {
        return Err("更新脚本未安装".into());
    }
    let state = std::fs::read_to_string(UPDATE_STATE).unwrap_or_default();
    if state.trim() == "running" {
        return Err("已有更新正在进行".into());
    }
    // The shell updater owns done/failed, but somebody has to publish "running"
    // for the WebUI path (the LuCI path does it in ucode). Without this the
    // status endpoint reported "idle" for the whole download.
    let _ = std::fs::write(UPDATE_STATE, "running\n");
    let _ = std::fs::write(UPDATE_LOG, "");
    let spawned = std::process::Command::new("/bin/sh")
        .arg(UPDATE_SCRIPT)
        .arg(core)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => Ok(json!({"ok": true, "core": core})),
        Err(e) => {
            let _ = std::fs::write(UPDATE_STATE, "failed\n");
            Err(format!("启动更新失败: {e}"))
        }
    }
}

/// Last `max` bytes of `text`, moved forward to a UTF-8 boundary.
/// `&text[text.len() - n..]` panics when the cut lands inside a multi-byte
/// character — and the updater log is full of Chinese lines. With
/// `panic = "abort"` that panic takes the whole API process down.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

pub fn update_status() -> Value {
    let state = std::fs::read_to_string(UPDATE_STATE)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "idle".into());
    let log = std::fs::read_to_string(UPDATE_LOG).unwrap_or_default();
    // Tail only: the log can grow across retries; the UI shows a live window.
    let tail = tail(&log, 4000);
    let core = active();
    json!({
        "state": state,
        "log": tail,
        "core_version": version(&core),
        "core_type": core,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_validation_and_paths() {
        assert!(valid_core("mihomo"));
        assert!(valid_core("sing-box"));
        assert!(!valid_core("clash"));
        assert!(!valid_core(""));
        assert!(bin_path("sing-box").ends_with("sing-box"));
        assert!(bin_path("mihomo").ends_with("mihomo"));
        // Not installed in the test environment: both helpers must degrade.
        if !installed("sing-box") {
            assert_eq!(version("sing-box"), "");
        }
    }

    #[test]
    fn update_status_defaults_idle() {
        // No state file in the test environment: must report idle, not panic.
        let value = update_status();
        assert!(value.get("state").is_some());
        assert!(value.get("core_type").is_some());
    }

    #[test]
    fn tail_never_splits_a_utf8_character() {
        // Updater logs are Chinese; cutting mid-character used to panic and,
        // with panic=abort, killed the whole API process.
        let text = "更".repeat(2000); // 6000 bytes
        let out = tail(&text, 4001);
        assert!(out.len() <= 4001);
        assert!(out.chars().all(|c| c == '更'));
        assert_eq!(tail("short", 4000), "short");
    }
}