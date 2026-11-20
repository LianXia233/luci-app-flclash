//! HTTP handlers implementing the /flclash/api contract. Degradation rule:
//! reads never fail while the core is stopped (they fall back to file-backed
//! values or empty sets with 200), only core-dependent writes return a clear
//! 503 "内核未运行" message — no raw dial errors ever reach the UI.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde_json::{json, Value};

use crate::config::{file_config, save_config};
use crate::ctrl::{core_reachable, ctrl_json, ctrl_req, CtrlReply};
use crate::state::{core_pid, core_uptime, local_ipv4, rss_bytes, AppState};
use crate::profiles;

/// Controller request wrappers bound to the app state (timeout defaults).
async fn req(st: &AppState, method: &str, path: &str) -> Result<CtrlReply, String> {
    ctrl_req(&st.controller, st.secret.as_deref(), method, path, None, Duration::from_secs(10)).await
}

async fn req_body(st: &AppState, method: &str, path: &str, body: &str) -> Result<CtrlReply, String> {
    ctrl_req(&st.controller, st.secret.as_deref(), method, path, Some(body), Duration::from_secs(20)).await
}

// ------------------------------------------------------------- reply helpers

fn ok_json(value: Value) -> Response {
    ([(axum::http::header::CACHE_CONTROL, "no-store")], axum::Json(value)).into_response()
}

fn ok_raw(body: Vec<u8>) -> Response {
    (
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        (
            [(axum::http::header::CONTENT_TYPE, "application/json; charset=utf-8")],
            String::from_utf8_lossy(&body).into_owned(),
        ),
    )
        .into_response()
}

fn fail_json(status: StatusCode, err: &str) -> Response {
    (status, axum::Json(json!({"error": err}))).into_response()
}

fn bad_gateway(err: &str) -> Response {
    fail_json(StatusCode::BAD_GATEWAY, err)
}

fn core_down() -> Response {
    fail_json(StatusCode::SERVICE_UNAVAILABLE, "内核未运行，请先在 LuCI 或状态页启动 FlClash")
}

/// 202 for operations that continue after the HTTP response.
fn accepted(task_id: String) -> Response {
    (
        StatusCode::ACCEPTED,
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        axum::Json(json!({"ok": true, "accepted": true, "taskId": task_id})),
    )
        .into_response()
}

async fn body_json(body: &Bytes) -> Result<Value, Response> {
    serde_json::from_slice::<Value>(body)
        .map_err(|e| fail_json(StatusCode::BAD_REQUEST, &e.to_string()))
}

// ------------------------------------------------------- controller payloads

fn proxy_delay(proxy: &Value) -> i64 {
    proxy
        .get("history")
        .and_then(|v| v.as_array())
        .and_then(|h| h.last())
        .and_then(|h| h.get("delay"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

/// True when the controller reports a group the user can switch manually.
pub(crate) fn is_selector(kind: &str) -> bool {
    kind == "selector"
}

/// mihomo's built-in `GLOBAL` selector contains *every* proxy and group, so it
/// is always first in `/proxies`. It is only meaningful in `mode: global`; in
/// rule/direct mode switching it changes nothing, yet it used to win the
/// "primary group" election and made manual switching look broken.
fn is_global_group(name: &str) -> bool {
    name == "GLOBAL"
}

/// Airport subscriptions often inject fake "nodes" that only carry account
/// text (remaining traffic, expiry). They are real shadowsocks entries to the
/// controller, so clicking one switches routing to a dead server — which looks
/// exactly like "manual switching is broken" (the page loses connectivity).
fn is_placeholder_node(name: &str) -> bool {
    const PREFIXES: [&str; 9] = [
        "剩余流量",
        "过期时间",
        "套餐",
        "到期时间",
        "距离下次",
        "流量重置",
        "官网",
        "订阅",
        "客服",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn proxy_tested(proxy: &Value) -> bool {
    proxy
        .get("history")
        .and_then(|v| v.as_array())
        .map(|h| !h.is_empty())
        .unwrap_or(false)
}

fn proxy_type(proxy: &Value) -> String {
    proxy
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase()
}

/// GET /proxies map → {groups, nodes}.
///
/// Parsing rules (shared by REST, the WebSocket snapshot and the selector):
/// * groups keep the controller's order (no alphabetical re-sort), because
///   subscriptions define a meaningful order (节点选择 → 自动选择 → …);
/// * nodes keep first-seen order and carry *all* groups they belong to;
/// * `group` is the group a manual click should target: the first *selector*
///   group that contains the node (falls back to the first group otherwise);
/// * `selectable` tells the UI whether any manual switch is possible;
/// * `tested` distinguishes "never measured" from "measured timeout".
pub async fn proxies_payload(st: &AppState) -> Result<Value, String> {
    let data = ctrl_json(st, "GET", "/proxies", None, Duration::from_secs(10)).await?;
    let proxies = data
        .get("proxies")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "bad proxies payload".to_string())?;
    Ok(build_proxies(proxies))
}

/// Pure part of the proxy parser (unit-testable without a controller).
pub(crate) fn build_proxies(proxies: &serde_json::Map<String, Value>) -> Value {
    let empty = Vec::new();
    let mut groups: Vec<Value> = Vec::new();
    let mut group_names: Vec<String> = Vec::new();
    let mut group_selectable: Vec<bool> = Vec::new();
    let mut member_names: Vec<Vec<String>> = Vec::new();

    for (name, proxy) in proxies {
        let all = proxy.get("all").and_then(|v| v.as_array()).unwrap_or(&empty);
        if all.is_empty() {
            continue;
        }
        let kind = proxy_type(proxy);
        let selectable = is_selector(&kind);
        // Account-info pseudo entries are not selectable servers; keep them
        // out of the grid and out of the group's node count.
        let members: Vec<String> = all
            .iter()
            .filter_map(|m| m.as_str().map(|s| s.to_string()))
            .filter(|member| !is_placeholder_node(member))
            .collect();
        groups.push(json!({
            "name": name,
            "type": kind,
            "now": proxy.get("now").and_then(|v| v.as_str()).unwrap_or(""),
            "count": members.len(),
            "selectable": selectable,
        }));
        group_names.push(name.clone());
        group_selectable.push(selectable);
        member_names.push(members);
    }

    let mut nodes: Vec<Value> = Vec::new();
    let mut node_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (gi, name) in group_names.iter().enumerate() {
        for member in &member_names[gi] {
            let Some(item) = proxies.get(member) else {
                // Provider node currently unavailable: skip it rather than
                // showing a card that cannot be selected or tested.
                continue;
            };
            if let Some(&idx) = node_index.get(member.as_str()) {
                let node = &mut nodes[idx];
                if let Some(list) = node.get_mut("groups").and_then(|v| v.as_array_mut()) {
                    if !list.iter().any(|g| g.as_str() == Some(name.as_str())) {
                        list.push(Value::String(name.clone()));
                    }
                }
                // Prefer a real routing selector as the click target. GLOBAL is
                // kept in `groups` (the global-mode tab needs it) but never
                // becomes the default group.
                if group_selectable[gi] && !is_global_group(name) {
                    let current = node["group"].as_str().unwrap_or("");
                    if current.is_empty() || is_global_group(current) {
                        node["selectable"] = json!(true);
                        node["group"] = json!(name);
                    }
                }
                continue;
            }
            node_index.insert(member.clone(), nodes.len());
            nodes.push(json!({
                "name": member,
                "type": proxy_type(item),
                "delay": proxy_delay(item),
                "tested": proxy_tested(item),
                // Empty lets the UI fall back to `groups[0]` (which may be
                // GLOBAL for nodes that only live there).
                "group": if group_selectable[gi] && !is_global_group(name) { name.clone() } else { String::new() },
                "groups": [name],
                "selectable": group_selectable[gi],
                // A node that is itself a group (自动选择 inside 节点选择):
                // the UI renders it as a group shortcut, not as a server.
                "isGroup": item.get("all").is_some(),
                "provider": item.get("provider").and_then(|v| v.as_str()),
            }));
        }
    }

    json!({
        "groups": groups,
        "nodes": nodes,
    })
}

/// Validates a proxy selection and resolves the selector group that should
/// actually receive it. Shared by the REST handler and the tests.
///
/// Returns `(group_to_put, fallback_used)` or a user-facing error.
pub(crate) fn resolve_selector(
    proxies: &serde_json::Map<String, Value>,
    group: &str,
    name: &str,
) -> Result<(String, bool), String> {
    let entry = proxies
        .get(group)
        .ok_or_else(|| format!("策略组「{}」不存在或内核已重载配置", group))?;
    let members = entry.get("all").and_then(|v| v.as_array());
    let has_member = members
        .map(|list| list.iter().any(|m| m.as_str() == Some(name)))
        .unwrap_or(false);
    if !has_member {
        return Err(format!("节点「{}」不属于策略组「{}」", name, group));
    }
    if is_selector(&proxy_type(entry)) {
        return Ok((group.to_string(), false));
    }
    // Legacy or ambiguous callers may address an auto group (url-test /
    // fallback / load-balance). Manual selection still has a well-defined
    // meaning: switch the first *routing* selector that contains the node.
    // GLOBAL only applies when nothing else does (mode: global).
    let mut preferred: Vec<String> = Vec::new();
    let mut global: Option<String> = None;
    for (other, proxy) in proxies {
        let list = proxy.get("all").and_then(|v| v.as_array());
        let contains = list
            .map(|l| l.iter().any(|m| m.as_str() == Some(name)))
            .unwrap_or(false);
        if contains && is_selector(&proxy_type(proxy)) {
            if is_global_group(other) {
                if global.is_none() {
                    global = Some(other.clone());
                }
            } else {
                preferred.push(other.clone());
            }
        }
    }
    if let Some(first) = preferred.into_iter().next() {
        return Ok((first, true));
    }
    if let Some(only_global) = global {
        return Ok((only_global, true));
    }
    Err(format!(
        "策略组「{}」由内核自动选择节点，且该节点不在任何手动策略组中，无法手动切换",
        group
    ))
}

/// days-from-civil for RFC3339 timestamps (inverse of Howard Hinnant's
/// civil_from_days) — enough to turn controller "start" into a duration.
fn rfc3339_to_unix(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut date_parts = date.split('-');
    let y: i64 = date_parts.next()?.parse().ok()?;
    let m: i64 = date_parts.next()?.parse().ok()?;
    let d: i64 = date_parts.next()?.parse().ok()?;
    let mut time = rest.to_string();
    let mut offset_secs: i64 = 0;
    // strip fractional seconds
    if let Some(dot) = time.find('.') {
        let end = time[dot..]
            .find(|c: char| c == '+' || c == '-' || c == 'Z')
            .map(|i| dot + i)
            .unwrap_or(time.len());
        time = format!("{}{}", &time[..dot], &time[end..]);
    }
    if time.ends_with('Z') {
        time.pop();
    } else if let Some(idx) = time.rfind(['+', '-']) {
        let sign = if time.as_bytes()[idx] == b'+' { 1 } else { -1 };
        let off = &time[idx + 1..];
        let (oh, om) = match off.split_once(':') {
            Some((h, m)) => (h.parse::<i64>().unwrap_or(0), m.parse::<i64>().unwrap_or(0)),
            None => (off.parse::<i64>().unwrap_or(0), 0),
        };
        offset_secs = sign * (oh * 3600 + om * 60);
        time = time[..idx].to_string();
    }
    let mut time_parts = time.split(':');
    let hh: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    let mm: i64 = time_parts.next().unwrap_or("0").parse().ok()?;
    let ss: i64 = time_parts.next().unwrap_or("0").parse().ok()?;

    // days_from_civil
    let yy = if m <= 2 { y - 1 } else { y };
    let era = if yy >= 0 { yy } else { yy - 399 } / 400;
    let yoe = yy - era * 400;
    let mp = (m + 9) % 12;
    let dd = d - 1;
    let doy = (153 * mp + 2) / 5 + dd;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset_secs)
}

/// Raw controller `/connections` payload (None when the core is down).
pub async fn connections_snapshot(st: &AppState) -> Option<Value> {
    ctrl_json(st, "GET", "/connections", None, Duration::from_secs(10))
        .await
        .ok()
}

/// Projects the controller payload into the list the UI renders. Split from
/// the fetch so one request feeds both the list and the totals sampler.
pub fn connections_from(data: &Value) -> Vec<Value> {
    let empty = Vec::new();
    let conns = data
        .get("connections")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let now_unix = crate::state::now_secs();
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0);

    conns
        .iter()
        .map(|c| {
            let start = c.get("start").and_then(|v| v.as_str()).unwrap_or("");
            let start_unix = rfc3339_to_unix(start).unwrap_or(now_unix);
            let start_nanos = match rfc3339_nanos(start) {
                Some(frac) => start_unix * 1_000_000_000 + frac,
                None => start_unix * 1_000_000_000,
            };
            let duration = ((now_nanos - start_nanos) as f64 / 1e9).max(0.0);
            let meta = c.get("metadata").cloned().unwrap_or(Value::Null);
            let host = meta
                .get("host")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .or_else(|| {
                    meta.get("destinationIP")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default();
            json!({
                "id": c.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                "host": host,
                "port": meta.get("destinationPort").and_then(|v| v.as_str()).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
                "rule": c.get("rule").and_then(|v| v.as_str()).unwrap_or(""),
                "chain": c.get("chains").and_then(|v| v.as_array()).and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or(""),
                "up": c.get("upload").and_then(|v| v.as_i64()).unwrap_or(0),
                "down": c.get("download").and_then(|v| v.as_i64()).unwrap_or(0),
                "startTime": start_unix * 1000,
                "duration": duration,
                "source": meta.get("sourceIP").and_then(|v| v.as_str()).unwrap_or(""),
                "network": meta.get("network").and_then(|v| v.as_str()).unwrap_or(""),
                "process": meta.get("processPath").and_then(|v| v.as_str())
                    .map(|p| std::path::Path::new(p).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

pub async fn connections_payload(st: &AppState) -> Value {
    match connections_snapshot(st).await {
        Some(data) => json!(connections_from(&data)),
        None => json!([]),
    }
}

fn rfc3339_nanos(s: &str) -> Option<i64> {
    let dot = s.find('.')?;
    let end = s[dot..]
        .find(|c: char| c == '+' || c == '-' || c == 'Z')
        .map(|i| dot + i)
        .unwrap_or(s.len());
    let frac = &s[dot + 1..end];
    if frac.is_empty() {
        return None;
    }
    let digits = frac.chars().take(9).collect::<String>();
    let mut value: i64 = digits.parse().ok()?;
    for _ in digits.len()..9 {
        value *= 10;
    }
    Some(value)
}

fn groups_summary_from(data: &Value) -> Value {
    let empty = Vec::new();
    let proxies = data.get("proxies").and_then(|v| v.as_object());
    let mut groups: Vec<Value> = Vec::new();
    let mut timeout = 0usize;
    if let Some(proxies) = proxies {
        for (name, proxy) in proxies {
            // GLOBAL duplicates every proxy/group and is not a routing group
            // outside mode:global; keep it out of the dashboard summary.
            if is_global_group(name) {
                continue;
            }
            let all = proxy.get("all").and_then(|v| v.as_array()).unwrap_or(&empty);
            if all.is_empty() {
                continue;
            }
            let delay = proxy_delay(proxy);
            if delay == 0 {
                timeout += 1;
            }
            let count = all
                .iter()
                .filter(|m| {
                    m.as_str()
                        .map(|name| !is_placeholder_node(name))
                        .unwrap_or(false)
                })
                .count();
            groups.push(json!({
                "name": name,
                "type": proxy.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
                "now": proxy.get("now").and_then(|v| v.as_str()).unwrap_or(""),
                "count": count,
            }));
        }
    }
    groups.sort_by_key(|g| g.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
    json!({"count": groups.len(), "timeout": timeout, "groups": groups})
}

// ------------------------------------------------------------------ handlers

fn proxy_state() -> String {
    std::fs::read_to_string("/tmp/flclash-proxy.state")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "off".into())
}

fn proxy_wish() -> bool {
    std::process::Command::new("uci")
        .args(["-q", "get", "flclash.proxy.enabled"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

fn proxy_enabled() -> bool {
    matches!(proxy_state().as_str(), "on" | "tun")
}

/// The single status model used by `GET /status`, the WebSocket `status`
/// event and the `snapshot` payload. Everything else is derived from it.
pub async fn status_payload(st: &AppState) -> Value {
    let reachable = core_reachable(st).await;
    let mut running = false;
    let mut version = String::new();
    // config.yaml can be megabytes (a big subscription). Keep the flash read
    // off the async workers; /status is rebuilt every two seconds.
    let config_path = st.config_path();
    let mut cfg = tokio::task::spawn_blocking(move || file_config(&config_path))
        .await
        .unwrap_or_default();
    let mut groups_summary = json!({"count": 0, "timeout": 0, "groups": []});
    let mut stats_conns = 0usize;

    if reachable {
        running = true;
        // Three independent controller reads: run them concurrently. Sequentially
        // the worst case was 5+5+10+10s per status sample, and this runs every
        // two seconds; a slow /proxies stalled the whole dashboard behind it.
        // The connection count is not fetched here: the connections sampler is
        // its single producer, which removes one /connections request per tick.
        let (configs, ver, proxies) = tokio::join!(
            ctrl_json(st, "GET", "/configs", None, Duration::from_secs(5)),
            ctrl_json(st, "GET", "/version", None, Duration::from_secs(5)),
            ctrl_json(st, "GET", "/proxies", None, Duration::from_secs(10)),
        );
        if let Ok(data) = configs {
            let get_i = |key: &str| data.get(key).and_then(|v| v.as_i64());
            let get_b = |key: &str| data.get(key).and_then(|v| v.as_bool());
            let get_s = |key: &str| data.get(key).and_then(|v| v.as_str()).unwrap_or("");
            if let Some(v) = get_i("mixed-port") {
                cfg.mixed_port = v;
            }
            if let Some(v) = get_i("socks-port") {
                cfg.socks_port = v;
            }
            if let Some(v) = get_i("port") {
                cfg.port = v;
            }
            if let Some(v) = get_b("allow-lan") {
                cfg.allow_lan = v;
            }
            if let Some(v) = get_b("ipv6") {
                cfg.ipv6 = v;
            }
            if let Some(v) = get_b("tun").or_else(|| {
                data.get("tun").and_then(|v| v.get("enable")).and_then(|v| v.as_bool())
            }) {
                cfg.tun = v;
            }
            if !get_s("mode").is_empty() {
                cfg.mode = get_s("mode").to_string();
            }
            if !get_s("log-level").is_empty() {
                cfg.log_level = get_s("log-level").to_string();
            }
        }
        if let Ok(data) = ver {
            version = data
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
        if let Ok(data) = proxies {
            groups_summary = groups_summary_from(&data);
        }
    }

    let hist = st.hist.lock().unwrap();
    if running {
        stats_conns = hist.conn_count;
    }
    let up_rate = hist.up.last().copied().unwrap_or(0.0);
    let down_rate = hist.down.last().copied().unwrap_or(0.0);
    let pid = core_pid();
    let core_rss = if pid != 0 { rss_bytes(pid) } else { 0.0 };
    let profile = profiles::load_profiles(st)
        .iter()
        .find(|p| p.active)
        .map(|p| profiles::profile_payload(st, p))
        .unwrap_or_else(|| json!({}));

    let quality = match &hist.ipinfo {
        Some(info) => json!({"type": info.quality_type(), "level": info.quality_level()}),
        None => json!({"type": "直连", "level": if hist.outbound_ok { "正常" } else { "未知" }}),
    };
    let ipinfo = hist.ipinfo.as_ref().map(|info| {
        json!({
            "ip": info.ip,
            "country": info.country,
            "countryCode": info.country_code,
            "city": info.city,
            "isp": info.isp,
            "org": info.org,
            "asn": info.asn,
            "proxy": info.proxy,
            "hosting": info.hosting,
            "mobile": info.mobile,
        })
    });
    let dns_json = json!({
        "enable": cfg.dns_enable,
        "enhanced-mode": if cfg.dns_mode.is_empty() { "redir-host" } else { cfg.dns_mode.as_str() },
        "nameserver": cfg.dns_nameservers,
        "default-nameserver": cfg.dns_default_nameservers,
        "fake-ip-range": if cfg.dns_fake_ip_range.is_empty() { "198.18.0.1/16" } else { cfg.dns_fake_ip_range.as_str() },
        "fake-ip-filter": cfg.dns_fake_ip_filter,
    });

    json!({
        "running": running,
        "version": version,
        "coreVersion": version,
        "apiVersion": env!("CARGO_PKG_VERSION"),
        "uptime": if pid != 0 { core_uptime(pid) } else { 0.0 },
        "mode": cfg.mode,
        "outboundIp": hist.outbound_ip,
        "outboundIp6": hist.outbound_ip6,
        "countryCode": hist.outbound_cc,
        "organization": hist.outbound_org,
        "ipInfo": ipinfo,
        "quality": quality,
        "localIp": local_ipv4(),
        "mixedPort": cfg.mixed_port,
        "socksPort": cfg.socks_port,
        "httpPort": cfg.port,
        "systemProxy": proxy_enabled(),
        "proxyLan": proxy_wish(),
        "takeoverState": proxy_state(),
        "tun": cfg.tun,
        "tunStack": cfg.tun_stack,
        "dnsMode": cfg.dns_mode,
        "allowLan": cfg.allow_lan,
        "ipv6Enabled": cfg.ipv6,
        "logLevel": cfg.log_level,
        "dns": dns_json,
        "traffic": {
            "up": up_rate, "down": down_rate,
            "upRate": up_rate, "downRate": down_rate,
            "peakUp": hist.peak_up, "peakDown": hist.peak_down,
            "totalUp": hist.total_up, "totalDown": hist.total_dn,
            "history": {"up": hist.up, "down": hist.down},
        },
        "memory": {"app": rss_bytes(std::process::id()), "core": core_rss},
        "services": [],
        "stats": {"connections": stats_conns, "requests": 0, "dns": 0},
        "profile": profile,
        "proxyGroups": groups_summary,
        "generatedAt": crate::events::now_millis(),
    })
}

/// `GET /status`: served from the sampler's snapshot so REST readers and
/// WebSocket subscribers observe the same object (no per-client probing).
pub async fn status(State(st): State<Arc<AppState>>) -> Response {
    if st.snapshot_age_ms() < 3000 {
        return ok_json((*st.snapshot()).clone());
    }
    let value = status_payload(&st).await;
    ok_json(value)
}

/// Full state for a new WebSocket client (or an explicit `sync`/gap resync):
/// status + proxies + profiles + tasks + core capabilities. Everything here
/// is also available over REST, and both come from the same builders.
pub async fn snapshot_payload(st: &AppState) -> Value {
    let status = status_payload(st).await;
    let running = status.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
    let proxies = if running {
        match proxies_payload(st).await {
            Ok(mut payload) => {
                payload["running"] = json!(true);
                payload
            }
            Err(_) => json!({"groups": [], "nodes": [], "running": false}),
        }
    } else {
        json!({"groups": [], "nodes": [], "running": false})
    };
    let profiles = profiles::list_payload(st);
    json!({
        "protocol": crate::events::PROTOCOL_VERSION,
        "bootId": st.boot_id,
        "sequence": st.bus.last_sequence(),
        "generatedAt": crate::events::now_millis(),
        "status": status,
        "proxies": proxies,
        "profiles": profiles,
        "tasks": st.tasks.list(),
        "capabilities": crate::core::capabilities(&crate::core::configured_core()),
    })
}

pub async fn tasks_list(State(st): State<Arc<AppState>>) -> Response {
    ok_json(json!({"tasks": st.tasks.list()}))
}

pub async fn task_get(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match st.tasks.get(&id) {
        Some(task) => ok_json(json!({"task": task})),
        None => fail_json(StatusCode::NOT_FOUND, "任务不存在或已过期"),
    }
}

pub async fn core_capabilities(State(st): State<Arc<AppState>>) -> Response {
    ok_json(crate::core::payload(&st).await)
}

/// Realtime transport health for monitoring pages and tests.
pub async fn ws_stats(State(st): State<Arc<AppState>>) -> Response {
    ok_json(json!({
        "clients": st.ws_clients.load(std::sync::atomic::Ordering::Relaxed),
        "busReceivers": st.bus.receivers(),
        "lastSequence": st.bus.last_sequence(),
        "protocol": crate::events::PROTOCOL_VERSION,
        "topics": crate::events::TOPICS,
        "heartbeatSeconds": crate::ws::HEARTBEAT_SECS,
        "compression": ["gzip+base64"],
        "bootId": st.boot_id,
        "uptimeMs": crate::events::now_millis() - st.started_at,
    }))
}

pub async fn version(State(st): State<Arc<AppState>>) -> Response {
    match req(&st, "GET", "/version").await {
        Ok(reply) => ok_raw(reply.body),
        Err(_) => ok_json(json!({"version": "", "meta": false, "running": false})),
    }
}

pub async fn memory(State(_st): State<Arc<AppState>>) -> Response {
    let pid = core_pid();
    let core = if pid != 0 { rss_bytes(pid) } else { 0.0 };
    let app = rss_bytes(std::process::id());
    ok_json(json!({"rss": app + core, "app": app, "core": core}))
}

pub async fn rules(State(st): State<Arc<AppState>>) -> Response {
    match req(&st, "GET", "/rules").await {
        Ok(reply) => ok_raw(reply.body),
        Err(_) => ok_json(json!({"rules": []})),
    }
}

pub async fn providers(State(st): State<Arc<AppState>>) -> Response {
    match req(&st, "GET", "/providers/proxies").await {
        Ok(reply) => ok_raw(reply.body),
        Err(_) => ok_json(json!({"providers": {}})),
    }
}

pub async fn dns_resolve(
    State(st): State<Arc<AppState>>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(name) = query.get("name").cloned() else {
        return fail_json(StatusCode::BAD_REQUEST, "name is required");
    };
    if name.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "name is required");
    }
    let qtype = query.get("type").cloned().unwrap_or_else(|| "A".into());
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!(
        "/dns/query?name={}&type={}",
        urlencode(&name),
        urlencode(&qtype)
    );
    match req(&st, "GET", &path).await {
        Ok(reply) => ok_raw(reply.body),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn config_get(State(st): State<Arc<AppState>>) -> Response {
    let cfg = file_config(&st.config_path());
    let running = core_reachable(&st).await;
    let mut live = cfg.clone();
    if running {
        if let Ok(data) = ctrl_json(&st, "GET", "/configs", None, Duration::from_secs(5)).await {
            let get_i = |key: &str| data.get(key).and_then(|v| v.as_i64());
            let get_b = |key: &str| data.get(key).and_then(|v| v.as_bool());
            let get_s = |key: &str| data.get(key).and_then(|v| v.as_str()).unwrap_or("");
            if let Some(v) = get_i("mixed-port") {
                live.mixed_port = v;
            }
            if let Some(v) = get_i("socks-port") {
                live.socks_port = v;
            }
            if let Some(v) = get_i("port") {
                live.port = v;
            }
            if let Some(v) = get_b("allow-lan") {
                live.allow_lan = v;
            }
            if let Some(v) = get_b("ipv6") {
                live.ipv6 = v;
            }
            if let Some(v) = get_b("tun").or_else(|| {
                data.get("tun").and_then(|v| v.get("enable")).and_then(|v| v.as_bool())
            }) {
                live.tun = v;
            }
            if !get_s("mode").is_empty() {
                live.mode = get_s("mode").to_string();
            }
            if !get_s("log-level").is_empty() {
                live.log_level = get_s("log-level").to_string();
            }
        }
    }
    ok_json(json!({
        "running": running,
        "mode": live.mode,
        "mixedPort": live.mixed_port,
        "socksPort": live.socks_port,
        "httpPort": live.port,
        "allowLan": live.allow_lan,
        "ipv6": live.ipv6,
        "tun": live.tun,
        "tunStack": live.tun_stack,
        "dnsMode": live.dns_mode,
        "logLevel": live.log_level,
        "systemProxy": proxy_enabled(),
    }))
}

pub async fn config_put(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let patch = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    // Client-side mistakes (bad enum, newline injection, port range) are 400s,
    // not 502s: the old mapping made a rejected save look like a core failure.
    if let Err(err) = crate::config::validate_patch(&patch) {
        return fail_json(StatusCode::BAD_REQUEST, &err);
    }
    match save_config(&st, &patch).await {
        Ok(()) => {
            // Other tabs/config views must see the change without re-polling.
            st.bus.publish(
                "system",
                "system.config",
                json!({"changed": patch, "at": crate::events::now_millis()}),
            );
            crate::loops::refresh_snapshot(&st).await;
            ok_json(json!({"ok": true}))
        }
        Err(err) => bad_gateway(&err),
    }
}

pub async fn proxies(State(st): State<Arc<AppState>>) -> Response {
    match proxies_payload(&st).await {
        Ok(mut payload) => {
            payload["running"] = json!(true);
            ok_json(payload)
        }
        Err(_) => ok_json(json!({"groups": [], "nodes": [], "running": false})),
    }
}

/// Manual selection with an explicit contract:
///
/// * the request must name a group the node actually belongs to;
/// * a node living in an auto group (url-test / fallback / load-balance) is
///   still switchable if at least one *selector* group contains it — the
///   selector is resolved server-side and reported back (`autoFallback`);
/// * otherwise the call fails with 409 and a Chinese explanation instead of
///   silently doing nothing (the old bug: `PUT` to a non-selector group).
pub async fn proxies_select(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let group = data.get("group").and_then(|v| v.as_str()).unwrap_or("").trim();
    let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    if group.is_empty() || name.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "group and name are required");
    }
    if group.len() > 256 || name.len() > 256 {
        return fail_json(StatusCode::BAD_REQUEST, "group or name too long");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let request_id = data
        .get("requestId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let proxy_map = match ctrl_json(&st, "GET", "/proxies", None, Duration::from_secs(10)).await {
        Ok(value) => value
            .get("proxies")
            .and_then(|v| v.as_object())
            .cloned(),
        Err(err) => return bad_gateway(&err),
    };
    let Some(proxy_map) = proxy_map else {
        return bad_gateway("controller returned an unexpected /proxies payload");
    };
    let (target, fallback) = match resolve_selector(&proxy_map, group, name) {
        Ok(value) => value,
        Err(err) => return fail_json(StatusCode::CONFLICT, &err),
    };
    let path = format!("/proxies/{}", urlencode(&target));
    match req_body(&st, "PUT", &path, &json!({"name": name}).to_string()).await {
        Ok(reply) if reply.status < 400 => {
            // Push the change to every connected client immediately; the
            // proxy sampler confirms it with a full payload afterwards.
            st.bus.publish_with(
                "proxies",
                "proxy.select",
                json!({
                    "group": target,
                    "requestedGroup": group,
                    "name": name,
                    "autoFallback": fallback,
                }),
                request_id.clone(),
                None,
            );
            ok_json(json!({
                "ok": true,
                "group": target,
                "name": name,
                "autoFallback": fallback,
            }))
        }
        Ok(reply) => bad_gateway(&format!(
            "controller: {}",
            String::from_utf8_lossy(&reply.body).trim()
        )),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn proxies_delay(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "name is required");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!(
        "/proxies/{}/delay?timeout=5000&url={}",
        urlencode(name),
        urlencode("https://www.gstatic.com/generate_204")
    );
    match req(&st, "GET", &path).await {
        Ok(reply) => {
            let value = serde_json::from_slice::<Value>(&reply.body)
                .ok()
                .and_then(|v| v.get("delay").and_then(|d| d.as_i64()))
                .unwrap_or(0);
            st.bus.publish(
                "proxies",
                "proxy.delay",
                json!({"name": name, "value": value, "tested": true}),
            );
            ok_json(json!({"name": name, "value": value, "tested": true}))
        }
        Err(err) => bad_gateway(&err),
    }
}

pub async fn proxies_group_delay(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let group = data.get("group").and_then(|v| v.as_str()).unwrap_or("");
    if group.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "group is required");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!(
        "/group/{}/delay?timeout=5000&url={}",
        urlencode(group),
        urlencode("https://www.gstatic.com/generate_204")
    );
    match req(&st, "GET", &path).await {
        Ok(reply) => {
            if let Ok(value) = serde_json::from_slice::<Value>(&reply.body) {
                st.bus.publish(
                    "proxies",
                    "proxy.delay.batch",
                    json!({"group": group, "delays": value}),
                );
            }
            ok_raw(reply.body)
        }
        Err(err) => bad_gateway(&err),
    }
}

pub async fn connections(State(st): State<Arc<AppState>>) -> Response {
    ok_json(connections_payload(&st).await)
}

pub async fn connection_close(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    if id.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "missing id");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!("/connections/{}", urlencode(&id));
    match req(&st, "DELETE", &path).await {
        Ok(reply) if reply.status < 400 => ok_json(json!({"ok": true})),
        Ok(reply) => bad_gateway(&format!("controller: {}", String::from_utf8_lossy(&reply.body).trim())),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn dns_queries() -> Response {
    ok_json(json!([]))
}

pub async fn dns_flush(State(st): State<Arc<AppState>>) -> Response {
    let _ = req(&st, "POST", "/cache/fakeip/flush").await;
    ok_json(json!({"ok": true}))
}

pub async fn logs(State(st): State<Arc<AppState>>) -> Response {
    let hist = st.hist.lock().unwrap();
    let out: Vec<Value> = hist
        .logs
        .iter()
        .map(|entry| json!({"time": entry.time, "level": entry.level, "message": entry.message}))
        .collect();
    ok_json(json!(out))
}

pub async fn system_proxy(State(st): State<Arc<AppState>>) -> Response {
    let cfg = file_config(&st.config_path());
    // The controller address is configurable; reporting a hard-coded 9097 made
    // the read-only summary wrong as soon as external_controller changed.
    let api_port = st
        .controller
        .rsplit(':')
        .next()
        .and_then(|p| p.parse::<i64>().ok())
        .unwrap_or(9097);
    ok_json(json!({
        "backend": "flclash",
        "mixed_port": cfg.mixed_port,
        "clash_api_port": api_port,
        "tun_name": "",
        "routing_mode": cfg.mode,
        "ipv6_support": cfg.ipv6,
        "running": core_reachable(&st).await,
        "read_only": true,
        "source": "flclash-api",
    }))
}

pub async fn profiles_get(State(st): State<Arc<AppState>>) -> Response {
    ok_json(profiles::list_payload(&st))
}

pub async fn profiles_import(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let url = data.get("url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if url.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "url is required");
    }
    // Long operation: answer immediately with a task id, stream progress and
    // the final result on the `tasks` topic (survives a client disconnect).
    let handle = st.tasks.begin("profile.import", "导入订阅", None);
    let task_id = handle.id().to_string();
    handle.progress(5.0, "下载订阅");
    let st_bg = st.clone();
    tokio::spawn(async move {
        match profiles::import_profile(&st_bg, &url).await {
            Ok(item) => {
                let payload = profiles::profile_payload(&st_bg, &item);
                st_bg
                    .bus
                    .publish("profiles", "profiles.changed", profiles::list_payload(&st_bg));
                crate::loops::refresh_snapshot(&st_bg).await;
                handle.succeed(json!({"profile": payload}));
            }
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

pub async fn profiles_apply(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let handle = st.tasks.begin("profile.apply", "应用配置", None);
    let task_id = handle.id().to_string();
    let st_bg = st.clone();
    tokio::spawn(async move {
        match profiles::apply_profile(&st_bg, &id).await {
            Ok(()) => {
                st_bg
                    .bus
                    .publish("profiles", "profiles.changed", profiles::list_payload(&st_bg));
                crate::loops::refresh_snapshot(&st_bg).await;
                handle.succeed(json!({"id": id}));
            }
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

pub async fn profiles_update(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let handle = st.tasks.begin("profile.update", "更新订阅", None);
    let task_id = handle.id().to_string();
    let st_bg = st.clone();
    tokio::spawn(async move {
        match profiles::update_profile(&st_bg, &id).await {
            Ok(result) => {
                st_bg
                    .bus
                    .publish("profiles", "profiles.changed", profiles::list_payload(&st_bg));
                crate::loops::refresh_snapshot(&st_bg).await;
                handle.succeed(result);
            }
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

pub async fn profiles_edit(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: Bytes) -> Response {
    let patch = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match profiles::patch_profile(&st, &id, &patch) {
        Ok(()) => {
            st.bus
                .publish("profiles", "profiles.changed", profiles::list_payload(&st));
            ok_json(json!({"ok": true, "id": id}))
        }
        Err(err) => bad_gateway(&err),
    }
}

pub async fn profiles_delete(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match profiles::delete_profile(&st, &id) {
        Ok(()) => {
            st.bus
                .publish("profiles", "profiles.changed", profiles::list_payload(&st));
            ok_json(json!({"ok": true, "id": id}))
        }
        Err(err) => bad_gateway(&err),
    }
}

pub async fn service_action(
    State(st): State<Arc<AppState>>,
    Path(action): Path<String>,
) -> Response {
    if !matches!(action.as_str(), "start" | "stop" | "restart") {
        return fail_json(StatusCode::BAD_REQUEST, "unsupported service action");
    }
    let handle = st.tasks.begin("service", "内核服务", None);
    let task_id = handle.id().to_string();
    let st_bg = st.clone();
    tokio::spawn(async move {
        match profiles::service(&st_bg, &action).await {
            Ok(()) => {
                st_bg.bus.publish(
                    "system",
                    "system.service",
                    json!({"action": action, "state": "done"}),
                );
                crate::loops::refresh_snapshot(&st_bg).await;
                handle.succeed(json!({"action": action}));
            }
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

/// LAN transparent-proxy takeover, ShellClash-style: nft REDIRECT/TPROXY plus
/// DNS hijack handled by /usr/libexec/flclash/proxy-rules.sh. Never touches
/// the router's own outbound path, so the management plane stays alive.
pub async fn proxy_status(State(st): State<Arc<AppState>>) -> Response {
    let enabled = proxy_wish();
    let state = proxy_state();
    let cfg = file_config(&st.config_path());
    let core_running = core_reachable(&st).await;
    let backend = std::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
        .arg("backend")
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s == "nftables" || s == "iptables" { Some(s) } else { None }
        })
        .unwrap_or_else(|| {
            // Fallback detection mirrors proxy-rules.sh when the helper is
            // not installed (e.g. local dev): presence of fw4 selects nft.
            if std::path::Path::new("/sbin/fw4").exists()
                || std::path::Path::new("/usr/sbin/fw4").exists()
            {
                "nftables".to_string()
            } else {
                "iptables".to_string()
            }
        });
    let rules = std::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
        .arg("status")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let lan_if = std::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
        .arg("lan")
        .output()
        .ok()
        .and_then(|o| {
            if !o.status.success() {
                return None;
            }
            let value = String::from_utf8_lossy(&o.stdout).trim().to_string();
            (!value.is_empty()).then_some(value)
        })
        .unwrap_or_else(|| "br-lan".into());
    ok_json(json!({
        "enabled": enabled,
        "proxyWish": enabled,
        "takeover": state.clone(),
        "coreRunning": core_running,
        "mode": cfg.mode,
        "tunConfigured": cfg.tun,
        "tunActive": core_running && state == "tun",
        "rulesActive": rules,
        "firewallBackend": backend,
        "lanInterface": lan_if,
        "dnsPort": 1053,
        "redirPort": 7893,
        "tproxyPort": 7894,
    }))
}

pub async fn proxy_toggle(
    State(_st): State<Arc<AppState>>,
    Path(action): Path<String>,
) -> Response {
    if !matches!(action.as_str(), "start" | "stop") {
        return fail_json(StatusCode::BAD_REQUEST, "action must be start or stop");
    }
    let script = "/usr/libexec/flclash/proxy-rules.sh";
    let output = tokio::process::Command::new(script)
        .arg(&action)
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => {
            // The helper owns the live state ("on"/"tun"/"off"). Do not
            // overwrite a successful TUN convergence with a generic "on".
            ok_json(json!({"ok": true, "action": action}))
        }
        Ok(out) => bad_gateway(&format!(
            "{}: {}",
            action,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(err) => bad_gateway(&format!("proxy-rules.sh: {}", err)),
    }
}

// ------------------------------------------------------------ core management

pub async fn core_info(State(_st): State<Arc<AppState>>) -> Response {
    ok_json(crate::coremgmt::info())
}

pub async fn core_switch(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let core = data.get("core").and_then(|v| v.as_str()).unwrap_or("");
    match crate::coremgmt::switch(&st, core).await {
        Ok(value) => ok_json(value),
        Err(err) => fail_json(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn core_update(State(_st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let core = data.get("core").and_then(|v| v.as_str()).unwrap_or("");
    match crate::coremgmt::update_start(core) {
        Ok(value) => ok_json(value),
        Err(err) => fail_json(StatusCode::BAD_REQUEST, &err),
    }
}

pub async fn core_update_status(State(_st): State<Arc<AppState>>) -> Response {
    ok_json(crate::coremgmt::update_status())
}

// ------------------------------------------------- providers & geo updates

/// Rule providers from the live core (mihomo /providers/rules).
pub async fn rule_providers(State(st): State<Arc<AppState>>) -> Response {
    if !core_reachable(&st).await {
        return ok_json(json!({"running": false, "providers": []}));
    }
    match ctrl_json(&st, "GET", "/providers/rules", None, Duration::from_secs(8)).await {
        Ok(data) => {
            let mut providers: Vec<Value> = data
                .get("providers")
                .and_then(|v| v.as_object())
                .map(|map| {
                    map.iter()
                        .map(|(name, p)| {
                            json!({
                                "name": name,
                                "type": p.get("type").and_then(|v| v.as_str()).unwrap_or("rule"),
                                "vehicleType": p.get("vehicleType").and_then(|v| v.as_str()).unwrap_or(""),
                                "behavior": p.get("behavior").and_then(|v| v.as_str()).unwrap_or(""),
                                "ruleCount": p.get("ruleCount").and_then(|v| v.as_i64()).unwrap_or(0),
                                "updatedAt": p.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            providers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            ok_json(json!({"running": true, "providers": providers}))
        }
        // sing-box has no rule-provider controller API: report honestly.
        Err(_) => ok_json(json!({"running": true, "providers": [], "unsupported": true})),
    }
}

/// Update one proxy provider (mihomo PUT /providers/proxies/{name}).
pub async fn proxy_provider_update(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if name.is_empty() || name.len() > 256 {
        return fail_json(StatusCode::BAD_REQUEST, "invalid provider name");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!("/providers/proxies/{}", urlencode(&name));
    match req_body(&st, "PUT", &path, "").await {
        Ok(reply) if reply.status < 400 => {
            st.bus.publish("proxies", "provider.updated", json!({"provider": name}));
            ok_json(json!({"ok": true, "name": name}))
        }
        Ok(reply) => bad_gateway(&format!("controller: {}", String::from_utf8_lossy(&reply.body).trim())),
        Err(err) => bad_gateway(&err),
    }
}

/// Update one rule provider (mihomo PUT /providers/rules/{name}).
pub async fn rule_provider_update(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    if name.is_empty() || name.len() > 256 {
        return fail_json(StatusCode::BAD_REQUEST, "invalid provider name");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!("/providers/rules/{}", urlencode(&name));
    match req_body(&st, "PUT", &path, "").await {
        Ok(reply) if reply.status < 400 => {
            st.bus.publish("rules", "rules.changed", json!({"provider": name}));
            ok_json(json!({"ok": true, "name": name}))
        }
        Ok(reply) => bad_gateway(&format!("controller: {}", String::from_utf8_lossy(&reply.body).trim())),
        Err(err) => bad_gateway(&err),
    }
}

/// GeoIP/GeoSite database update via the core's own updater
/// (mihomo POST /upgrade/geo). Long-running: reported as a task.
pub async fn geo_update(State(st): State<Arc<AppState>>) -> Response {
    if !core_reachable(&st).await {
        return core_down();
    }
    let handle = st.tasks.begin("system.geo", "更新 Geo 数据", None);
    let task_id = handle.id().to_string();
    handle.progress(5.0, "请求内核更新");
    tokio::spawn(async move {
        match req_body(&st, "POST", "/upgrade/geo", "").await {
            Ok(reply) if reply.status < 400 => {
                handle.succeed(json!({"ok": true, "detail": String::from_utf8_lossy(&reply.body).trim().to_string()}));
            }
            Ok(reply) => handle.fail(&format!("controller: {}", String::from_utf8_lossy(&reply.body).trim())),
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

/// Full dns: view for the advanced DNS page (file-backed, works core-down).
pub async fn dns_config(State(st): State<Arc<AppState>>) -> Response {
    let cfg = file_config(&st.config_path());
    ok_json(json!({"dns": cfg.dns_json(), "running": core_reachable(&st).await}))
}

// ------------------------------------------------------------------ LAN ACL

pub async fn acl_get(State(_st): State<Arc<AppState>>) -> Response {
    let acl = crate::acl::load();
    let state = proxy_state();
    ok_json(json!({
        "acl": acl.to_json(),
        "takeover": state,
        // LAN ACL applies to REDIRECT/TPROXY only; TUN intercepts at L3.
        "applies": state == "on",
        "note": if state == "tun" { "TUN 模式下局域网 ACL 不生效（三层接管）" } else { "" },
    }))
}

pub async fn acl_put(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let acl = match crate::acl::parse_payload(&data) {
        Ok(a) => a,
        Err(err) => return fail_json(StatusCode::BAD_REQUEST, &err),
    };
    if let Err(err) = crate::acl::save(&acl) {
        return bad_gateway(&err);
    }
    st.bus.publish("system", "acl.changed", acl.to_json());
    // Rebuild live rules so the change takes effect immediately. reapply is
    // a no-op when the proxy is off or in TUN mode.
    let out = tokio::process::Command::new("/usr/libexec/flclash/proxy-rules.sh")
        .arg("reapply")
        .output()
        .await;
    let reapplied = matches!(&out, Ok(o) if o.status.success());
    let detail = out
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "proxy-rules.sh 不可用（开发环境）".into());
    ok_json(json!({"ok": true, "acl": acl.to_json(), "reapplied": reapplied, "detail": detail}))
}

// ------------------------------------------------------------- static assets

pub async fn static_index() -> Redirect {
    Redirect::to("/flclash/")
}

pub async fn static_root(State(st): State<Arc<AppState>>) -> Response {
    serve_file(&st, "index.html").await
}

pub async fn static_file(State(st): State<Arc<AppState>>, Path(path): Path<String>) -> Response {
    serve_file(&st, &path).await
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Html("<h1>404</h1>".to_string())).into_response()
}

/// Serves a file below `ui_dir` and nothing else.
///
/// Axum percent-decodes path captures, so `%2F` arrives as a literal `/`:
/// `Path::join` would then treat the value as absolute and serve any file the
/// API runs as (root). `%2E%2E` variants defeat the old `contains("..")` check
/// the same way. The only reliable rule is "the canonical target must stay
/// inside the canonical UI root".
///
/// Async on purpose: a static bundle read is a flash I/O wait, and blocking a
/// Tokio worker stalls every other request (including the WebSocket pusher).
async fn serve_file(st: &Arc<AppState>, rel: &str) -> Response {
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() || rel.contains('\\') || rel.contains('\0') {
        return not_found();
    }
    let root = std::path::Path::new(&st.ui_dir);
    let Ok(root) = tokio::fs::canonicalize(root).await else {
        return not_found();
    };
    let target = root.join(rel);
    let Ok(target) = tokio::fs::canonicalize(&target).await else {
        return not_found();
    };
    if !target.starts_with(&root) {
        return not_found();
    }
    match tokio::fs::read(&target).await {
        Ok(bytes) => {
            let mime = mime_of(&rel);
            (
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        format!("{}; charset=utf-8", mime),
                    ),
                    // The UI ships unhashed asset names. Without an explicit
                    // policy browsers heuristically cache them, so a deployed
                    // JS fix can keep running as the previous version for days
                    // (exactly what made "manual switching" look broken after
                    // the backend was already fixed).
                    (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
                    (axum::http::header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
                ],
                bytes,
            )
                .into_response()
        }
        Err(_) => not_found(),
    }
}

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html",
        "css" => "text/css",
        "js" => "application/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        _ => "application/octet-stream",
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

// ------------------------------------------------------------- mixin / editor

pub async fn mixin_get(State(st): State<Arc<AppState>>) -> Response {
    let text = std::fs::read_to_string(crate::mixin::mixin_path(&st.data_dir)).unwrap_or_default();
    ok_json(json!({"text": text, "enabled": !text.trim().is_empty()}))
}

pub async fn mixin_put(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if text.len() > 1 << 20 {
        return fail_json(StatusCode::BAD_REQUEST, "mixin 内容过大");
    }
    match crate::mixin::save_mixin(&st.data_dir, text) {
        Ok(()) => {
            st.bus.publish("system", "mixin.changed", json!({"enabled": !text.trim().is_empty()}));
            ok_json(json!({"ok": true}))
        }
        Err(err) => fail_json(StatusCode::BAD_REQUEST, &err),
    }
}

/// POST /config/validate {text?, profileId?} → structural check + mixin preview.
pub async fn config_validate(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let text = if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
        text.to_string()
    } else if let Some(id) = data.get("profileId").and_then(|v| v.as_str()) {
        // The id is user input (URL/path segment): never join it unchecked.
        let Some(path) = profiles::profile_path(&st, id) else {
            return fail_json(StatusCode::BAD_REQUEST, "非法配置 ID");
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return fail_json(StatusCode::NOT_FOUND, "配置不存在"),
        }
    } else {
        match std::fs::read_to_string(st.config_path()) {
            Ok(text) => text,
            Err(_) => return fail_json(StatusCode::NOT_FOUND, "没有可校验的配置"),
        }
    };
    let base_check = crate::mixin::validate_payload(&text);
    match crate::mixin::apply_mixin(&st.data_dir, &text) {
        Ok((merged, touched)) => {
            let merged_check = crate::mixin::validate_payload(&merged);
            ok_json(json!({
                "base": base_check,
                "merged": merged_check,
                "mixinKeys": touched,
                "preview": merged,
            }))
        }
        Err(err) => fail_json(StatusCode::BAD_REQUEST, &err),
    }
}

/// GET /profiles/{id}/raw → the stored (converted) profile YAML for editing.
pub async fn profile_raw(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let Some(path) = profiles::profile_path(&st, &id) else {
        return fail_json(StatusCode::BAD_REQUEST, "非法配置 ID");
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => ok_json(json!({"id": id, "text": text})),
        Err(_) => fail_json(StatusCode::NOT_FOUND, "配置不存在"),
    }
}

/// PUT /profiles/{id}/raw {text} → validate, back up, atomic write, re-apply
/// when this profile is the active one.
pub async fn profile_raw_put(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    body: Bytes,
) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if text.trim().is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "配置内容为空");
    }
    if text.len() > 4 << 20 {
        return fail_json(StatusCode::BAD_REQUEST, "配置内容过大");
    }
    let Some(path) = profiles::profile_path(&st, &id) else {
        return fail_json(StatusCode::BAD_REQUEST, "非法配置 ID");
    };
    if !path.is_file() {
        return fail_json(StatusCode::NOT_FOUND, "配置不存在");
    }
    let check = crate::mixin::validate_payload(text);
    if !check["ok"].as_bool().unwrap_or(false) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "配置校验失败", "detail": check})),
        )
            .into_response();
    }
    // Keep the previous body recoverable before overwriting.
    if let Ok(previous) = std::fs::read(&path) {
        let backup = st.profiles_dir().join(format!("{}.yaml.bak", id));
        let _ = crate::config::write_atomic(&backup, &previous);
    }
    if let Err(err) = crate::config::write_atomic(&path, text.as_bytes()) {
        return fail_json(StatusCode::INTERNAL_SERVER_ERROR, &err);
    }
    let active = crate::profiles::load_profiles(&st)
        .iter()
        .any(|item| item.id == id && item.active);
    if active {
        let st_bg = st.clone();
        let id_bg = id.clone();
        let handle = st.tasks.begin("profile.apply", "应用编辑后的配置", None);
        let task_id = handle.id().to_string();
        tokio::spawn(async move {
            match crate::profiles::apply_profile(&st_bg, &id_bg).await {
                Ok(()) => {
                    st_bg.bus.publish(
                        "profiles",
                        "profiles.changed",
                        crate::profiles::list_payload(&st_bg),
                    );
                    crate::loops::refresh_snapshot(&st_bg).await;
                    handle.succeed(json!({"id": id_bg}));
                }
                Err(err) => handle.fail(&err),
            }
        });
        return accepted(task_id);
    }
    st.bus
        .publish("profiles", "profiles.changed", crate::profiles::list_payload(&st));
    ok_json(json!({"ok": true, "id": id, "applied": false}))
}

// ------------------------------------------------------------- backup / restore

pub async fn backup_list(State(st): State<Arc<AppState>>) -> Response {
    ok_json(crate::backup::list(&st))
}

/// POST /backup {label?} → creates a snapshot of the whitelisted data files.
pub async fn backup_create(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = if body.is_empty() {
        json!({})
    } else {
        match body_json(&body).await {
            Ok(v) => v,
            Err(resp) => return resp,
        }
    };
    let label = data.get("label").and_then(|v| v.as_str()).unwrap_or("manual");
    match crate::backup::snapshot(&st, label, "") {
        Ok(meta) => {
            st.bus.publish("system", "backup.created", meta.clone());
            ok_json(json!({"ok": true, "snapshot": meta}))
        }
        Err(err) => fail_json(StatusCode::BAD_REQUEST, &err),
    }
}

/// GET /backup/download/{name} → the stored snapshot as a gzip tarball.
pub async fn backup_download(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    match crate::backup::read_snapshot(&st, &name) {
        Ok(bytes) => (
            [
                (axum::http::header::CACHE_CONTROL, "no-store"),
                (axum::http::header::CONTENT_TYPE, "application/gzip"),
                (
                    axum::http::header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{}\"", name),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => fail_json(StatusCode::NOT_FOUND, &err),
    }
}

/// POST /backup/restore {name} → restore a stored snapshot (background task).
pub async fn backup_restore_snapshot(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let bytes = match crate::backup::read_snapshot(&st, name) {
        Ok(bytes) => bytes,
        Err(err) => return fail_json(StatusCode::NOT_FOUND, &err),
    };
    restore_task(st, bytes)
}

/// POST /backup/upload → raw gzip tarball body; validated then restored.
pub async fn backup_upload(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    if body.is_empty() || body.len() > crate::backup::MAX_TOTAL_BYTES {
        return fail_json(StatusCode::BAD_REQUEST, "备份内容为空或过大");
    }
    restore_task(st, body.to_vec())
}

fn restore_task(st: Arc<AppState>, bytes: Vec<u8>) -> Response {
    // Validate synchronously so a bad archive fails fast with a useful error.
    if let Err(err) = crate::backup::unpack(&bytes) {
        return fail_json(StatusCode::BAD_REQUEST, &err);
    }
    let handle = st.tasks.begin("backup.restore", "恢复配置", None);
    let task_id = handle.id().to_string();
    handle.progress(10.0, "校验备份");
    tokio::spawn(async move {
        match crate::backup::restore(&st, &bytes) {
            Ok(result) => {
                handle.progress(60.0, "重载内核");
                crate::profiles::reload_core(&st).await;
                st.bus
                    .publish("profiles", "profiles.changed", crate::profiles::list_payload(&st));
                crate::loops::refresh_snapshot(&st).await;
                handle.succeed(result);
            }
            Err(err) => handle.fail(&err),
        }
    });
    accepted(task_id)
}

pub async fn backup_delete(State(st): State<Arc<AppState>>, Path(name): Path<String>) -> Response {
    match crate::backup::delete(&st, &name) {
        Ok(()) => {
            st.bus.publish("system", "backup.deleted", json!({"name": name}));
            ok_json(json!({"ok": true}))
        }
        Err(err) => fail_json(StatusCode::NOT_FOUND, &err),
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::*;

    fn fixture() -> serde_json::Map<String, Value> {
        json!({
            "节点选择": {"type": "Selector", "now": "自动选择", "all": ["自动选择", "香港 01", "日本 02"]},
            "自动选择": {"type": "URLTest", "now": "香港 01", "all": ["香港 01", "日本 02"]},
            "香港 01": {"type": "Vmess", "history": [{"delay": 42}]},
            "日本 02": {"type": "Trojan", "history": [{"delay": 0}]},
            "直连": {"type": "Direct"}
        })
        .as_object()
        .unwrap()
        .clone()
    }

    #[test]
    fn keeps_group_order_and_marks_selectable() {
        let payload = build_proxies(&fixture());
        let groups = payload["groups"].as_array().unwrap();
        let names: Vec<&str> = groups.iter().map(|g| g["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["节点选择", "自动选择"]);
        assert_eq!(groups[0]["selectable"], true);
        assert_eq!(groups[1]["selectable"], false);
        assert_eq!(groups[0]["count"], 3);
    }

    #[test]
    fn nodes_carry_membership_and_primary_selector() {
        let payload = build_proxies(&fixture());
        let nodes = payload["nodes"].as_array().unwrap();
        let hk = nodes.iter().find(|n| n["name"] == "香港 01").unwrap();
        assert_eq!(hk["group"], "节点选择");
        assert_eq!(hk["groups"].as_array().unwrap().len(), 2);
        assert_eq!(hk["selectable"], true);
        assert_eq!(hk["delay"], 42);
        assert_eq!(hk["tested"], true);

        let jp = nodes.iter().find(|n| n["name"] == "日本 02").unwrap();
        assert_eq!(jp["delay"], 0);
        assert_eq!(jp["tested"], true, "measured timeout must not look untested");

        let auto = nodes.iter().find(|n| n["name"] == "自动选择").unwrap();
        assert_eq!(auto["isGroup"], true);
        assert_eq!(auto["group"], "节点选择");
        // Duplicate membership must not repeat the same group name.
        assert_eq!(auto["groups"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn selector_resolution_prefers_manual_groups() {
        let map = fixture();
        assert_eq!(
            resolve_selector(&map, "节点选择", "日本 02").unwrap(),
            ("节点选择".to_string(), false)
        );
        // Addressing an auto group still resolves to a selector.
        assert_eq!(
            resolve_selector(&map, "自动选择", "日本 02").unwrap(),
            ("节点选择".to_string(), true)
        );
        // Membership is validated, not assumed.
        assert!(resolve_selector(&map, "节点选择", "香港 01").is_ok());
        assert!(resolve_selector(&map, "节点选择", "直连").is_err());
        assert!(resolve_selector(&map, "自动选择", "香港 01").is_ok());
        assert!(resolve_selector(&map, "不存在", "香港 01").is_err());
    }

    #[test]
    fn auto_only_nodes_are_rejected_with_reason() {
        let map = json!({
            "自动选择": {"type": "URLTest", "now": "A", "all": ["A"]},
            "A": {"type": "Vless", "history": []}
        })
        .as_object()
        .unwrap()
        .clone();
        let err = resolve_selector(&map, "自动选择", "A").unwrap_err();
        assert!(err.contains("无法手动切换"), "{}", err);
        let payload = build_proxies(&map);
        assert_eq!(payload["nodes"][0]["selectable"], false);
        assert_eq!(payload["nodes"][0]["tested"], false);
    }

    #[test]
    fn global_is_never_the_primary_target() {
        // Controller order puts GLOBAL first, exactly like mihomo does. The
        // node's primary group used to become GLOBAL, so manual clicks in the
        // "全部" view switched a group that rule mode never reads.
        let map = json!({
            "GLOBAL": {"type": "Selector", "now": "香港 01", "all": ["自动选择", "香港 01", "日本 02"]},
            "自动选择": {"type": "URLTest", "now": "香港 01", "all": ["香港 01", "日本 02"]},
            "节点选择": {"type": "Selector", "now": "自动选择", "all": ["自动选择", "香港 01", "日本 02"]},
            "香港 01": {"type": "Vmess", "history": [{"delay": 40}]},
            "日本 02": {"type": "Trojan", "history": [{"delay": 50}]}
        })
        .as_object()
        .unwrap()
        .clone();

        let payload = build_proxies(&map);
        let nodes = payload["nodes"].as_array().unwrap();
        let hk = nodes.iter().find(|n| n["name"] == "香港 01").unwrap();
        assert_eq!(
            hk["group"], "节点选择",
            "GLOBAL must not win the primary-group election"
        );
        assert_eq!(hk["groups"].as_array().unwrap().len(), 3);

        // Resolving an auto group prefers the routing selector over GLOBAL.
        assert_eq!(
            resolve_selector(&map, "自动选择", "日本 02").unwrap(),
            ("节点选择".to_string(), true)
        );
        // Explicitly addressing GLOBAL still works (mode: global).
        assert_eq!(
            resolve_selector(&map, "GLOBAL", "日本 02").unwrap(),
            ("GLOBAL".to_string(), false)
        );

        // The dashboard summary drops the synthetic group.
        let summary = groups_summary_from(&json!({"proxies": map}));
        assert_eq!(summary["count"], 2);
    }

    #[test]
    fn account_info_placeholders_are_not_nodes() {
        let map = json!({
            "节点选择": {
                "type": "Selector",
                "now": "香港 01",
                "all": ["香港 01", "剩余流量：80% 100GB", "过期时间：2026-12-18"]
            },
            "香港 01": {"type": "Vmess"},
            "剩余流量：80% 100GB": {"type": "Shadowsocks"},
            "过期时间：2026-12-18": {"type": "Shadowsocks"}
        })
        .as_object()
        .unwrap()
        .clone();
        let payload = build_proxies(&map);
        assert_eq!(payload["groups"][0]["count"], 1);
        let names: Vec<&str> = payload["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["香港 01"]);
    }

    #[tokio::test]
    async fn static_files_cannot_escape_the_ui_root() {
        let dir = std::env::temp_dir()
            .join(format!("flclash-ui-{}", crate::events::now_millis()));
        std::fs::create_dir_all(dir.join("css")).unwrap();
        std::fs::write(dir.join("css/app.css"), "body{}").unwrap();
        let st = AppState::new(
            "127.0.0.1:9".into(),
            dir.to_string_lossy().to_string(),
            dir.to_string_lossy().to_string(),
            "/bin/true".into(),
            None,
        );
        assert_eq!(
            serve_file(&st, "css/app.css").await.status(),
            StatusCode::OK
        );
        // axum percent-decodes the wildcard capture, so an absolute path
        // (`%2Fetc%2Fpasswd`) used to replace the UI root entirely.
        assert_eq!(
            serve_file(&st, "/etc/passwd").await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            serve_file(&st, "../../etc/passwd").await.status(),
            StatusCode::NOT_FOUND
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
