//! WebSocket transport: one connection manager, per-connection subscription
//! filters, sequence/gap recovery, heartbeat, protocol negotiation and
//! optional application-level gzip.
//!
//! Protocol (JSON envelopes, see `events.rs`):
//!
//! ```text
//! server → hello        {protocol, authRequired, topics, heartbeatSeconds, compression}
//! client → auth         {secret}                     (only when authRequired)
//! client → hello        {protocol, capabilities:[…], resume?:<sequence>}
//! server → snapshot     {sequence, status, proxies, profiles, tasks, capabilities}
//! client → subscribe    {topics:[…]}   server → subscribed {topics}
//! client → unsubscribe  {topics:[…]}   server → unsubscribed {topics}
//! client → sync                          server → snapshot
//! client → resume       {sequence}       server → snapshot | resumed
//! client → ack          {sequence}
//! server → ping (20s)   client → pong
//! ```

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::{interval, MissedTickBehavior};

use crate::api;
use crate::events::{self, Envelope, PROTOCOL_VERSION, TOPICS};
use crate::state::AppState;

pub const HEARTBEAT_SECS: u64 = 20;
/// A client that stops answering pings for this long is dropped.
pub const IDLE_TIMEOUT_SECS: u64 = 70;
/// Any single client frame larger than this is rejected.
pub const MAX_CLIENT_MESSAGE: usize = 8 * 1024;
/// Frames larger than this are gzip+base64 encoded when negotiated.
pub const COMPRESS_THRESHOLD: usize = 4096;
/// Handshake (auth + hello) budget.
const HANDSHAKE_SECS: u64 = 8;
/// Client hello budget once authenticated (browser sends it immediately).
const HELLO_SECS: u64 = 2;
/// Inbound message budget per second (protocol abuse guard).
const MAX_MESSAGES_PER_SEC: u32 = 120;

struct Conn {
    /// Always subscribed to `system` and `status`.
    subs: HashSet<String>,
    gzip: bool,
    last_ack: u64,
    last_topic_seq: HashMap<String, u64>,
    last_activity: Instant,
    window_start: Instant,
    window_count: u32,
}

impl Conn {
    fn new() -> Self {
        let mut subs = HashSet::new();
        subs.insert("system".to_string());
        subs.insert("status".to_string());
        Self {
            subs,
            gzip: false,
            last_ack: 0,
            last_topic_seq: HashMap::new(),
            last_activity: Instant::now(),
            window_start: Instant::now(),
            window_count: 0,
        }
    }

    fn allow_message(&mut self) -> bool {
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.window_count = 0;
        }
        self.window_count += 1;
        self.window_count <= MAX_MESSAGES_PER_SEC
    }
}

/// Same-origin check: browsers send `Origin`; a foreign origin must not be
/// able to open a privileged socket against the router.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        // Non-browser clients (tests, curl) do not send Origin.
        return true;
    };
    let Some(host) = headers.get("host").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let origin_host = origin
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split('/')
        .next()
        .unwrap_or("");
    origin_host.eq_ignore_ascii_case(host)
}

fn header_secret(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-flclash-secret")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
        })
}

pub async fn ws_handler(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&headers) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({"error": "origin not allowed"})),
        )
            .into_response();
    }
    // Header/query auth is accepted for non-browser clients; browsers cannot
    // set headers on WebSocket, so they authenticate with an `auth` message.
    let pre_auth = match &st.secret {
        None => true,
        Some(secret) => {
            header_secret(&headers).as_deref() == Some(secret.as_str())
                || query.get("secret").map(|v| v.as_str()) == Some(secret.as_str())
        }
    };
    ws.max_message_size(MAX_CLIENT_MESSAGE)
        .max_frame_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| serve(socket, st, pre_auth))
        .into_response()
}

type Sink = SplitSink<WebSocket, Message>;

async fn send_frame(sink: &mut Sink, envelope: &Envelope, gzip: bool) -> Result<(), ()> {
    let text = if gzip {
        let raw = serde_json::to_string(&envelope.data).unwrap_or_default();
        if raw.len() >= COMPRESS_THRESHOLD {
            match gzip_base64(&raw) {
                Ok(payload) => {
                    let mut wire = envelope.clone();
                    wire.encoding = Some("gzip+base64".into());
                    wire.data = Value::String(payload);
                    events::encode(&wire)
                }
                Err(_) => events::encode(envelope),
            }
        } else {
            events::encode(envelope)
        }
    } else {
        events::encode(envelope)
    };
    sink.send(Message::Text(text.into())).await.map_err(|_| ())
}

fn gzip_base64(raw: &str) -> Result<String, String> {
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(raw.as_bytes())
        .map_err(|e| e.to_string())?;
    let buffer = encoder.finish().map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::STANDARD.encode(buffer))
}

async fn close(sink: &mut Sink, code: u16, reason: &str) {
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })))
        .await;
}

pub async fn serve(socket: WebSocket, st: Arc<AppState>, pre_auth: bool) {
    let (mut sink, mut stream) = socket.split();
    let mut conn = Conn::new();
    let mut authed = pre_auth;

    let hello = Envelope::local(
        "hello",
        "system",
        json!({
            "protocol": PROTOCOL_VERSION,
            "server": {
                "version": env!("CARGO_PKG_VERSION"),
                "bootId": st.boot_id,
            },
            "authRequired": !authed,
            "topics": TOPICS,
            "heartbeatSeconds": HEARTBEAT_SECS,
            "compression": ["gzip+base64"],
            "sequence": st.bus.last_sequence(),
        }),
        st.bus.last_sequence(),
    );
    if send_frame(&mut sink, &hello, false).await.is_err() {
        return;
    }

    // ---- handshake: auth (if required) + client hello -------------------
    let handshake_start = Instant::now();
    let mut hello_seen = false;
    loop {
        if authed && hello_seen {
            break;
        }
        let budget = if authed {
            Duration::from_secs(HELLO_SECS)
        } else {
            Duration::from_secs(HANDSHAKE_SECS)
        };
        let remaining = budget.saturating_sub(handshake_start.elapsed());
        if remaining.is_zero() {
            break;
        }
        let next = tokio::time::timeout(remaining, stream.next()).await;
        let message = match next {
            Err(_) => break,
            Ok(None) => return,
            Ok(Some(Err(_))) => return,
            Ok(Some(Ok(message))) => message,
        };
        let Some(text) = text_of(message) else {
            continue;
        };
        let Some(msg) = parse_json(&text) else {
            let _ = send_frame(
                &mut sink,
                &Envelope::local(
                    "protocol.error",
                    "system",
                    json!({"message": "invalid json"}),
                    st.bus.last_sequence(),
                ),
                false,
            )
            .await;
            continue;
        };
        let kind = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match kind {
            "auth" => {
                let provided = msg.get("secret").and_then(|v| v.as_str()).unwrap_or("");
                match &st.secret {
                    None => authed = true,
                    Some(secret) if provided == secret => authed = true,
                    Some(_) => {
                        close(&mut sink, 4401, "unauthorized").await;
                        return;
                    }
                }
            }
            "hello" => {
                if let Some(reason) = negotiate_protocol(&msg) {
                    let _ = send_frame(
                        &mut sink,
                        &Envelope::local(
                            "protocol.error",
                            "system",
                            json!({"message": reason}),
                            st.bus.last_sequence(),
                        ),
                        false,
                    )
                    .await;
                    close(&mut sink, 4400, "protocol mismatch").await;
                    return;
                }
                conn.gzip = msg
                    .get("capabilities")
                    .and_then(|v| v.as_array())
                    .map(|caps| caps.iter().any(|c| c.as_str() == Some("gzip+base64")))
                    .unwrap_or(false);
                // `resume` is advisory: the snapshot below already carries
                // the truth, so a client never resumes into a half-state.
                hello_seen = true;
                if !authed {
                    let _ = send_frame(
                        &mut sink,
                        &Envelope::local(
                            "auth.required",
                            "system",
                            json!({}),
                            st.bus.last_sequence(),
                        ),
                        false,
                    )
                    .await;
                }
            }
            "ping" => {
                let _ = send_frame(
                    &mut sink,
                    &Envelope::local("pong", "system", json!({}), st.bus.last_sequence()),
                    conn.gzip,
                )
                .await;
            }
            _ => {
                if !authed {
                    let _ = send_frame(
                        &mut sink,
                        &Envelope::local(
                            "auth.required",
                            "system",
                            json!({}),
                            st.bus.last_sequence(),
                        ),
                        false,
                    )
                    .await;
                }
            }
        }
    }

    if !authed {
        close(&mut sink, 4401, "unauthorized").await;
        return;
    }

    // ---- initial snapshot -------------------------------------------------
    let snapshot = api::snapshot_payload(&st).await;
    let snapshot_env = Envelope::local("snapshot", "system", snapshot, st.bus.last_sequence());
    if send_frame(&mut sink, &snapshot_env, conn.gzip)
        .await
        .is_err()
    {
        return;
    }

    let clients = st.ws_clients.fetch_add(1, Ordering::SeqCst) + 1;
    st.bus.publish(
        "system",
        "system.clients",
        json!({"clients": clients, "bootId": st.boot_id}),
    );

    let mut rx = st.bus.subscribe();
    let mut ticker = interval(Duration::from_secs(HEARTBEAT_SECS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick

    'session: loop {
        tokio::select! {
            biased;
            incoming = stream.next() => {
                let message = match incoming {
                    None => break,
                    Some(Err(_)) => break,
                    Some(Ok(message)) => message,
                };
                match message {
                    Message::Close(_) => break,
                    Message::Ping(payload) => {
                        if sink.send(Message::Pong(payload)).await.is_err() { break; }
                        conn.last_activity = Instant::now();
                    }
                    Message::Pong(_) => { conn.last_activity = Instant::now(); }
                    Message::Binary(_) => {
                        let _ = send_frame(&mut sink, &Envelope::local("protocol.error", "system",
                            json!({"message": "binary frames are not supported"}), st.bus.last_sequence()), conn.gzip).await;
                    }
                    Message::Text(text) => {
                        conn.last_activity = Instant::now();
                        if !conn.allow_message() {
                            close(&mut sink, 4029, "rate limit").await;
                            break;
                        }
                        let Some(msg) = parse_json(text.as_str()) else {
                            let _ = send_frame(&mut sink, &Envelope::local("protocol.error", "system",
                                json!({"message": "invalid json"}), st.bus.last_sequence()), conn.gzip).await;
                            continue;
                        };
                        match client_command(&msg) {
                            ClientCommand::Sync => {
                                let snapshot = api::snapshot_payload(&st).await;
                                let env = Envelope::local("snapshot", "system", snapshot, st.bus.last_sequence());
                                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            }
                            ClientCommand::Resume(sequence) => {
                                let latest = st.bus.last_sequence();
                                if sequence + 1 >= latest {
                                    let env = Envelope::local("resumed", "system",
                                        json!({"from": sequence, "sequence": latest}), latest);
                                    let _ = send_frame(&mut sink, &env, conn.gzip).await;
                                } else {
                                    let snapshot = api::snapshot_payload(&st).await;
                                    let mut env = Envelope::local("snapshot", "system", snapshot, latest);
                                    env.data["reason"] = json!("gap");
                                    env.data["from"] = json!(sequence);
                                    if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                                }
                                conn.last_topic_seq.clear();
                            }
                            ClientCommand::Subscribe(topics) => {
                                for topic in &topics { conn.subs.insert(topic.clone()); }
                                let env = Envelope::local("subscribed", "system", json!({"topics": topics}), st.bus.last_sequence());
                                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            }
                            ClientCommand::Unsubscribe(topics) => {
                                for topic in &topics {
                                    if topic == "system" || topic == "status" { continue; }
                                    conn.subs.remove(topic);
                                }
                                let env = Envelope::local("unsubscribed", "system", json!({"topics": topics}), st.bus.last_sequence());
                                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            }
                            ClientCommand::Ack(sequence) => { conn.last_ack = sequence; }
                            ClientCommand::Ping => {
                                let env = Envelope::local("pong", "system", json!({"ts": events::now_millis()}), st.bus.last_sequence());
                                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            }
                            ClientCommand::Ignore => {}
                            ClientCommand::Error(message) => {
                                let env = Envelope::local("error", "system", json!({"message": message}), st.bus.last_sequence());
                                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            }
                        }
                    }
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if !conn.subs.contains(event.topic.as_str()) { continue; }
                        // Per-topic gap detection: a lost frame means the
                        // client's incremental view is unusable → snapshot.
                        let expected = conn.last_topic_seq.get(&event.topic).copied().unwrap_or(0) + 1;
                        let gap = event.topic_seq != expected;
                        conn.last_topic_seq.insert(event.topic.clone(), event.topic_seq);
                        if gap {
                            let notice = Envelope::local("resync", "system", json!({
                                "reason": "gap",
                                "topic": event.topic,
                                "expected": expected,
                                "received": event.topic_seq,
                            }), event.sequence);
                            let _ = send_frame(&mut sink, &notice, conn.gzip).await;
                            let snapshot = api::snapshot_payload(&st).await;
                            let env = Envelope::local("snapshot", "system", snapshot, st.bus.last_sequence());
                            if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                            conn.last_topic_seq.clear();
                            continue;
                        }
                        if send_frame(&mut sink, &event, conn.gzip).await.is_err() { break 'session; }
                    }
                    Err(RecvError::Lagged(missed)) => {
                        let notice = Envelope::local("resync", "system", json!({"reason": "lagged", "missed": missed}), st.bus.last_sequence());
                        let _ = send_frame(&mut sink, &notice, conn.gzip).await;
                        let snapshot = api::snapshot_payload(&st).await;
                        let env = Envelope::local("snapshot", "system", snapshot, st.bus.last_sequence());
                        if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
                        conn.last_topic_seq.clear();
                    }
                    Err(RecvError::Closed) => break 'session,
                }
            }
            _ = ticker.tick() => {
                if conn.last_activity.elapsed() > Duration::from_secs(IDLE_TIMEOUT_SECS) {
                    close(&mut sink, 4008, "idle timeout").await;
                    break 'session;
                }
                let env = Envelope::local("ping", "system", json!({"ts": events::now_millis(), "lastAck": conn.last_ack}), st.bus.last_sequence());
                if send_frame(&mut sink, &env, conn.gzip).await.is_err() { break 'session; }
            }
        }
    }

    let remaining = st
        .ws_clients
        .fetch_sub(1, Ordering::SeqCst)
        .saturating_sub(1);
    st.bus.publish(
        "system",
        "system.clients",
        json!({"clients": remaining, "bootId": st.boot_id}),
    );
}

fn text_of(message: Message) -> Option<String> {
    match message {
        Message::Text(text) => Some(text.to_string()),
        _ => None,
    }
}

fn parse_json(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok()
}

/// Returns Some(reason) when the client protocol cannot be served.
fn negotiate_protocol(msg: &Value) -> Option<String> {
    match msg.get("protocol").and_then(|v| v.as_u64()) {
        None => None,
        Some(version) if version == PROTOCOL_VERSION as u64 => None,
        Some(version) => Some(format!(
            "unsupported protocol {} (server speaks {})",
            version, PROTOCOL_VERSION
        )),
    }
}

enum ClientCommand {
    Sync,
    Resume(u64),
    Subscribe(Vec<String>),
    Unsubscribe(Vec<String>),
    Ack(u64),
    Ping,
    Ignore,
    Error(String),
}

fn client_command(msg: &Value) -> ClientCommand {
    let kind = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "sync" => ClientCommand::Sync,
        "resume" => match msg.get("sequence").and_then(|v| v.as_u64()) {
            Some(sequence) => ClientCommand::Resume(sequence),
            None => ClientCommand::Error("resume requires sequence".into()),
        },
        "subscribe" | "unsubscribe" => {
            match events::parse_topics(msg.get("topics").unwrap_or(&Value::Null)) {
                Ok(topics) if topics.is_empty() => {
                    ClientCommand::Error("topics must not be empty".into())
                }
                Ok(topics) if kind == "subscribe" => ClientCommand::Subscribe(topics),
                Ok(topics) => ClientCommand::Unsubscribe(topics),
                Err(error) => ClientCommand::Error(error),
            }
        }
        "ack" => match msg.get("sequence").and_then(|v| v.as_u64()) {
            Some(sequence) => ClientCommand::Ack(sequence),
            None => ClientCommand::Error("ack requires sequence".into()),
        },
        "ping" => ClientCommand::Ping,
        _ => ClientCommand::Ignore,
    }
}

#[path = "ws_tests.rs"]
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) fn test_router(st: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .with_state(st)
}
