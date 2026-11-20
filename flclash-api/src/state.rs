//! Shared application state and small helpers used across modules.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::events::{now_millis, EventBus};
use crate::tasks::TaskManager;

pub const MAX_HISTORY: usize = 60;
pub const MAX_LOGS: usize = 500;
pub const MAX_BODY: usize = 32 << 20;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub time: i64,
    pub level: String,
    pub message: String,
}

/// Bridge-side caches: sampled traffic history, logs, totals and the cached
/// outbound lookup. Guarded by one mutex — contention is negligible at UI
/// polling rates.
/// Outbound lookup result (IP profile + quality flags), cached by the
/// background loop and rendered on the dashboard status card.
#[derive(Debug, Clone, Default)]
pub struct IpInfo {
    pub ip: String,
    pub country: String,
    pub country_code: String,
    pub city: String,
    pub isp: String,
    pub org: String,
    pub asn: String,
    pub proxy: bool,
    pub hosting: bool,
    pub mobile: bool,
}

impl IpInfo {
    /// IP quality classification mirroring the desktop client's badges:
    /// 住宅 (residential) / 机房 (datacenter) / 移动 (mobile network),
    /// plus a 干净/代理 trust level.
    pub fn quality_type(&self) -> &'static str {
        if self.mobile {
            "移动网络"
        } else if self.hosting {
            "机房/IDC"
        } else if !self.isp.is_empty() || !self.org.is_empty() {
            "住宅"
        } else {
            "未知"
        }
    }

    pub fn quality_level(&self) -> &'static str {
        if self.proxy {
            "风险:代理"
        } else {
            "干净"
        }
    }
}

#[derive(Default)]
pub struct History {
    pub up: Vec<f64>,
    pub down: Vec<f64>,
    pub peak_up: f64,
    pub peak_down: f64,
    pub total_up: f64,
    pub total_dn: f64,
    pub logs: Vec<LogEntry>,
    /// Live connection count, sampled once per connections poll and reused by
    /// /status (it used to issue a second /connections request just for this).
    pub conn_count: usize,
    pub outbound_ip: String,
    pub outbound_ip6: String,
    pub outbound_cc: String,
    pub outbound_org: String,
    pub outbound_ok: bool,
    pub ipinfo: Option<IpInfo>,
}

pub struct AppState {
    pub controller: String,
    pub ui_dir: String,
    pub data_dir: String,
    pub init_script: String,
    pub secret: Option<String>,
    pub hist: Mutex<History>,
    /// Cross-transport event bus (WebSocket fan-out + task progress).
    pub bus: EventBus,
    /// Long-running operation registry.
    pub tasks: TaskManager,
    /// Latest authoritative status snapshot. REST `/status` and the WebSocket
    /// `status`/`snapshot` events both read this, so no two views can disagree.
    latest: tokio::sync::watch::Sender<Arc<Value>>,
    snapshot_at: AtomicI64,
    /// Connected WebSocket clients (monitoring).
    pub ws_clients: AtomicUsize,
    pub started_at: i64,
    /// Random-ish identifier of this process, echoed in `hello` so clients can
    /// tell a restart (state must be re-synced) from a reconnect.
    pub boot_id: String,
}

impl AppState {
    pub fn new(
        controller: String,
        ui_dir: String,
        data_dir: String,
        init_script: String,
        secret: Option<String>,
    ) -> Arc<Self> {
        let bus = EventBus::new();
        let tasks = TaskManager::new(bus.clone());
        let (latest, _initial_rx) = tokio::sync::watch::channel(Arc::new(json!({
            "running": false,
            "generatedAt": 0,
        })));
        Arc::new(Self {
            controller,
            ui_dir,
            data_dir,
            init_script,
            secret,
            hist: Mutex::default(),
            bus,
            tasks,
            latest,
            snapshot_at: AtomicI64::new(0),
            ws_clients: AtomicUsize::new(0),
            started_at: now_millis(),
            boot_id: format!("{}-{}", now_millis(), std::process::id()),
        })
    }

    /// Current authoritative status snapshot (never blocks on the controller).
    pub fn snapshot(&self) -> Arc<Value> {
        self.latest.borrow().clone()
    }

    pub fn snapshot_age_ms(&self) -> i64 {
        let at = self.snapshot_at.load(Ordering::Relaxed);
        if at == 0 {
            i64::MAX
        } else {
            now_millis() - at
        }
    }

    /// Marks the cached snapshot as just verified without re-publishing it.
    /// Used by the sampler when the payload is byte-identical.
    pub fn touch_snapshot(&self) {
        self.snapshot_at.store(now_millis(), Ordering::Relaxed);
    }

    /// Publishes a new snapshot to REST readers and WS subscribers simultaneously.
    pub fn publish_snapshot(&self, value: Value) {
        self.snapshot_at.store(now_millis(), Ordering::Relaxed);
        let value = Arc::new(value);
        let _ = self.latest.send_replace(value.clone());
        self.bus.publish("status", "status", (*value).clone());
    }

    pub fn index_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("index.json")
    }
    pub fn profiles_dir(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("profiles")
    }
    pub fn config_path(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.data_dir).join("config.yaml")
    }
}

/// Formats a byte count the same way the Go bridge did ("1.5 MB").
pub fn human_bytes(v: i64) -> String {
    const UNIT: i64 = 1024;
    if v < UNIT {
        return format!("{} B", v);
    }
    let mut div: i64 = UNIT;
    let mut exp = 0usize;
    let mut n = v / UNIT;
    while n >= UNIT {
        div *= UNIT;
        exp += 1;
        n /= UNIT;
    }
    format!("{:.1} {}B", v as f64 / div as f64, ['K', 'M', 'G', 'T', 'P', 'E'][exp])
}

/// Relative-age label matching the Go bridge.
pub fn human_age(ts: i64) -> String {
    if ts == 0 {
        return "—".into();
    }
    let delta = now_secs() - ts;
    if delta < 60 {
        "刚刚".into()
    } else if delta < 3600 {
        format!("{} 分钟前", delta / 60)
    } else if delta < 86400 {
        format!("{} 小时前", delta / 3600)
    } else {
        format!("{} 天前", delta / 86400)
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Scans /proc for the running proxy core (`mihomo` or `sing-box`) and returns
/// its pid. Matching only "mihomo" made memory/uptime read 0 forever as soon
/// as the user switched to sing-box.
pub fn core_pid() -> u32 {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) {
            let comm = comm.trim();
            if comm == "mihomo" || comm == "sing-box" {
                return name.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Core RSS in bytes from /proc/<pid>/status (VmRSS, kB).
pub fn rss_bytes(pid: u32) -> f64 {
    let Ok(data) = std::fs::read_to_string(format!("/proc/{}/status", pid)) else {
        return 0.0;
    };
    for line in data.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: f64 = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0);
            return kb * 1024.0;
        }
    }
    0.0
}

/// Core uptime in seconds from /proc/<pid>/stat starttime (CLK_TCK=100).
pub fn core_uptime(pid: u32) -> f64 {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", pid)) else {
        return 0.0;
    };
    let Some(idx) = stat.rfind(')') else { return 0.0 };
    let fields: Vec<&str> = stat[idx + 1..].split_whitespace().collect();
    if fields.len() < 20 {
        return 0.0;
    }
    let start_ticks: f64 = fields[19].parse().unwrap_or(0.0);
    let Ok(uptime) = std::fs::read_to_string("/proc/uptime") else {
        return 0.0;
    };
    let up_sec: f64 = uptime
        .split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    (up_sec - start_ticks / 100.0).max(0.0)
}

/// First non-loopback IPv4 address, matching localIPv4() of the Go bridge.
/// std has no interface enumeration, so ask the routing table: a UDP
/// "connect" to a LAN address selects the source address without sending
/// any packet.
pub fn local_ipv4() -> String {
    if let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if sock.connect("192.168.255.255:9").is_ok() {
            if let Ok(addr) = sock.local_addr() {
                if let std::net::IpAddr::V4(v4) = addr.ip() {
                    if !v4.is_loopback() {
                        return v4.to_string();
                    }
                }
            }
        }
    }
    String::new()
}
