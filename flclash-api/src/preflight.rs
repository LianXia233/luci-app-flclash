//! Core-level configuration preflight.
//!
//! The bundled mihomo is the FlClash native-IPC core: `cmd/core/main.go` treats
//! `argv[1]` as the IPC socket path, so `mihomo -t` is NOT available for it.
//! We validate through `flclash-core-bridge --validate` instead (core.init +
//! config.setup against a sanitised copy, without listener.start). Standalone
//! CLI mihomo uses `-t`; sing-box uses `check -c`.
//!
//! A failing preflight must never replace the live config: callers write a
//! candidate, validate it, then rename. `FLCLASH_SKIP_PREFLIGHT=1` exists for
//! development only.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};


fn uci_get(key: &str) -> String {
    Command::new("uci")
        .args(["-q", "get", key])
        .output()
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

fn tail(text: &[u8], max: usize) -> String {
    let text = String::from_utf8_lossy(text);
    if text.len() <= max {
        return text.trim().to_string();
    }
    let mut start = text.len() - max;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].trim().to_string()
}

fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|e| format!("wait: {e}"));
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {}s", timeout.as_secs()));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

fn check_output(label: &str, out: Output) -> Result<(), String> {
    if out.status.success() {
        return Ok(());
    }
    let detail = {
        let stderr = tail(&out.stderr, 4096);
        if stderr.is_empty() {
            tail(&out.stdout, 4096)
        } else {
            stderr
        }
    };
    if detail.is_empty() {
        Err(format!("{label}: exit {}", out.status))
    } else {
        Err(format!("{label}: {detail}"))
    }
}

/// Validates a Clash YAML by converting it to the sing-box config that the
/// init script will actually run, then `sing-box check` that JSON. Running
/// `sing-box check -c config.yaml` directly (the previous behaviour) always
/// failed, which made every settings save / profile apply impossible while
/// the sing-box core was selected.
pub fn check_singbox_yaml(yaml: &str) -> Result<(), String> {
    if std::env::var("FLCLASH_SKIP_PREFLIGHT").map(|v| v == "1").unwrap_or(false) {
        return Ok(());
    }
    let json = crate::singbox::convert(yaml)?;
    check_singbox_json(&json)
}

/// Validates an already-converted sing-box JSON document.
pub fn check_singbox_json(json: &str) -> Result<(), String> {
    if std::env::var("FLCLASH_SKIP_PREFLIGHT").map(|v| v == "1").unwrap_or(false) {
        return Ok(());
    }
    let bin = crate::coremgmt::bin_path("sing-box");
    if !bin.is_file() {
        return Err("sing-box 未安装，无法预检".into());
    }
    // Keep the probe on the same filesystem as its temporary name so the
    // removal is atomic and never leaves a half-written file behind.
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir();
    let path = dir.join(format!(
        "flclash-singbox-check-{}-{}.json",
        std::process::id(),
        seq
    ));
    crate::config::write_atomic(&path, json.as_bytes())?;
    let mut cmd = Command::new(bin);
    cmd.args(["check", "-c"]).arg(&path);
    // Configs produced by older converters may still carry the legacy
    // rule-set download_detour field; allow it for the duration of the probe.
    cmd.env("ENABLE_DEPRECATED_LEGACY_RULE_SET_DOWNLOAD_DETOUR", "true");
    let result = run_with_timeout(cmd, Duration::from_secs(20))
        .and_then(|out| check_output("sing-box check", out));
    let _ = std::fs::remove_file(&path);
    result
}

/// Validates `path` with the configured core. Returns a user-readable error
/// (already trimmed) on failure. Never touches the live config.
pub fn validate_config_blocking(path: &Path) -> Result<(), String> {
    if std::env::var("FLCLASH_SKIP_PREFLIGHT").map(|v| v == "1").unwrap_or(false) {
        return Ok(());
    }
    if !path.is_file() {
        return Err(format!("配置不存在: {}", path.display()));
    }
    let home = path.parent().unwrap_or_else(|| Path::new("/etc/flclash"));
    let core = crate::core::configured_core();
    if core == "sing-box" {
        let yaml = std::fs::read_to_string(path)
            .map_err(|e| format!("读取配置失败: {e}"))?;
        return check_singbox_yaml(&yaml);
    }

    let bin = crate::coremgmt::bin_path("mihomo");
    if !bin.is_file() {
        return Err("mihomo 未安装，无法预检".into());
    }
    let core_mode = uci_get("flclash.main.core_mode");
    if core_mode != "cli" {
        let bridge = Path::new("/usr/bin/flclash-core-bridge");
        if bridge.is_file() {
            let mut cmd = Command::new(bridge);
            cmd.arg("--validate")
                .arg(path)
                .arg("-core")
                .arg(&bin)
                .arg("-home-dir")
                .arg(home);
            return check_output(
                "内核预检",
                run_with_timeout(cmd, Duration::from_secs(45))?,
            );
        }
    }

    let mut cmd = Command::new(bin);
    cmd.arg("-t").arg("-d").arg(home).arg("-f").arg(path);
    check_output("mihomo -t", run_with_timeout(cmd, Duration::from_secs(20))?)
}

/// Async wrapper: a preflight boots a second core (IPC mode) or parses a large
/// YAML, so it must not run on a Tokio worker.
pub async fn validate_config(path: std::path::PathBuf) -> Result<(), String> {
    tokio::task::spawn_blocking(move || validate_config_blocking(&path))
        .await
        .map_err(|e| format!("preflight task: {e}"))?
}
