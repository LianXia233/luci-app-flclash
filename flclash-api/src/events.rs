//! Unified event bus shared by every producer (background samplers, REST
//! handlers, task runner) and every consumer (WebSocket connections).
//!
//! One envelope shape is used for REST-driven snapshots, WebSocket pushes and
//! task progress, so the WebUI never has to special-case a transport:
//!
//! ```json
//! {"id":"…","type":"status","topic":"status","data":{…},
//!  "timestamp":1730000000000,"sequence":42,"version":1}
//! ```
//!
//! Sequences are global and monotonic. Clients use them to (a) drop stale
//! frames, (b) detect gaps after a reconnect or a lagged broadcast receiver.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::broadcast;

/// Wire protocol version. Bumped only for breaking envelope changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Per-bus broadcast capacity. A subscriber that falls further behind than
/// this gets `RecvError::Lagged` and is told to re-sync from a snapshot.
pub const EVENT_BUFFER: usize = 1024;

/// Topics a client may subscribe to. `system` and `status` are always on.
pub const TOPICS: [&str; 7] = [
    "system",
    "status",
    "traffic",
    "proxies",
    "connections",
    "logs",
    "tasks",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    /// Event type, e.g. `status`, `traffic`, `proxies`, `task`, `hello`.
    #[serde(rename = "type")]
    pub kind: String,
    pub topic: String,
    pub data: Value,
    /// Unix milliseconds.
    pub timestamp: i64,
    pub sequence: u64,
    /// Per-topic monotonic counter. Lets a client detect a lost frame on
    /// exactly the topics it subscribed to (the global sequence interleaves).
    #[serde(rename = "topicSequence")]
    pub topic_seq: u64,
    pub version: u32,
    /// Present when `data` is transport-encoded (`gzip+base64`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

impl Envelope {
    /// Builds an envelope that is only sent to one connection (hello, pong,
    /// resync notices). It still carries the bus sequence so clients can keep
    /// one ordered view of everything they receive.
    pub fn local(kind: &str, topic: &str, data: Value, sequence: u64) -> Envelope {
        let timestamp = now_millis();
        Envelope {
            id: format!("{}-{}-local", timestamp, sequence),
            kind: kind.to_string(),
            topic: topic.to_string(),
            data,
            timestamp,
            sequence,
            topic_seq: sequence,
            version: PROTOCOL_VERSION,
            encoding: None,
            request_id: None,
            trace_id: None,
        }
    }
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Envelope>>,
    seq: Arc<AtomicU64>,
    topic_seq: Arc<Mutex<HashMap<String, u64>>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(EVENT_BUFFER);
        Self {
            tx,
            seq: Arc::new(AtomicU64::new(0)),
            topic_seq: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Envelope>> {
        self.tx.subscribe()
    }

    pub fn receivers(&self) -> usize {
        self.tx.receiver_count()
    }

    pub fn last_sequence(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    /// Publishes an event on `topic`. `kind` is the event type carried in
    /// `type`; it is usually the same as the topic but may be finer grained
    /// (`logs.line`, `task.progress`, `system.hello`).
    pub fn publish(&self, topic: &str, kind: &str, data: Value) -> Arc<Envelope> {
        self.publish_with(topic, kind, data, None, None)
    }

    pub fn publish_with(
        &self,
        topic: &str,
        kind: &str,
        data: Value,
        request_id: Option<String>,
        trace_id: Option<String>,
    ) -> Arc<Envelope> {
        let sequence = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let topic_seq = {
            let mut map = self.topic_seq.lock().unwrap();
            let entry = map.entry(topic.to_string()).or_insert(0);
            *entry += 1;
            *entry
        };
        let timestamp = now_millis();
        let envelope = Arc::new(Envelope {
            id: format!("{}-{}", timestamp, sequence),
            kind: kind.to_string(),
            topic: topic.to_string(),
            data,
            timestamp,
            sequence,
            topic_seq,
            version: PROTOCOL_VERSION,
            encoding: None,
            request_id,
            trace_id,
        });
        // No subscribers is a normal state (UI closed, REST-only clients).
        let _ = self.tx.send(envelope.clone());
        envelope
    }
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// True for a topic name accepted by `subscribe`/`unsubscribe`.
pub fn is_topic(name: &str) -> bool {
    TOPICS.contains(&name)
}

/// Normalises the `topics` array of a subscribe message.
pub fn parse_topics(value: &Value) -> Result<Vec<String>, String> {
    let arr = value
        .as_array()
        .ok_or_else(|| "topics must be an array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let name = item
            .as_str()
            .ok_or_else(|| "topic must be a string".to_string())?;
        if !is_topic(name) {
            return Err(format!("unknown topic: {}", name));
        }
        if !out.iter().any(|t| t == name) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

/// Compact JSON for the wire (no whitespace).
pub fn encode(envelope: &Envelope) -> String {
    serde_json::to_string(envelope).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sequence_is_monotonic() {
        let bus = EventBus::new();
        let a = bus.publish("status", "status", json!({"a": 1}));
        let b = bus.publish("status", "status", json!({"a": 2}));
        assert_eq!(a.sequence, 1);
        assert_eq!(b.sequence, 2);
        assert_eq!(bus.last_sequence(), 2);
        assert_eq!(a.version, PROTOCOL_VERSION);
        assert!(a.id.contains(&a.sequence.to_string()));
    }

    #[tokio::test]
    async fn subscribers_receive_and_filter() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let sent = bus.publish("proxies", "proxies", json!({"groups": []}));
        let got = rx.recv().await.expect("event");
        assert_eq!(got.sequence, sent.sequence);
        assert_eq!(got.topic, "proxies");
        assert!(is_topic("proxies"));
        assert!(!is_topic("nope"));
    }

    #[test]
    fn topic_parsing_is_strict_and_deduped() {
        assert_eq!(
            parse_topics(&json!(["logs", "logs", "proxies"])).unwrap(),
            vec!["logs".to_string(), "proxies".to_string()]
        );
        assert!(parse_topics(&json!(["logs", "bogus"])).is_err());
        assert!(parse_topics(&json!("logs")).is_err());
    }
}
