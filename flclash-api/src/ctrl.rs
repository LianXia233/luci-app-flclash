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

/// Downloads a URL through curl and returns (headers, body). TLS stays out of
/// the binary: the router ships curl and it already trusts the system CA set.
pub async fn curl_fetch(url: &str, timeout_secs: u64) -> Result<(Vec<String>, Vec<u8>), String> {
    let output = tokio::process::Command::new("curl")
        .args([
            "-sS", "-i", "-L",
            "-m", &timeout_secs.to_string(),
            "-A", "FlClash-OpenWrt/1.2 (luci-app-flclash)",
            url,
        ])
        .output()
        .await
        .map_err(|e| format!("curl: {}", e))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl: {}", err.trim()));
    }
    let raw = output.stdout;
    // split headers/body at the first CRLFCRLF (or LFLF)
    for sep in [b"\r\n\r\n".as_slice(), b"\n\n".as_slice()] {
        if let Some(pos) = find_subslice(&raw, sep) {
            let head = String::from_utf8_lossy(&raw[..pos]).into_owned();
            let headers: Vec<String> = head.lines().map(|l| l.to_string()).collect();
            let body = raw[pos + sep.len()..].to_vec();
            return Ok((headers, body));
        }
    }
    Ok((Vec::new(), raw))
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

/// Runs a short curl fetch whose body is JSON.
pub async fn curl_json<T: serde::de::DeserializeOwned>(url: &str, timeout_secs: u64) -> Result<T, String> {
    let (_headers, body) = curl_fetch(url, timeout_secs).await?;
    serde_json::from_slice(&body).map_err(|e| format!("bad json from {}: {}", url, e))
}
