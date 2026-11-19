//! Background loops: traffic/log samplers, totals polling, transparent-proxy
//! reconciliation and subscription auto-update. The outbound IP profile
//! lookup lives in its own module (`ipinfo.rs`).
//!
//! Every sampler is the single producer for its topic: REST readers and
//! WebSocket subscribers consume the same snapshot/event, so N browser tabs
//! no longer mean N controller polls.

use std::time::Duration;
use tokio::io::AsyncBufReadExt;

use crate::ctrl::ctrl_json;
use crate::state::{now_secs, AppState, LogEntry, MAX_HISTORY, MAX_LOGS};

fn controller_url(controller: &str, path: &str) -> String {
    format!("http://{}{}", controller, path)
}

/// Recomputes `/status` and publishes it when it actually changed. This is
/// the authoritative status source for both `GET /status` and WS `status`.
pub async fn refresh_snapshot(st: &AppState) {
    let value = crate::api::status_payload(st).await;
    st.publish_snapshot(value);
}

pub async fn sample_status(st: std::sync::Arc<AppState>) {
    loop {
        let value = crate::api::status_payload(&st).await;
        if *st.snapshot() == value {
            // Same bytes: keep the REST cache fresh without emitting a redundant
            // event (clients already hold this state).
            st.touch_snapshot();
        } else {
            st.publish_snapshot(value);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// `/proxies` snapshots for the proxies page, only while someone is listening.
pub async fn sample_proxies(st: std::sync::Arc<AppState>) {
    loop {
        if st.bus.receivers() > 0 {
            if let Ok(mut payload) = crate::api::proxies_payload(&st).await {
                payload["running"] = serde_json::json!(true);
                st.bus.publish("proxies", "proxies", payload);
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// `/connections` snapshots (dashboard table + connections page).
pub async fn sample_connections(st: std::sync::Arc<AppState>) {
    loop {
        if st.bus.receivers() > 0 {
            let payload = crate::api::connections_payload(&st).await;
            st.bus.publish("connections", "connections", payload);
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
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
            let snapshot = {
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
                serde_json::json!({
                    "up": hist.up.last().copied().unwrap_or(0.0),
                    "down": hist.down.last().copied().unwrap_or(0.0),
                    "upRate": hist.up.last().copied().unwrap_or(0.0),
                    "downRate": hist.down.last().copied().unwrap_or(0.0),
                    "peakUp": hist.peak_up,
                    "peakDown": hist.peak_down,
                    "totalUp": hist.total_up,
                    "totalDown": hist.total_dn,
                    "history": {"up": hist.up.clone(), "down": hist.down.clone()},
                })
            };
            // Live chart data without waiting for the next status snapshot.
            st.bus.publish("traffic", "traffic", snapshot);
        }
        let _ = child.kill().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// /logs?level=info → ring buffer for the logs page + batched WS pushes.
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
        let mut pending: Vec<serde_json::Value> = Vec::new();
        let mut ticker = tokio::time::interval(Duration::from_millis(300));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                    let payload = value.get("payload").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if payload.is_empty() { continue; }
                    let level = value.get("type").and_then(|v| v.as_str()).unwrap_or("info").to_string();
                    let entry = LogEntry { time: now_secs(), level: level.clone(), message: payload.clone() };
                    {
                        let mut hist = st.hist.lock().unwrap();
                        hist.logs.push(entry);
                        if hist.logs.len() > MAX_LOGS {
                            let excess = hist.logs.len() - MAX_LOGS;
                            hist.logs.drain(..excess);
                        }
                    }
                    pending.push(serde_json::json!({"time": now_secs(), "level": level, "message": payload}));
                }
                _ = ticker.tick() => {
                    if !pending.is_empty() {
                        let batch = std::mem::take(&mut pending);
                        st.bus.publish("logs", "logs.append", serde_json::json!({"lines": batch}));
                    }
                }
            }
        }
        if !pending.is_empty() {
            let batch = std::mem::take(&mut pending);
            st.bus.publish("logs", "logs.append", serde_json::json!({"lines": batch}));
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
