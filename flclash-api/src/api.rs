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
use crate::{profiles, state::MAX_BODY};

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

/// GET /proxies map → {groups, nodes}, mirroring proxiesPayload().
pub async fn proxies_payload(st: &AppState) -> Result<Value, String> {
    let data = ctrl_json(st, "GET", "/proxies", None, Duration::from_secs(10)).await?;
    let empty = Vec::new();
    let proxies = data
        .get("proxies")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "bad proxies payload".to_string())?;

    let mut groups: Vec<Value> = Vec::new();
    let mut nodes: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    for (name, proxy) in proxies {
        let all = proxy.get("all").and_then(|v| v.as_array()).unwrap_or(&empty);
        if all.is_empty() {
            continue;
        }
        groups.push(json!({
            "name": name,
            "type": proxy.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
            "now": proxy.get("now").and_then(|v| v.as_str()).unwrap_or(""),
            "count": all.len(),
        }));
        for member in all {
            let member_name = member.as_str().unwrap_or("");
            let item = match proxies.get(member_name) {
                Some(item) => item,
                None => continue,
            };
            match nodes.get_mut(member_name) {
                Some(existing) => {
                    if let Some(list) = existing.get_mut("groups").and_then(|v| v.as_array_mut()) {
                        list.push(Value::String(name.clone()));
                    }
                }
                None => {
                    nodes.insert(
                        member_name.to_string(),
                        json!({
                            "name": member_name,
                            "type": item.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
                            "delay": proxy_delay(item),
                            "group": name,
                            "groups": [name],
                        }),
                    );
                }
            }
        }
    }
    groups.sort_by_key(|g| g.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
    Ok(json!({
        "groups": groups,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
    }))
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

async fn connections_payload(st: &AppState) -> Value {
    let data = match ctrl_json(
        st,
        "GET",
        "/connections",
        None,
        Duration::from_secs(10),
    )
    .await
    {
        Ok(data) => data,
        Err(_) => return json!([]),
    };
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

    let out: Vec<Value> = conns
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
        .collect();
    json!(out)
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
            let all = proxy.get("all").and_then(|v| v.as_array()).unwrap_or(&empty);
            if all.is_empty() {
                continue;
            }
            let delay = proxy_delay(proxy);
            if delay == 0 {
                timeout += 1;
            }
            groups.push(json!({
                "name": name,
                "type": proxy.get("type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase(),
                "now": proxy.get("now").and_then(|v| v.as_str()).unwrap_or(""),
                "count": all.len(),
            }));
        }
    }
    groups.sort_by_key(|g| g.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string());
    json!({"count": groups.len(), "timeout": timeout, "groups": groups})
}

// ------------------------------------------------------------------ handlers

pub async fn status(State(st): State<Arc<AppState>>) -> Response {
    let reachable = core_reachable(&st).await;
    let mut running = false;
    let mut version = String::new();
    let mut cfg = file_config(&st.config_path());
    let mut groups_summary = json!({"count": 0, "timeout": 0, "groups": []});
    let mut stats_conns = 0usize;

    if reachable {
        running = true;
        if let Ok(data) = ctrl_json(&st, "GET", "/configs", None, Duration::from_secs(5)).await {
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
        if let Ok(data) = ctrl_json(&st, "GET", "/version", None, Duration::from_secs(5)).await {
            version = data
                .get("version")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
        if let Ok(data) = ctrl_json(&st, "GET", "/proxies", None, Duration::from_secs(10)).await {
            groups_summary = groups_summary_from(&data);
        }
        if let Ok(data) = ctrl_json(&st, "GET", "/connections", None, Duration::from_secs(10)).await {
            stats_conns = data
                .get("connections")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
        }
    }

    let hist = st.hist.lock().unwrap();
    let up_rate = hist.up.last().copied().unwrap_or(0.0);
    let down_rate = hist.down.last().copied().unwrap_or(0.0);
    let pid = core_pid();
    let core_rss = if pid != 0 { rss_bytes(pid) } else { 0.0 };
    let profile = profiles::load_profiles(&st)
        .iter()
        .find(|p| p.active)
        .map(|p| profiles::profile_payload(&st, p))
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
        "enhanced-mode": if cfg.dns_mode.is_empty() { "fake-ip" } else { cfg.dns_mode.as_str() },
        "nameserver": cfg.dns_nameservers,
        "default-nameserver": cfg.dns_default_nameservers,
        "fake-ip-range": if cfg.dns_fake_ip_range.is_empty() { "198.18.0.1/16" } else { cfg.dns_fake_ip_range.as_str() },
        "fake-ip-filter": cfg.dns_fake_ip_filter,
    });

    ok_json(json!({
        "running": running,
        "version": version,
        "coreVersion": version,
        "uptime": if pid != 0 { core_uptime(pid) } else { 0.0 },
        "mode": cfg.mode,
        "outboundIp": hist.outbound_ip,
        "countryCode": hist.outbound_cc,
        "organization": hist.outbound_org,
        "ipInfo": ipinfo,
        "quality": quality,
        "localIp": local_ipv4(),
        "mixedPort": cfg.mixed_port,
        "socksPort": cfg.socks_port,
        "httpPort": cfg.port,
        "systemProxy": false,
        "tun": cfg.tun,
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
    }))
}

pub async fn version(State(st): State<Arc<AppState>>) -> Response {
    match req(&st, "GET", "/version").await {
        Ok(reply) => ok_raw(reply.body),
        Err(_) => ok_json(json!({"version": "", "meta": false, "running": false})),
    }
}

pub async fn memory(State(st): State<Arc<AppState>>) -> Response {
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
        "logLevel": live.log_level,
        "systemProxy": false,
    }))
}

pub async fn config_put(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let patch = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match save_config(&st, &patch).await {
        Ok(()) => ok_json(json!({"ok": true})),
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

pub async fn proxies_select(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let group = data.get("group").and_then(|v| v.as_str()).unwrap_or("");
    let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if group.is_empty() || name.is_empty() {
        return fail_json(StatusCode::BAD_REQUEST, "group and name are required");
    }
    if !core_reachable(&st).await {
        return core_down();
    }
    let path = format!("/proxies/{}", urlencode(group));
    match req_body(&st, "PUT", &path, &json!({"name": name}).to_string()).await {
        Ok(reply) if reply.status < 400 => ok_json(json!({"ok": true})),
        Ok(reply) => bad_gateway(&format!("controller: {}", String::from_utf8_lossy(&reply.body).trim())),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn proxies_delay(State(st): State<Arc<AppState>>, body: Bytes) -> Response {
    let data = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");
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
            ok_json(json!({"name": name, "value": value}))
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
        Ok(reply) => ok_raw(reply.body),
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
    ok_json(json!({
        "backend": "flclash",
        "mixed_port": cfg.mixed_port,
        "clash_api_port": 9097,
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
    match profiles::import_profile(&st, &url).await {
        Ok(item) => ok_json(profiles::profile_payload(&st, &item)),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn profiles_apply(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match profiles::apply_profile(&st, &id).await {
        Ok(()) => ok_json(json!({"ok": true, "id": id})),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn profiles_update(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match profiles::update_profile(&st, &id).await {
        Ok(result) => ok_json(json!({"ok": true, "id": id, "result": result})),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn profiles_edit(State(st): State<Arc<AppState>>, Path(id): Path<String>, body: Bytes) -> Response {
    let patch = match body_json(&body).await {
        Ok(v) => v,
        Err(resp) => return resp,
    };
    match profiles::patch_profile(&st, &id, &patch) {
        Ok(()) => ok_json(json!({"ok": true, "id": id})),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn profiles_delete(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match profiles::delete_profile(&st, &id) {
        Ok(()) => ok_json(json!({"ok": true, "id": id})),
        Err(err) => bad_gateway(&err),
    }
}

pub async fn service_action(
    State(st): State<Arc<AppState>>,
    Path(action): Path<String>,
) -> Response {
    match profiles::service(&st, &action).await {
        Ok(()) => ok_json(json!({"ok": true, "action": action})),
        Err(err) => bad_gateway(&err),
    }
}

// ------------------------------------------------------------- static assets

pub async fn static_index() -> Redirect {
    Redirect::to("/flclash/")
}

pub async fn static_root(State(st): State<Arc<AppState>>) -> Response {
    serve_file(&st, "index.html")
}

pub async fn static_file(State(st): State<Arc<AppState>>, Path(path): Path<String>) -> Response {
    serve_file(&st, &path)
}

fn serve_file(st: &Arc<AppState>, rel: &str) -> Response {
    if rel.contains("..") {
        return fail_json(StatusCode::NOT_FOUND, "not found");
    }
    let target = std::path::Path::new(&st.ui_dir).join(rel);
    match std::fs::read(&target) {
        Ok(bytes) => {
            let mime = mime_of(&rel);
            (
                [(
                    axum::http::header::CONTENT_TYPE,
                    format!("{}; charset=utf-8", mime),
                )],
                bytes,
            )
                .into_response()
        }
        Err(_) => Html("<h1>404</h1>".to_string()).into_response(),
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
