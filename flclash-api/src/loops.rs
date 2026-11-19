//! Background loops: traffic/log samplers, totals polling, transparent-proxy
//! reconciliation, subscription auto-update and the outbound IP cache. Long
//! streams use `curl -N` (newline-delimited JSON from the controller); the
//! rest uses the hand-rolled loopback client.

use std::time::Duration;
use tokio::io::AsyncBufReadExt;

use crate::ctrl::{ctrl_json, curl_json};
use crate::state::{now_secs, AppState, LogEntry, MAX_HISTORY, MAX_LOGS};

fn controller_url(controller: &str, path: &str) -> String {
    format!("http://{}{}", controller, path)
}

/// /traffic → rate history for the dashboard charts.
pub async fn sample_traffic(st: std::sync::Arc<AppState>) {
    loop {
        let mut cmd = tokio::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-N",
            "-m",
            "86400",
            &controller_url(&st.controller, "/traffic"),
        ]);
        if let Some(secret) = &st.secret {
            cmd.arg("-H").arg(format!("Authorization: Bearer {}", secret));
        }
        let Ok(mut child) = cmd.stdout(std::process::Stdio::piped()).spawn() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let Some(stdout) = child.stdout.take() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let up = value.get("up").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let down = value.get("down").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let mut hist = st.hist.lock().unwrap();
            hist.up.push(up);
            hist.down.push(down);
            if hist.up.len() > MAX_HISTORY {
                let excess = hist.up.len() - MAX_HISTORY;
                hist.up.drain(..excess);
                hist.down.drain(..excess);
            }
            if up > hist.peak_up {
                hist.peak_up = up;
            }
            if down > hist.peak_down {
                hist.peak_down = down;
            }
        }
        let _ = child.kill().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// /logs?level=info → ring buffer for the logs page.
pub async fn sample_logs(st: std::sync::Arc<AppState>) {
    loop {
        let mut cmd = tokio::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-N",
            "-m",
            "86400",
            &controller_url(&st.controller, "/logs?level=info"),
        ]);
        if let Some(secret) = &st.secret {
            cmd.arg("-H").arg(format!("Authorization: Bearer {}", secret));
        }
        let Ok(mut child) = cmd.stdout(std::process::Stdio::piped()).spawn() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let Some(stdout) = child.stdout.take() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        let reader = tokio::io::BufReader::new(stdout);
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            let payload = value
                .get("payload")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if payload.is_empty() {
                continue;
            }
            let level = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("info")
                .to_string();
            let mut hist = st.hist.lock().unwrap();
            hist.logs.push(LogEntry {
                time: now_secs(),
                level,
                message: payload,
            });
            if hist.logs.len() > MAX_LOGS {
                let excess = hist.logs.len() - MAX_LOGS;
                hist.logs.drain(..excess);
            }
        }
        let _ = child.kill().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// /connections totals for the dashboard counters.
pub async fn sample_totals(st: std::sync::Arc<AppState>) {
    loop {
        if let Ok(value) = ctrl_json(&st, "GET", "/connections", None, Duration::from_secs(5)).await {
            let up = value.get("uploadTotal").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let down = value.get("downloadTotal").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let mut hist = st.hist.lock().unwrap();
            hist.total_up = up;
            hist.total_dn = down;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// Reconcile transparent proxy after firewall restarts, core crashes and IPC
/// reloads. The shell state machine is idempotent and repairs partial chains.
pub async fn sync_proxy(_st: std::sync::Arc<AppState>) {
    loop {
        let _ = tokio::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
            .arg("sync")
            .output()
            .await;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Subscription auto-update: refreshes URL profiles past their interval and
/// re-applies the active one.
pub async fn auto_update(st: std::sync::Arc<AppState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let list = crate::profiles::load_profiles(&st);
        let now = now_secs();
        for item in list {
            if item.url.is_empty() || item.interval < 60 {
                continue;
            }
            if now - item.updated_at < item.interval {
                continue;
            }
            if let Err(err) = crate::profiles::update_profile(&st, &item.id).await {
                eprintln!("auto-update {}: {}", item.id, err);
            }
        }
    }
}

/// Caches the outbound IP profile (country/city/ISP/ASN + quality flags).
pub async fn refresh_outbound(st: std::sync::Arc<AppState>) {
    loop {
        let mut info = crate::state::IpInfo::default();
        let mut ok = false;
        if let Ok(v) = curl_json::<serde_json::Value>(
            "http://ip-api.com/json/?fields=status,message,query,country,countryCode,city,isp,org,as,proxy,hosting,mobile&lang=zh-CN",
            8,
        )
        .await
        {
            info.ip = v.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.country = v.get("country").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.country_code = v.get("countryCode").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.city = v.get("city").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.isp = v.get("isp").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.org = v.get("org").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.asn = v.get("as").and_then(|v| v.as_str()).unwrap_or("").to_string();
            info.proxy = v.get("proxy").and_then(|v| v.as_bool()).unwrap_or(false);
            info.hosting = v.get("hosting").and_then(|v| v.as_bool()).unwrap_or(false);
            info.mobile = v.get("mobile").and_then(|v| v.as_bool()).unwrap_or(false);
            ok = !info.ip.is_empty();
        }
        if !ok {
            if let Ok(v) = curl_json::<serde_json::Value>("https://api.ip.sb/geoip", 8).await {
                info.ip = v.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.country = v.get("country").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.country_code = v
                    .get("country_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                info.city = v.get("city").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.isp = v.get("isp").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.org = v
                    .get("asn_organization")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                info.asn = v.get("asn").map(|a| format!("AS{}", a)).unwrap_or_default();
                ok = !info.ip.is_empty();
            }
        }
        {
            let mut hist = st.hist.lock().unwrap();
            if ok {
                hist.outbound_ip = info.ip.clone();
                hist.outbound_cc = info.country_code.clone();
                hist.outbound_org = if info.isp.is_empty() { info.org.clone() } else { info.isp.clone() };
                if hist.outbound_org.is_empty() {
                    hist.outbound_org = "未知".into();
                }
                hist.outbound_ok = true;
                hist.ipinfo = Some(info);
            } else {
                hist.outbound_ok = false;
                hist.outbound_org = "查询失败".into();
            }
        }
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
}
