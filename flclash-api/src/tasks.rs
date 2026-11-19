//! Long-running operation tracker.
//!
//! REST handlers answer immediately with a `taskId`; the real progress is
//! published on the `tasks` topic, so a client that loses its HTTP response
//! (timeout, refresh, second browser tab) still observes the outcome.
//!
//! Lifecycle: `queued → running → done | failed`. Every transition carries
//! the full task object, and `GET /tasks` returns the same objects — one
//! authoritative model for both transports.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::events::{now_millis, EventBus};

/// Finished tasks older than this are pruned from the list.
const TASK_TTL_MS: i64 = 60 * 60 * 1000;
const TASK_MAX: usize = 64;

struct Inner {
    bus: EventBus,
    tasks: Mutex<HashMap<String, Value>>,
    seq: AtomicU64,
}

/// Cheap handle: cloning shares the same registry (never a snapshot copy).
#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Inner>,
}

pub struct TaskHandle {
    id: String,
    manager: TaskManager,
}

impl TaskManager {
    pub fn new(bus: EventBus) -> Self {
        Self {
            inner: Arc::new(Inner {
                bus,
                tasks: Mutex::new(HashMap::new()),
                seq: AtomicU64::new(0),
            }),
        }
    }

    /// Creates a task and publishes its `running` state.
    pub fn begin(&self, kind: &str, label: &str, request_id: Option<String>) -> TaskHandle {
        let seq = self.inner.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("t{}-{}", now_millis(), seq);
        let task = json!({
            "id": id,
            "kind": kind,
            "label": label,
            "state": "running",
            "progress": 0.0,
            "message": "",
            "error": null,
            "requestId": request_id,
            "startedAt": now_millis(),
            "finishedAt": null,
            "result": null,
        });
        self.store(&id, task);
        TaskHandle {
            id,
            manager: self.clone(),
        }
    }

    pub fn list(&self) -> Value {
        let mut map = self.inner.tasks.lock().unwrap();
        let now = now_millis();
        map.retain(|_, task| {
            let finished = task.get("finishedAt").and_then(|v| v.as_i64()).unwrap_or(0);
            finished == 0 || now - finished < TASK_TTL_MS
        });
        let mut tasks: Vec<Value> = map.values().cloned().collect();
        tasks.sort_by(|a, b| {
            let ak = a.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0);
            let bk = b.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0);
            bk.cmp(&ak)
        });
        tasks.truncate(TASK_MAX);
        json!(tasks)
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.inner.tasks.lock().unwrap().get(id).cloned()
    }

    fn store(&self, id: &str, task: Value) {
        {
            let mut map = self.inner.tasks.lock().unwrap();
            if map.len() >= TASK_MAX {
                // drop the oldest finished task to stay bounded
                let victim = map
                    .iter()
                    .filter(|(_, t)| t.get("finishedAt").and_then(|v| v.as_i64()).unwrap_or(0) != 0)
                    .min_by_key(|(_, t)| t.get("startedAt").and_then(|v| v.as_i64()).unwrap_or(0))
                    .map(|(k, _)| k.clone());
                if let Some(key) = victim {
                    map.remove(&key);
                }
            }
            map.insert(id.to_string(), task.clone());
        }
        self.inner.bus.publish("tasks", "task", task);
    }
}

impl TaskHandle {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Updates progress (0..=100) and the human-readable step message.
    pub fn progress(&self, progress: f64, message: &str) {
        self.patch(|task| {
            task["state"] = json!("running");
            task["progress"] = json!(progress.clamp(0.0, 100.0));
            task["message"] = json!(message);
        });
    }

    pub fn succeed(&self, result: Value) {
        self.patch(|task| {
            task["state"] = json!("done");
            task["progress"] = json!(100.0);
            task["finishedAt"] = json!(now_millis());
            task["result"] = result;
        });
    }

    pub fn fail(&self, error: &str) {
        self.patch(|task| {
            task["state"] = json!("failed");
            task["finishedAt"] = json!(now_millis());
            task["error"] = json!(error);
        });
    }

    fn patch(&self, apply: impl FnOnce(&mut Value)) {
        let mut task = match self.manager.get(&self.id) {
            Some(task) => task,
            None => return,
        };
        apply(&mut task);
        self.manager.store(&self.id, task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_is_observable() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let manager = TaskManager::new(bus.clone());
        let handle = manager.begin("profile.import", "导入订阅", Some("req-1".into()));

        let start = rx.try_recv().expect("running event");
        assert_eq!(start.kind, "task");
        assert_eq!(start.data["state"], "running");
        assert_eq!(start.data["requestId"], "req-1");

        handle.progress(40.0, "下载中");
        let progress = rx.try_recv().expect("progress event");
        assert_eq!(progress.data["progress"], 40.0);

        handle.succeed(json!({"proxies": 12}));
        let done = rx.try_recv().expect("done event");
        assert_eq!(done.data["state"], "done");
        assert_eq!(done.data["result"]["proxies"], 12);
        assert_eq!(manager.get(handle.id()).unwrap()["state"], "done");
        assert_eq!(manager.list().as_array().unwrap().len(), 1);
    }
}
