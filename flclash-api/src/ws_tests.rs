//! Transport-level integration tests: a real axum server on a random port,
//! a real WebSocket client, real bus events.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message as Wire;

use super::*;

fn state(secret: Option<String>) -> Arc<AppState> {
    let dir = std::env::temp_dir().join(format!("flclash-ws-test-{}", crate::events::now_millis()));
    let _ = std::fs::create_dir_all(&dir);
    AppState::new(
        "127.0.0.1:9097".into(),
        "/tmp".into(),
        dir.to_string_lossy().to_string(),
        "/bin/true".into(),
        secret,
    )
}

async fn serve(state: Arc<AppState>) -> String {
    let router = super::test_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    format!("ws://{}/ws", addr)
}

async fn next_json<S>(ws: &mut S) -> Value
where
    S: futures_util::Stream<Item = Result<Wire, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = Duration::from_secs(6);
    let frame = tokio::time::timeout(deadline, ws.next())
        .await
        .expect("frame within deadline")
        .expect("stream open")
        .expect("frame ok");
    match frame {
        Wire::Text(text) => {
            let value: Value = serde_json::from_str(text.as_str()).expect("json frame");
            decompress(&value)
        }
        other => panic!("unexpected frame: {:?}", other),
    }
}

fn decompress(value: &Value) -> Value {
    if value.get("encoding").and_then(|v| v.as_str()) == Some("gzip+base64") {
        use std::io::Read;
        let b64 = value.get("data").and_then(|v| v.as_str()).expect("payload");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .expect("base64");
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
        let mut raw = String::new();
        decoder.read_to_string(&mut raw).expect("gunzip");
        let mut cloned = value.clone();
        cloned["data"] = serde_json::from_str(&raw).expect("gzip json");
        return cloned;
    }
    value.clone()
}

/// Reads frames until one of `kind` arrives (system noise may interleave).
async fn next_type<S>(ws: &mut S, kind: &str) -> Value
where
    S: futures_util::Stream<Item = Result<Wire, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    for _ in 0..8 {
        let frame = next_json(ws).await;
        if frame["type"] == kind {
            return frame;
        }
    }
    panic!("frame of type {} not received", kind);
}

#[test]
fn gzip_roundtrip() {
    let raw = json!({"lines": ["a", "b"], "padding": "x".repeat(5000)}).to_string();
    let encoded = gzip_base64(&raw).expect("encode");
    let decoded = decompress(&json!({"encoding": "gzip+base64", "data": encoded}));
    assert_eq!(decoded["data"]["padding"].as_str().unwrap().len(), 5000);
}

#[tokio::test]
async fn hello_snapshot_subscribe_and_event() {
    let st = state(None);
    let url = serve(st.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");

    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["data"]["protocol"], 1);
    assert_eq!(hello["data"]["authRequired"], false);

    ws.send(Wire::Text(
        json!({"type":"hello","protocol":1,"capabilities":["gzip+base64"]})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    let snapshot = next_json(&mut ws).await;
    assert_eq!(snapshot["type"], "snapshot");
    assert!(snapshot["data"]["status"].is_object());
    assert!(snapshot["data"]["capabilities"]["core"].is_string());

    ws.send(Wire::Text(
        json!({"type":"subscribe","topics":["logs"]})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let ack = next_json(&mut ws).await;
    assert_eq!(ack["type"], "subscribed");
    assert_eq!(ack["data"]["topics"][0], "logs");

    st.bus.publish(
        "logs",
        "logs.append",
        json!({"lines":[{"time":1,"level":"info","message":"hello"}]}),
    );
    let event = next_json(&mut ws).await;
    assert_eq!(event["type"], "logs.append");
    assert_eq!(event["topic"], "logs");
    assert!(event["topicSequence"].as_u64().unwrap() >= 1);

    // Unsubscribed topics are filtered out.
    st.bus.publish("traffic", "traffic", json!({"up": 1}));
    ws.send(Wire::Text(json!({"type":"ping"}).to_string().into()))
        .await
        .unwrap();
    let pong = next_json(&mut ws).await;
    assert_eq!(pong["type"], "pong");
}

#[tokio::test]
async fn a_used_topic_is_seeded_not_resynced_on_first_event() {
    let st = state(None);
    // Advance the per-topic counter before the client connects: a long-running
    // bridge always looks like this. The old code assumed every topic started
    // at 1, so this first event was flagged as a gap → resync → clear → gap
    // again, i.e. a full snapshot on every single event.
    for n in 0..5 {
        st.bus.publish("logs", "logs.append", json!({"lines": [], "n": n}));
    }
    let url = serve(st.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let _hello = next_json(&mut ws).await;
    ws.send(Wire::Text(
        json!({"type":"hello","protocol":1}).to_string().into(),
    ))
    .await
    .unwrap();
    let _snapshot = next_type(&mut ws, "snapshot").await;
    ws.send(Wire::Text(
        json!({"type":"subscribe","topics":["logs"]})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let _subscribed = next_type(&mut ws, "subscribed").await;

    st.bus.publish(
        "logs",
        "logs.append",
        json!({"lines":[{"time":1,"level":"info","message":"x"}]}),
    );
    let frame = next_json(&mut ws).await;
    assert_eq!(
        frame["type"], "logs.append",
        "first event on a used topic must be delivered, not resynced: {}",
        frame
    );
}

#[tokio::test]
async fn client_commands_sync_and_resume() {
    let st = state(None);
    let url = serve(st.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let _hello = next_json(&mut ws).await;
    ws.send(Wire::Text(
        json!({"type":"hello","protocol":1}).to_string().into(),
    ))
    .await
    .unwrap();
    let _snapshot = next_json(&mut ws).await;

    ws.send(Wire::Text(json!({"type":"sync"}).to_string().into()))
        .await
        .unwrap();
    let snapshot = next_type(&mut ws, "snapshot").await;
    assert_eq!(snapshot["type"], "snapshot");

    // Resume from an old sequence must produce a snapshot, not silence.
    for i in 0..3 {
        st.bus.publish("status", "status", json!({"n": i}));
    }
    ws.send(Wire::Text(
        json!({"type":"resume","sequence":0}).to_string().into(),
    ))
    .await
    .unwrap();
    let resumed = next_type(&mut ws, "snapshot").await;
    assert_eq!(resumed["type"], "snapshot");
    assert_eq!(resumed["data"]["reason"], "gap");

    ws.send(Wire::Text(
        json!({"type":"resume","sequence": st.bus.last_sequence()})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    let ok = next_type(&mut ws, "resumed").await;
    assert_eq!(ok["type"], "resumed");
}

#[tokio::test]
async fn rejects_bad_protocol_and_bad_auth() {
    let st = state(Some("s3cret".into()));
    let url = serve(st.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["data"]["authRequired"], true);

    ws.send(Wire::Text(
        json!({"type":"hello","protocol":1}).to_string().into(),
    ))
    .await
    .unwrap();
    let required = next_json(&mut ws).await;
    assert_eq!(required["type"], "auth.required");

    ws.send(Wire::Text(
        json!({"type":"auth","secret":"nope"}).to_string().into(),
    ))
    .await
    .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(4), ws.next())
        .await
        .expect("close frame")
        .expect("stream")
        .expect("frame");
    match frame {
        Wire::Close(Some(close)) => assert_eq!(u16::from(close.code), 4401),
        other => panic!("expected close, got {:?}", other),
    }

    // Unsupported protocol version is refused before any state is leaked.
    let url = serve(st.clone()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("connect");
    let _hello = next_json(&mut ws).await;
    ws.send(Wire::Text(
        json!({"type":"auth","secret":"s3cret", "protocol":1})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    ws.send(Wire::Text(
        json!({"type":"hello","protocol":9}).to_string().into(),
    ))
    .await
    .unwrap();
    let err = next_json(&mut ws).await;
    assert_eq!(err["type"], "protocol.error");
}

#[tokio::test]
async fn two_clients_share_one_bus() {
    let st = state(None);
    let url = serve(st.clone()).await;
    let (mut a, _) = tokio_tungstenite::connect_async(url.clone())
        .await
        .expect("a");
    let (mut b, _) = tokio_tungstenite::connect_async(url).await.expect("b");
    for ws in [&mut a, &mut b] {
        let _hello = next_json(ws).await;
        ws.send(Wire::Text(
            json!({"type":"hello","protocol":1}).to_string().into(),
        ))
        .await
        .unwrap();
        let _snapshot = next_json(ws).await;
    }
    let event = st.bus.publish("status", "status", json!({"marker": 7}));
    for ws in [&mut a, &mut b] {
        let mut frame = next_json(ws).await;
        for _ in 0..4 {
            if frame["type"] == "status" && frame["data"]["marker"] == 7 {
                break;
            }
            frame = next_json(ws).await;
        }
        assert_eq!(frame["sequence"], event.sequence);
    }
    assert_eq!(st.ws_clients.load(std::sync::atomic::Ordering::Relaxed), 2);
}

#[tokio::test]
async fn origin_mismatch_is_rejected() {
    let st = state(None);
    let router = super::test_router(st);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{}/ws", addr))
        .header("host", addr.to_string())
        .header("origin", "http://evil.example")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .unwrap();
    let result = tokio_tungstenite::connect_async(request).await;
    assert!(result.is_err(), "foreign origin must not upgrade");
}
