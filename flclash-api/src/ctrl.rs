//! Minimal HTTP/1.1 client for the local mihomo controller plus curl-backed
//! helpers for TLS resources (subscriptions, outbound lookup) and long-lived
//! JSON streams. Everything else in the backend talks plain HTTP on loopback,
//! so no TLS crate is needed and the binary stays statically linkable with
//! rust-lld on musl.

use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct CtrlReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Percent-decodes a string (share-link fragments / userinfo). Note `+` is
/// kept literal: unlike form-encoding, base64 in share links uses `+` as a
/// real character and only `%XX` sequences need unfolding.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Form-encoding variant: `%XX` unfolding plus `+` → space, matching Go's
/// url.QueryUnescape (used for query pairs and share-link names).
pub fn percent_decode_plus(s: &str) -> String {
    percent_decode(&s.replace('+', " "))
}

/// Dechunks a chunked-transfer body.
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    loop {
        let Some(line_end) = find_crlf(data) else { break };
        let size_str = std::str::from_utf8(&data[..line_end]).unwrap_or("");
        let size = match usize::from_str_radix(size_str.trim().split(';').next().unwrap_or(""), 16) {
            Ok(v) => v,
            Err(_) => break,
        };
        data = &data[line_end + 2..];
        if size == 0 {
            break;
        }
        if data.len() < size {
            out.extend_from_slice(data);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        // consume CRLF after chunk
        if data.len() >= 2 && &data[..2] == b"\r\n" {
            data = &data[2..];
        }
    }
    out
}

fn find_crlf(data: &[u8]) -> Option<usize> {
    data.windows(2).position(|w| w == b"\r\n")
}

/// Performs one request against the controller. Reads until EOF because we
/// always send `Connection: close`; handles both identity and chunked bodies.
pub async fn ctrl_req(
    controller: &str,
    secret: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<CtrlReply, String> {
    let fut = ctrl_req_inner(controller, secret, method, path, body);
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| format!("controller {} {}: timeout", method, path))?
}

async fn ctrl_req_inner(
    controller: &str,
    secret: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<CtrlReply, String> {
    let mut req = format!("{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: */*\r\n", method, path, controller);
    if let Some(s) = secret {
        req.push_str(&format!("Authorization: Bearer {}\r\n", s));
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");

    let mut stream = TcpStream::connect(controller)
        .await
        .map_err(|e| format!("dial tcp {}: {}", controller, e))?;
    stream.write_all(req.as_bytes()).await.map_err(|e| e.to_string())?;
    if let Some(b) = body {
        stream.write_all(b.as_bytes()).await.map_err(|e| e.to_string())?;
    }

    let mut raw = Vec::with_capacity(16 * 1024);
    stream.read_to_end(&mut raw).await.map_err(|e| e.to_string())?;
    parse_reply(&raw)
}

pub fn parse_reply(raw: &[u8]) -> Result<CtrlReply, String> {
    // The header/body boundary is CRLFCRLF, not the first CRLF (which only
    // ends the status line). Splitting on the first CRLF glued response
    // headers into the body and broke JSON parsing for every endpoint.
    let split = find_subslice(raw, b"\r\n\r\n")
        .map(|i| i + 4)
        .ok_or_else(|| "malformed controller reply".to_string())?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("bad status line: {}", status_line))?;
    let mut chunked = false;
    for line in lines {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let body_part = &raw[split..];
    let body = if chunked { dechunk(body_part) } else { body_part.to_vec() };
    Ok(CtrlReply { status, body })
}

/// GET a controller path and parse the body as JSON. Mirrors apiJSON().
pub async fn ctrl_json(
    st: &crate::state::AppState,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let reply = ctrl_req(&st.controller, st.secret.as_deref(), method, path, body, timeout).await?;
    if reply.status >= 400 {
        let text = String::from_utf8_lossy(&reply.body);
        return Err(format!("controller {} {}: {}", method, path, text.trim()));
    }
    if reply.body.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&reply.body).map_err(|e| format!("bad controller json: {}", e))
}

/// True when the controller answers a quick /version.
pub async fn core_reachable(st: &crate::state::AppState) -> bool {
    matches!(
        ctrl_req(&st.controller, st.secret.as_deref(), "GET", "/version", None, Duration::from_secs(2)).await,
        Ok(reply) if reply.status < 500
    )
}

/// Global native HTTP client (rustls + webpki-roots baked in): no external
/// curl binary dependency, works for http:// and https:// alike.
/// IPv4-only on purpose: the transparent proxy takeover hooks the v4 path,
/// so queries must follow the actual takeover egress — a happy-eyeballs v6
/// socket would bypass it and always report the router's direct address.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("FlClash-OpenWrt/1.2 (luci-app-flclash)")
            .redirect(reqwest::redirect::Policy::limited(8))
            .local_address(Some(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)))
            .build()
            .expect("reqwest client")
    })
}

/// Hard cap for a downloaded body (subscriptions, outbound lookup). Without it
/// a hostile or broken URL could make the API allocate until the router OOMs.
pub const MAX_FETCH_BYTES: usize = 8 << 20;

/// Fetches a URL natively (TLS via rustls/webpki-roots) and returns
/// (headers, body). Never fails on non-2xx: callers inspect the payload
/// themselves (the profile converter looks at content-type headers).
pub async fn curl_fetch(url: &str, timeout_secs: u64) -> Result<(Vec<String>, Vec<u8>), String> {
    use futures_util::StreamExt;

    let resp = http_client()
        .get(url)
        .timeout(Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| format!("fetch {}: {}", url, e))?;
    if let Some(length) = resp.content_length() {
        if length as usize > MAX_FETCH_BYTES {
            return Err(format!(
                "fetch {}: response too large ({} bytes)",
                url, length
            ));
        }
    }
    let headers: Vec<String> = resp
        .headers()
        .iter()
        .map(|(k, v)| format!("{}: {}", k.as_str(), v.to_str().unwrap_or("<binary>")))
        .collect();
    // Stream instead of `resp.bytes()`: a chunked response has no
    // content-length, so the cap must be enforced while reading.
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("body {}: {}", url, e))?;
        if body.len() + chunk.len() > MAX_FETCH_BYTES {
            return Err(format!(
                "fetch {}: response exceeds {} MiB",
                url,
                MAX_FETCH_BYTES >> 20
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((headers, body))
}

/// Extracts a header value (case-insensitive) from curl-dumped header lines.
pub fn header_value(headers: &[String], name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    headers.iter().find_map(|line| {
        let (k, v) = line.split_once(':')?;
        if k.trim().to_ascii_lowercase() == lower {
            Some(v.trim().to_string())
        } else {
            None
        }
    })
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Fetches the Cloudflare trace over IPv6 by pinning www.cloudflare.com to a
/// CF anycast v6 address — no DNS involved, so it works even when the hijack
/// DNS answers no AAAA (dns.ipv6: false). Returns the raw trace body.
pub async fn cf_trace_v6(timeout_secs: u64) -> Result<Vec<u8>, String> {
    static CLIENT6: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT6.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("FlClash-OpenWrt/1.2 (luci-app-flclash)")
            .redirect(reqwest::redirect::Policy::limited(8))
            // 2606:4700:4700::1111 — Cloudflare anycast v6, stable for years;
            // the cert carries the www.cloudflare.com name so TLS verifies.
            .resolve(
                "www.cloudflare.com",
                std::net::SocketAddr::new(
                    "2606:4700:4700::1111".parse().expect("cf v6 addr"),
                    443,
                ),
            )
            .build()
            .expect("reqwest v6 client")
    });
    let resp = client
        .get("https://www.cloudflare.com/cdn-cgi/trace")
        .timeout(Duration::from_secs(timeout_secs))
        .send()
        .await
        .map_err(|e| format!("cf v6 trace: {}", e))?;
    let body = resp
        .bytes()
        .await
        .map_err(|e| format!("cf v6 trace body: {}", e))?
        .to_vec();
    Ok(body)
}

/// Runs a short curl fetch whose body is JSON.
pub async fn curl_json<T: serde::de::DeserializeOwned>(url: &str, timeout_secs: u64) -> Result<T, String> {
    let (_headers, body) = curl_fetch(url, timeout_secs).await?;
    serde_json::from_slice(&body).map_err(|e| format!("bad json from {}: {}", url, e))
}
