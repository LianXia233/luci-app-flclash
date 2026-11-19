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

/// skk.moe cf-geoip: keyless Cloudflare-fronted geo API
/// (ip/country/region/city/asn/asOrg). Verified to answer real data to
/// non-browser clients (unlike its /v1/home endpoints, which easter-egg).
async fn fetch_skk_geoip() -> Option<serde_json::Value> {
    curl_json::<serde_json::Value>("https://ip.api.skk.moe/cf-geoip", 8)
        .await
        .ok()
        .filter(|v| {
            v.get("ip")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        })
}

/// ipfinal.com embed card: api.ipfinal.com serves an SVG whose <title> packs
/// "IP · 国家 · 省 · 市 · ASN" (zh locale). No JSON endpoint exists, so parse
/// the title for CN-granular region info.
async fn fetch_ipfinal_card() -> Option<serde_json::Value> {
    let (_headers, body) = crate::ctrl::curl_fetch(
        "https://api.ipfinal.com/ip-card/card.svg?layout=card&theme=dark&w=480&lang=zh&tags=home",
        8,
    )
    .await
    .ok()?;
    let text = String::from_utf8_lossy(&body);
    let start = text.find("<title>")? + "<title>".len();
    let end = text[start..].find("</title>")? + start;
    let title = &text[start..end];
    let mut parts = title.split('·').map(|s| s.trim().to_string());
    let ip = parts.next()?;
    if ip.is_empty() || !ip.contains('.') {
        return None;
    }
    Some(serde_json::json!({
        "ip": ip,
        "country": parts.next().unwrap_or_default(),
        "region": parts.next().unwrap_or_default(),
        "city": parts.next().unwrap_or_default(),
        "asn": parts.next().unwrap_or_default(),
    }))
}

/// Geo databases fill unknown owners with generic placeholders ("Private
/// Customer", bare RIR names, ...). Showing those as the IP owner is
/// meaningless — treat them as missing so the fallback chain keeps going.
fn junk_org(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    if t.len() < 3 {
        return true;
    }
    const JUNK: [&str; 13] = [
        "private customer",
        "private use",
        "private ip space",
        "private",
        "not routed",
        "unrouted",
        "reserved",
        "iana",
        "apnic",
        "arin",
        "lacnic",
        "ripencc",
        "ripe ncc",
    ];
    JUNK.iter().any(|k| t == *k)
}

/// Caches the outbound IP profile (country/city/ISP/ASN + quality flags).
/// Source order (user preference): Cloudflare edge trace first — it always
/// answers and reports exactly what the takeover's egress path sees — then
/// the geo sources discovered from ip.skk.moe / ipfinal.com, then the
/// classic keyless providers as last resort.
pub async fn refresh_outbound(st: std::sync::Arc<AppState>) {
    loop {
        let mut info = crate::state::IpInfo::default();
        let mut ok = false;
        // Source 0: Cloudflare cdn-cgi/trace (key=value text, no rate limit).
        if let Ok((_headers, body)) =
            crate::ctrl::curl_fetch("https://www.cloudflare.com/cdn-cgi/trace", 8).await
        {
            let text = String::from_utf8_lossy(&body);
            let mut colo = String::new();
            for line in text.lines() {
                let mut kv = line.splitn(2, '=');
                match (kv.next(), kv.next()) {
                    (Some("ip"), Some(v)) => info.ip = v.trim().to_string(),
                    (Some("loc"), Some(v)) => info.country_code = v.trim().to_string(),
                    (Some("colo"), Some(v)) => colo = v.trim().to_string(),
                    (Some("warp"), Some(v)) => info.proxy = v.trim() != "off",
                    _ => {}
                }
            }
            ok = !info.ip.is_empty();
            if ok {
                info.city = colo;
                info.country = info.country_code.clone();
            }
        }
        // Source 1: skk.moe cf-geoip as a full fallback (rich: region/city/asn).
        if !ok {
            if let Some(v) = fetch_skk_geoip().await {
                info.ip = v.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cc = v.get("country").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.country_code = cc.clone();
                info.country = cc;
                info.city = v
                    .get("city")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .or_else(|| v.get("region").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                info.org = v.get("asOrg").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.asn = v
                    .get("asn")
                    .and_then(|v| v.as_u64())
                    .map(|a| format!("AS{}", a))
                    .unwrap_or_default();
                ok = !info.ip.is_empty();
            }
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
        if !ok {
            // ipwho.is: keyless, different network path than the above.
            if let Ok(v) = curl_json::<serde_json::Value>("https://ipwho.is/", 10).await {
                let conn = v.get("connection");
                info.ip = v.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.country = v.get("country").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.country_code = v
                    .get("country_code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                info.city = v.get("city").and_then(|v| v.as_str()).unwrap_or("").to_string();
                info.isp = conn
                    .and_then(|c| c.get("isp"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                info.org = conn
                    .and_then(|c| c.get("org"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                info.asn = conn
                    .and_then(|c| c.get("asn"))
                    .and_then(|v| v.as_u64())
                    .map(|a| format!("AS{}", a))
                    .unwrap_or_default();
                ok = !info.ip.is_empty();
            }
        }
        if !ok {
            // Last resort: ip-api.com (free tier, HTTP only, flaky from CN).
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
        }
        // Enrichment: CF trace (and skk's own db) carry no usable ISP owner
        // for many exits, and some sources answer junk placeholders. Fill
        // org/isp/asn from the discovered sources; junk never counts as data
        // and a failed enrichment never undoes the primary lookup.
        let mut have_org = !junk_org(&info.org) || !junk_org(&info.isp);
        let mut zh_region = String::new();
        if ok && !have_org {
            if let Some(v) = fetch_skk_geoip().await {
                let org = v.get("asOrg").and_then(|v| v.as_str()).unwrap_or("");
                if !junk_org(org) {
                    info.org = org.to_string();
                    have_org = true;
                }
                if info.asn.is_empty() {
                    info.asn = v
                        .get("asn")
                        .and_then(|v| v.as_u64())
                        .map(|a| format!("AS{}", a))
                        .unwrap_or_default();
                }
                if info.city.is_empty() {
                    info.city = v
                        .get("city")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| v.get("region").and_then(|v| v.as_str()))
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
        if ok && !have_org {
            if let Some(v) = fetch_ipfinal_card().await {
                // zh locale: country/region/city read natively (中国 · 广东 · 深圳).
                let zh_country = v.get("country").and_then(|v| v.as_str()).unwrap_or("");
                let region = v.get("region").and_then(|v| v.as_str()).unwrap_or("");
                let city = v.get("city").and_then(|v| v.as_str()).unwrap_or("");
                let mut segs: Vec<&str> = Vec::new();
                if !zh_country.is_empty() {
                    info.country = zh_country.to_string();
                    segs.push(zh_country);
                }
                if !region.is_empty() {
                    segs.push(region);
                }
                if !city.is_empty() && city != region {
                    segs.push(city);
                }
                if !segs.is_empty() {
                    zh_region = segs.join(" ");
                    if info.city.is_empty() {
                        info.city = segs[1..].join(" ");
                    }
                    have_org = true;
                }
                let asn = v.get("asn").and_then(|v| v.as_str()).unwrap_or("");
                if !asn.is_empty() && info.asn.is_empty() && asn.chars().all(|c| c.is_ascii_digit()) {
                    info.asn = format!("AS{}", asn);
                }
            }
        }
        if ok && !have_org {
            if let Ok(v) = curl_json::<serde_json::Value>("https://api.ip.sb/geoip", 8).await {
                let isp = v.get("isp").and_then(|v| v.as_str()).unwrap_or("");
                let org = v
                    .get("asn_organization")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !junk_org(org) {
                    info.org = org.to_string();
                    have_org = true;
                }
                if !junk_org(isp) {
                    info.isp = isp.to_string();
                    have_org = true;
                }
                if info.asn.is_empty() {
                    info.asn = v.get("asn").map(|a| format!("AS{}", a)).unwrap_or_default();
                }
                info.hosting = v.get("hosting").and_then(|v| v.as_bool()).unwrap_or(false);
            } else if let Ok(v) = curl_json::<serde_json::Value>("https://ipwho.is/", 10).await {
                let conn = v.get("connection");
                let isp = conn
                    .and_then(|c| c.get("isp"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let org = conn
                    .and_then(|c| c.get("org"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !junk_org(&org) {
                    info.org = org;
                    have_org = true;
                }
                if !junk_org(&isp) {
                    info.isp = isp;
                    have_org = true;
                }
                if info.asn.is_empty() {
                    info.asn = conn
                        .and_then(|c| c.get("asn"))
                        .and_then(|v| v.as_u64())
                        .map(|a| format!("AS{}", a))
                        .unwrap_or_default();
                }
            }
        }
        let succeeded = ok;
        {
            let mut hist = st.hist.lock().unwrap();
            if ok {
                hist.outbound_ip = info.ip.clone();
                hist.outbound_cc = info.country_code.clone();
                // IP 资料 display: owner first; without one fall back to the
                // zh region line (香港 九龙 style), then country/city. Never
                // surface the geo databases' "Private Customer" placeholders.
                let display = if !junk_org(&info.isp) {
                    info.isp.clone()
                } else if !junk_org(&info.org) {
                    info.org.clone()
                } else {
                    String::new()
                };
                if !display.is_empty() {
                    hist.outbound_org = display;
                } else if !zh_region.is_empty() {
                    hist.outbound_org = if info.asn.is_empty() {
                        zh_region.clone()
                    } else {
                        format!("{} · {}", zh_region, info.asn)
                    };
                } else {
                    let mut segs: Vec<String> = Vec::new();
                    if !info.country.trim().is_empty() {
                        segs.push(info.country.trim().to_string());
                    }
                    if !info.city.trim().is_empty() {
                        segs.push(info.city.trim().to_string());
                    }
                    hist.outbound_org = if segs.is_empty() {
                        "未知".into()
                    } else {
                        segs.join(" · ")
                    };
                }
                hist.outbound_ok = true;
                hist.ipinfo = Some(info);
            } else {
                // Transient upstream failures (rate limits, node switches)
                // must not wipe the last good data from the dashboard: keep
                // the stale org/IP and only flag the quality as unknown.
                hist.outbound_ok = false;
                if hist.outbound_org.is_empty() {
                    hist.outbound_org = "查询失败".into();
                }
            }
        }
        // Retry quickly after a failure; relax to the normal interval once
        // the lookup succeeds again.
        let wait = if succeeded { 600 } else { 60 };
        tokio::time::sleep(Duration::from_secs(wait)).await;
    }
}
