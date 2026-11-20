//! Outbound IP profile lookup — a self-contained module.
//!
//! Sources (user preference order):
//!   0. Cloudflare cdn-cgi/trace — always answers, reports exactly what the
//!      takeover's egress path sees (ip/loc/colo/warp).
//!   1. ip.skk.moe cf-geoip — keyless CF-fronted geo (region/city/asn/asOrg).
//!   2. ipfinal.com card SVG — zh locale "IP · 国家 · 省 · 市 · ASN".
//!   3. api.ip.sb / ipwho.is — ISP/ASN enrichment.
//!   4. ip-api.com — last resort (HTTP-only, flaky from CN).
//!
//! Geo databases fill unknown owners with placeholders ("Private Customer",
//! bare RIR names); those never reach the dashboard — without a real owner
//! the display falls back to a zh region line or country/city.
//!
//! The lookup refreshes on a schedule AND immediately when the core starts or
//! stops: the takeover routes the router's own traffic through the core, so
//! that transition changes the egress path (proxy exit <-> direct WAN IP) and
//! the cached profile goes stale. The stopped state therefore keeps live,
//! correct IP info.

use std::time::Duration;

use crate::ctrl::{curl_fetch, curl_json};
use crate::state::AppState;

/// Cheap liveness probe for the core controller port: the takeover routes
/// the router's own traffic through the core, so core start/stop changes the
/// egress path (proxy exit <-> direct WAN IP) and the cached IP profile.
async fn core_controller_up(st: &AppState) -> bool {
    match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(st.controller.as_str()),
    )
    .await
    {
        Ok(Ok(_)) => true,
        _ => false,
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
    let (_headers, body) = curl_fetch(
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

/// True when the string carries CJK ideographs — the "IP 资料 is localized
/// where possible" gate.
fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c))
}

/// Geo databases fill unknown owners with generic placeholders ("Private
/// Customer", bare RIR names, ...). Showing those as the IP owner is
/// meaningless — treat them as missing so the fallback chain keeps going.
fn junk_org(s: &str) -> bool {    let t = s.trim().to_ascii_lowercase();
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

/// Runs one full lookup round and stores the result in the app state.
/// Returns whether any source produced an IP.
async fn lookup_once(st: &AppState) -> bool {
    let mut info = crate::state::IpInfo::default();
    let mut ip6 = String::new();
    let mut ok = false;

    // Source 0a: Cloudflare trace over the v4-forced client (ip/loc/colo/warp).
    if let Ok((_headers, body)) =
        curl_fetch("https://www.cloudflare.com/cdn-cgi/trace", 8).await
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
            // The node egressed v6: park the address for the v6 display and
            // require a separate v4 sample below (the takeover semantics and
            // most site traffic are v4).
            if info.ip.contains(':') {
                ip6 = info.ip.clone();
                info.ip.clear();
                ok = false;
            }
        }
    }
    // Source 0b: the v6 side, sampled independently through a CF anycast v6
    // pin (no DNS involved — works even with dns.ipv6 filtering on).
    if let Ok(body) = crate::ctrl::cf_trace_v6(8).await {
        let text = String::from_utf8_lossy(&body);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("ip=") {
                let v = rest.trim();
                if v.contains(':') {
                    ip6 = v.to_string();
                }
            }
        }
    }
    if !ok {
        // The node answered over v6 (or the trace failed): sample the v4
        // exit via the A-record-only host so both families get covered.
        if let Ok(v) = curl_json::<serde_json::Value>("https://api-ipv4.ip.sb/geoip", 8).await {
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
            }
            if !junk_org(isp) {
                info.isp = isp.to_string();
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
            }
            if !junk_org(&isp) {
                info.isp = isp;
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

    // Localization pass (user preference: 汉化 where possible): the geo DBs
    // answer English owner names; ip-api.com (zh-CN) carries Chinese ISP/org
    // for most networks. Only consulted when the display has no CJK yet.
    if ok && !has_cjk(&info.isp) && !has_cjk(&info.org) {        if let Ok(v) = curl_json::<serde_json::Value>(
            "http://ip-api.com/json/?fields=status,isp,org,as&lang=zh-CN",
            8,
        )
        .await
        {
            let isp = v.get("isp").and_then(|v| v.as_str()).unwrap_or("");
            let org = v.get("org").and_then(|v| v.as_str()).unwrap_or("");
            if has_cjk(isp) {
                info.isp = isp.to_string();
            }
            if has_cjk(org) {
                info.org = org.to_string();
            }
        }
    }
    // Sources like skk.moe only answer an owner (asOrg); mirror it into isp
    // so display and quality consumers never see one without the other.
    if ok && info.isp.trim().is_empty() && !junk_org(&info.org) {
        info.isp = info.org.clone();
    }
    let succeeded = ok;
    {
        let mut hist = st.hist.lock().unwrap();
        if ok {
            hist.outbound_ip = info.ip.clone();
            hist.outbound_ip6 = ip6;
            hist.outbound_cc = info.country_code.clone();
            // IP 资料 display: owner first (prefer a Chinese name when we
            // have one), then the zh region line (香港 九龙 · AS141031
            // style), then country/city. Never surface the geo databases'
            // "Private Customer" placeholders.
            let display = [info.isp.as_str(), info.org.as_str()]
                .into_iter()
                .find(|s| !junk_org(s) && has_cjk(s))
                .or_else(|| {
                    [info.isp.as_str(), info.org.as_str()]
                        .into_iter()
                        .find(|s| !junk_org(s))
                })
                .map(|s| s.to_string())
                .unwrap_or_default();
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
    succeeded
}

/// Waits until the takeover state file reflects the post-transition reality
/// ("on"/"tun" after a core start, "off" after a stop) so the IP sample is
/// taken against the NEW egress path, or 20s elapse.
async fn wait_takeover_settled(up: bool) {
    let mut waited = 0;
    while waited < 20 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        waited += 2;
        if let Ok(s) = std::fs::read_to_string("/tmp/flclash-proxy.state") {
            let t = s.trim();
            if up && (t == "on" || t == "tun") {
                // Rules armed; give the first proxied connections a beat.
                tokio::time::sleep(Duration::from_secs(2)).await;
                return;
            }
            if !up && t == "off" {
                return;
            }
        }
    }
}

/// Caches the outbound IP profile (country/city/ISP/ASN + quality flags).
pub async fn refresh_outbound(st: std::sync::Arc<AppState>) {
    // None = not sampled yet; first lookup fires immediately.
    let mut last_core_up: Option<bool> = None;
    let mut target_wait = 0u64;
    loop {
        // Sleep toward the next scheduled lookup in 5s ticks, but cut it
        // short when the core starts/stops: the takeover routes the router's
        // own traffic through the core, so the egress path (and the cached
        // IP profile) goes stale the moment that happens — the status card
        // must keep showing live IP info in the stopped state too.
        let mut waited = 0u64;
        while waited < target_wait {
            tokio::time::sleep(Duration::from_secs(5)).await;
            waited += 5;
            let up = core_controller_up(&st).await;
            let prev = last_core_up.replace(up);
            if prev.is_some() && prev != Some(up) {
                // Egress path transitioned (core start/stop): hold the sample
                // until the takeover state settles so the lookup reflects the
                // NEW path instead of racing the rule swap.
                wait_takeover_settled(up).await;
                break;
            }
        }

        // The proxy group (漏网之鱼) url-tests its nodes and sometimes parks
        // on a dead one for a few seconds — a single chain would then time
        // out on every source and keep stale data on the dashboard. Ride out
        // the flap: up to 3 full chain attempts, 3s apart.
        let mut succeeded = false;
        for attempt in 0..3 {
            succeeded = lookup_once(&st).await;
            if succeeded || attempt == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        // Retry quickly after a failure; relax to the normal interval once
        // the lookup succeeds again. Core start/stop cuts the wait short
        // via the transition watch above.
        target_wait = if succeeded { 600 } else { 60 };
    }
}
