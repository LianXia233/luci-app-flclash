//! Background loops: traffic/log samplers, totals polling, transparent-proxy
//! reconciliation and subscription auto-update. The outbound IP profile
//! lookup lives in its own module (`ipinfo.rs`).
//!
//! Every sampler is the single producer for its topic: REST readers and
//! WebSocket subscribers consume the same snapshot/event, so N browser tabs
//! no longer mean N controller polls.

use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;

use crate::ctrl::ctrl_json;
use crate::state::{now_secs, AppState, DnsQueryEntry, LogEntry, MAX_HISTORY, MAX_LOGS};

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

/// `/connections` is the single source for the connection list, the dashboard
/// counter and the traffic totals. It runs even without WebSocket clients
/// because /status reads the cached count instead of re-querying the core.
pub async fn sample_connections(st: std::sync::Arc<AppState>) {
    loop {
        if let Some(raw) = crate::api::connections_snapshot(&st).await {
            let payload = serde_json::Value::Array(crate::api::connections_from(&raw));
            let up = raw.get("uploadTotal").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let down = raw
                .get("downloadTotal")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let count = payload.as_array().map(|list| list.len()).unwrap_or(0);
            {
                let mut hist = st.hist.lock().unwrap();
                hist.total_up = up;
                hist.total_dn = down;
                hist.conn_count = count;
            }
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

// ---------------------------------------------------------------- DNS 查询日志

/// One observed mihomo DNS-debug line, already unescaped from the JSON log
/// envelope (`[DNS] …` payload).
pub(crate) enum DnsEvent {
    /// `resolve <domain> <TYPE> from <upstream>` — an upstream was asked.
    Query {
        domain: String,
        qtype: String,
        upstream: String,
    },
    /// `<domain> --> [ips] <TYPE> from <upstream>` — an upstream answered.
    Answer {
        domain: String,
        qtype: String,
        ips: String,
        upstream: String,
    },
    /// `cache hit <domain> --> [ips] <TYPE>` — served from the core cache.
    Hit {
        domain: String,
        qtype: String,
        ips: String,
    },
}

pub(crate) fn parse_dns_line(payload: &str) -> Option<DnsEvent> {
    let rest = payload.strip_prefix("[DNS] ")?;
    if let Some(rest) = rest.strip_prefix("cache hit ") {
        let (domain, ips, qtype) = parse_dns_target(rest)?;
        return Some(DnsEvent::Hit { domain, qtype, ips });
    }
    if let Some(rest) = rest.strip_prefix("resolve ") {
        // "domain TYPE from upstream" — domains have no spaces.
        let (head, upstream) = rest.split_once(" from ")?;
        let idx = head.rfind(' ')?;
        return Some(DnsEvent::Query {
            domain: head[..idx].trim().to_string(),
            qtype: head[idx + 1..].trim().to_string(),
            upstream: upstream.trim().to_string(),
        });
    }
    let (target, upstream) = rest.split_once(" from ")?;
    let (domain, ips, qtype) = parse_dns_target(target)?;
    Some(DnsEvent::Answer {
        domain,
        qtype,
        ips,
        upstream: upstream.trim().to_string(),
    })
}

/// `domain --> [ip1 ip2 …] TYPE` (TYPE may carry a trailing `, expire at …`).
fn parse_dns_target(rest: &str) -> Option<(String, String, String)> {
    let (head, tail) = rest.split_once(" --> [")?;
    let close = tail.find(']')?;
    let ips = tail[..close].trim().to_string();
    let after = tail[close + 1..].trim();
    let qtype = after.split(',').next().unwrap_or("").trim().to_string();
    Some((head.trim().to_string(), ips, qtype))
}

/// The egress path the DNS query itself used, derived from the upstream:
/// encrypted transports (DoH / DoT / DoQ, and gRPC or QUIC pools) are
/// reached through the proxy chain ("代理"), the system resolver is neither
/// ("其他"), plain UDP goes straight out WAN ("直连").
fn dns_origin(upstream: &str) -> &'static str {
    if upstream.starts_with("system(") {
        return "其他";
    }
    let scheme = upstream.split("://").next().unwrap_or("");
    if upstream.contains("/dns-query") || matches!(scheme, "https" | "h3" | "tls" | "quic" | "grpc") {
        "代理"
    } else {
        "直连"
    }
}

fn record_dns_entry(st: &AppState, entry: DnsQueryEntry) {
    const DNS_LOG_MAX: usize = 500;
    let mut log = st.dns_log.lock().unwrap();
    log.insert(0, entry);
    log.truncate(DNS_LOG_MAX);
}

fn apply_dns_event(st: &AppState, event: DnsEvent, pending: &mut std::collections::HashMap<String, i64>) {
    let now = crate::events::now_millis();
    match event {
        DnsEvent::Query { domain, qtype, upstream } => {
            if pending.len() > 400 {
                pending.clear();
            }
            let key = format!("{}|{}|{}", domain, qtype, upstream);
            pending.entry(key).or_insert(now);
        }
        DnsEvent::Answer { domain, qtype, ips, upstream } => {
            let key = format!("{}|{}|{}", domain, qtype, upstream);
            let started = pending.remove(&key).unwrap_or(now);
            let entry = DnsQueryEntry {
                time: now_secs(),
                domain,
                qtype,
                origin: dns_origin(&upstream).to_string(),
                result: ips.replace(' ', ", "),
                source: upstream,
                elapsed: (now - started).max(0),
                cached: false,
            };
            record_dns_entry(st, entry);
        }
        DnsEvent::Hit { domain, qtype, ips } => {
            // Inherit the origin from the last real query of this name; a
            // first-seen cache hit for a node server domain is a proxy-chain
            // resolution (proxy-server-nameserver / proxy dial).
            let origin = st
                .dns_log
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.domain == domain && e.qtype == qtype && !e.cached)
                .map(|e| e.origin.clone())
                .unwrap_or_else(|| {
                    if st.node_domains.lock().unwrap().contains(&domain) {
                        "代理".to_string()
                    } else {
                        "其他".to_string()
                    }
                });
            let entry = DnsQueryEntry {
                time: now_secs(),
                domain,
                qtype,
                origin,
                result: ips.replace(' ', ", "),
                source: String::new(),
                elapsed: 0,
                cached: true,
            };
            record_dns_entry(st, entry);
        }
    }
}

/// Node server domains from the live config.yaml (`server:` values inside the
/// proxies section). mihomo's /proxies payload does not expose `server`, so
/// the config file is the reliable source; refreshed lazily while the DNS
/// page is open so cache-hit rows can be tagged 代理 for node domains.
fn refresh_node_domains(st: &AppState) {
    let Ok(text) = std::fs::read_to_string(st.config_path()) else { return };
    let mut set = st.node_domains.lock().unwrap();
    set.clear();
    for line in text.lines() {
        // Match the `server:` KEY, not any substring: `nameserver:` and
        // `proxy-server-nameserver:` both contain "server:", which used to
        // feed DNS-resolver addresses into the node-domain set and mislabel
        // DNS servers' own queries as 代理 in the log.
        let mut scan = 0usize;
        let pos = loop {
            let found = match line[scan..].find("server:") {
                Some(p) => scan + p,
                None => break None,
            };
            if found == 0 || line.as_bytes()[found - 1] == b' ' || line.as_bytes()[found - 1] == b'\t' {
                break Some(found);
            }
            scan = found + 1;
        };
        let Some(pos) = pos else { continue };
        let rest = line[pos + 7..].trim();
        let end = rest.find(|c| c == ',' || c == '}').unwrap_or(rest.len());
        let server = rest[..end].trim().trim_matches('\'').trim_matches('"').to_string();
        if server.contains('.') && !server.contains(' ') && !server.contains(':') {
            set.insert(server);
        }
    }
    if set.len() > 5000 {
        set.clear();
    }
}

/// Streams mihomo's debug log and distils the `[DNS]` lines into the query
/// ring buffer. Debug mode logs *every* connection, so it is only enabled
/// while the DNS page polls /dns/queries (`dns_log_active`); otherwise the
/// core is patched back to its configured level.
pub async fn sample_dns_log(st: std::sync::Arc<AppState>) {
    let mut debug_on = false;
    let mut last_domains_refresh: i64 = 0;
    loop {
        let active = crate::events::now_millis() - st.dns_log_active.load(Ordering::Relaxed) < 15_000;
        if active {
            if crate::events::now_millis() - last_domains_refresh > 30_000 {
                refresh_node_domains(&st);
                last_domains_refresh = crate::events::now_millis();
            }
        }
        if active != debug_on {
            // Restore the level the user actually configured — a hardcoded
            // "info" silently overrode warning/error/debug set in settings
            // whenever the DNS page was closed.
            let level = if active {
                "debug".to_string()
            } else {
                crate::config::file_config(&st.config_path()).log_level
            };
            let body = serde_json::json!({ "log-level": level }).to_string();
            let _ = ctrl_json(&st, "PATCH", "/configs", Some(&body), Duration::from_secs(5)).await;
            debug_on = active;
        }
        if !active {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let mut cmd = tokio::process::Command::new("curl");
        cmd.args([
            "-sS",
            "-N",
            "-m",
            "86400",
            &controller_url(&st.controller, "/logs?level=debug"),
        ]);
        if let Some(secret) = &st.secret {
            cmd.arg("-H").arg(format!("Authorization: Bearer {}", secret));
        }
        let Ok(mut child) = cmd.stdout(std::process::Stdio::piped()).spawn() else {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        };
        if let Some(stdout) = child.stdout.take() {
            let reader = tokio::io::BufReader::new(stdout);
            let mut lines = reader.lines();
            let mut pending: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                let payload = value.get("payload").and_then(|v| v.as_str()).unwrap_or("");
                if !payload.starts_with("[DNS] ") {
                    continue;
                }
                if let Some(event) = parse_dns_line(payload) {
                    apply_dns_event(&st, event, &mut pending);
                }
                if crate::events::now_millis() - st.dns_log_active.load(Ordering::Relaxed) > 15_000 {
                    break; // page closed: drop the stream, the level flips back next tick
                }
            }
        }
        let _ = child.kill().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod dns_log_tests {
    use super::*;

    #[test]
    fn parse_dns_debug_lines() {
        match parse_dns_line(
            "[DNS] resolve node-02.example.net AAAA from https://203.0.113.10:8443/dns-query/abcd",
        ) {
            Some(DnsEvent::Query { domain, qtype, upstream }) => {
                assert_eq!(domain, "node-02.example.net");
                assert_eq!(qtype, "AAAA");
                assert_eq!(dns_origin(&upstream), "代理");
            }
            _ => panic!("resolve not parsed"),
        }
        match parse_dns_line("[DNS] edge.example.org --> [2001:db8::f15] AAAA from udp://8.8.4.4:53") {
            Some(DnsEvent::Answer { domain, ips, qtype, upstream }) => {
                assert_eq!(domain, "edge.example.org");
                assert_eq!(ips, "2001:db8::f15");
                assert_eq!(qtype, "AAAA");
                assert_eq!(dns_origin(&upstream), "直连");
            }
            _ => panic!("answer not parsed"),
        }
        match parse_dns_line(
            "[DNS] cache hit node-02.example.net --> [203.0.113.20 203.0.113.21] A, expire at 2026-11-20 18:34:08",
        ) {
            Some(DnsEvent::Hit { domain, qtype, ips }) => {
                assert_eq!(domain, "node-02.example.net");
                assert_eq!(qtype, "A");
                assert_eq!(ips, "203.0.113.20 203.0.113.21");
            }
            _ => panic!("hit not parsed"),
        }
        match parse_dns_line(
            "[DNS] resolve edge.example.org AAAA from system(udp://127.0.0.1:53,udp://[::1]:53)",
        ) {
            Some(DnsEvent::Query { upstream, .. }) => assert_eq!(dns_origin(&upstream), "其他"),
            _ => panic!("system query not parsed"),
        }
        assert!(parse_dns_line("[TCP] dial something").is_none());
        // empty AAAA answer still parses
        match parse_dns_line("[DNS] edge.example.net --> [] AAAA from udp://8.8.4.4:53") {
            Some(DnsEvent::Answer { ips, .. }) => assert_eq!(ips, ""),
            _ => panic!("empty answer not parsed"),
        }
        // encrypted transports classify as 代理 by their scheme alone, so a
        // resolver whose path is not /dns-query is still attributed correctly
        assert_eq!(dns_origin("tls://8.8.8.8"), "代理");
        assert_eq!(dns_origin("quic://dns.example.net"), "代理");
        assert_eq!(dns_origin("https://203.0.113.10:8443/resolver"), "代理");
        assert_eq!(dns_origin("udp://8.8.4.4:53"), "直连");
        assert_eq!(dns_origin("system(udp://127.0.0.1:53,udp://[::1]:53)"), "其他");
    }
}
