//! flclash-core-bridge: native IPC host for the FlClash (chen08209/FlClash)
//! Clash.Meta core.
//!
//! Why this exists
//! ---------------
//! The FlClash desktop core is built for gomobile/IPC, NOT for the standalone
//! `mihomo -f config.yaml` CLI. Its `hub.ApplyConfig` deliberately skips
//! creating inbound listeners and TUN (`executor.go` comments out
//! `updateListeners`/`updateTun`); those are driven explicitly by the host over
//! IPC via `listener.start`. Running the raw CLI thus opens no mixed/http/socks/
//! TUN ports even with a valid config.
//!
//! This bridge reproduces the desktop host over a loopback IPC socket:
//!   1. listen on a local Unix socket (or 127.0.0.1:<port> if -socket is numeric)
//!   2. spawn the core with that socket address as argv[1]; the core dials us
//!   3. drive the desktop handshake:
//!        core.init{home-dir,version} -> config.setup{...} -> listener.start
//!      (HTTP controller 9097 starts inside config.setup via hub.ApplyConfig;
//!       inbound ports + TUN start inside listener.start via updateListeners)
//!   4. keep draining core -> host event frames so the core's IPC writes never
//!      block on a full socket buffer
//!   5. on disconnect/crash, respawn + re-handshake with bounded backoff
//!
//! Wire format (core/server.go): 4-byte little-endian length + JSON body.
//! MethodCall = {id, method, arguments}; MethodResponse = {id, result, error,
//! file}; events = {type, data} with no id.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf as TcpRead, OwnedWriteHalf as TcpWrite};
use tokio::net::unix::{OwnedReadHalf as UnixRead, OwnedWriteHalf as UnixWrite};
use tokio::net::{TcpListener, UnixListener};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::sleep;

const MAX_FRAME: usize = 64 * 1024 * 1024;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>>;

fn flag_value(args: &[String], name: &str, default: &str) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == name {
            return args.get(i + 1).cloned().unwrap_or_else(|| default.to_string());
        }
        for prefix in [format!("{}=", name), format!("-{}", name), format!("--{}", name)] {
            if let Some(rest) = arg.strip_prefix(&prefix) {
                if !rest.is_empty() {
                    return rest.to_string();
                }
            }
        }
    }
    default.to_string()
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter()
        .any(|a| a == name || a.as_str() == format!("-{}", name) || a.as_str() == format!("--{}", name))
}

/// `--validate <config>`: syntax/semantic preflight for the IPC core.
///
/// The core has no `-t` (argv[1] is the IPC socket), so we boot a throwaway
/// instance in a temp home and stop after `config.setup` — the phase that
/// parses proxies/rules/DNS and starts the controller. Listener.start is never
/// called, so no proxy port/TUN is created.
async fn validate_config(core: &str, home: &str, config: &str) -> Result<(), String> {
    use serde_yaml::{Mapping, Value as YamlValue};

    let text = std::fs::read_to_string(config).map_err(|e| format!("read {config}: {e}"))?;
    let mut doc: YamlValue =
        serde_yaml::from_str(&text).map_err(|e| format!("YAML parse: {e}"))?;
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| "config root must be a mapping".to_string())?;

    let set = |map: &mut Mapping, key: &str, value: YamlValue| {
        map.insert(YamlValue::String(key.to_string()), value);
    };
    // Hold a free loopback port only long enough to pick a number; the core
    // then binds it itself. This avoids colliding with the live controller.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| format!("pick controller port: {e}"))?;
    set(
        map,
        "external-controller",
        YamlValue::String(format!("127.0.0.1:{port}")),
    );
    for key in [
        "port",
        "socks-port",
        "mixed-port",
        "redir-port",
        "tproxy-port",
    ] {
        set(map, key, YamlValue::from(0));
    }
    set(map, "allow-lan", YamlValue::Bool(false));
    map.remove(&YamlValue::String("listeners".into()));
    map.remove(&YamlValue::String("external-ui".into()));
    if let Some(tun) = map
        .get_mut(&YamlValue::String("tun".into()))
        .and_then(|value| value.as_mapping_mut())
    {
        set(tun, "enable", YamlValue::Bool(false));
        tun.remove(&YamlValue::String("enabled".into()));
    }
    if let Some(dns) = map
        .get_mut(&YamlValue::String("dns".into()))
        .and_then(|value| value.as_mapping_mut())
    {
        // A DNS listener on 1053 would collide with the live core. Port 0 is
        // accepted by Go's net.Listen and still validates the DNS schema.
        set(
            dns,
            "listen",
            YamlValue::String("127.0.0.1:0".to_string()),
        );
    }
    let sanitized = serde_yaml::to_string(&doc).map_err(|e| format!("serialize: {e}"))?;

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::path::PathBuf::from(format!("/tmp/flclash-validate-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    // Providers/rule-providers/GeoDB use paths relative to the real home.
    // Symlinking them keeps the temp home cheap (no 30MB copies).
    if let Ok(entries) = std::fs::read_dir(home) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str == "config.yaml"
                || name_str.ends_with(".log")
                || name_str == "cache.db"
                || name_str == "config.yaml.candidate"
                || name_str == "config.yaml.last-good"
            {
                continue;
            }
            let _ = std::os::unix::fs::symlink(entry.path(), dir.join(&name));
        }
    }
    let tmp_config = dir.join("config.yaml");
    std::fs::write(&tmp_config, sanitized).map_err(|e| format!("write candidate: {e}"))?;

    let socket = dir.join("ipc.sock");
    let listener = UnixListener::bind(&socket).map_err(|e| format!("bind ipc: {e}"))?;
    let mut child = Command::new(core)
        .arg(&socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn core {core}: {e}"))?;

    let result = async {
        let (stream, _) = tokio::time::timeout(Duration::from_secs(15), listener.accept())
            .await
            .map_err(|_| "core did not connect within 15s".to_string())?
            .map_err(|e| format!("accept: {e}"))?;
        let (reader, writer) = stream.into_split();
        let mut writer = Writer::Unix(writer);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let reader_pending = pending.clone();
        let reader_task = tokio::spawn(async move {
            reader_loop(Reader::Unix(reader), reader_pending, false).await;
        });
        let call_result = async {
            call(
                &mut writer,
                &pending,
                "1",
                "core.init",
                serde_json::json!({
                    "home-dir": dir.to_string_lossy(),
                    "version": 1,
                }),
                Duration::from_secs(30),
            )
            .await?;
            call(
                &mut writer,
                &pending,
                "2",
                "config.setup",
                serde_json::json!({
                    "selected-map": {},
                    "options": {
                        "test-url": "https://www.gstatic.com/generate_204",
                        "skip-cert-verify": false,
                        "delay-test-timeout": 5000,
                        "mitm-capture": false
                    }
                }),
                Duration::from_secs(60),
            )
            .await?;
            Ok::<(), String>(())
        }
        .await;
        reader_task.abort();
        call_result
    }
    .await;

    let _ = child.kill().await;
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn log(msg: &str) {
    eprintln!("[flclash-bridge] {}", msg);
}

enum Reader {
    Unix(UnixRead),
    Tcp(TcpRead),
}

enum Writer {
    Unix(UnixWrite),
    Tcp(TcpWrite),
}

impl Reader {
    async fn read_exact(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Reader::Unix(r) => r.read_exact(buf).await,
            Reader::Tcp(r) => r.read_exact(buf).await,
        }
    }
}

impl Writer {
    async fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        match self {
            Writer::Unix(w) => w.write_all(buf).await,
            Writer::Tcp(w) => w.write_all(buf).await,
        }
    }
    async fn shutdown(&mut self) {
        match self {
            Writer::Unix(w) => {
                let _ = w.shutdown().await;
            }
            Writer::Tcp(w) => {
                let _ = w.shutdown().await;
            }
        }
    }
}

enum Listener {
    Unix(UnixListener),
    Tcp(TcpListener),
}

async fn bind(socket: &str) -> std::io::Result<Listener> {
    if let Ok(port) = socket.parse::<u16>() {
        log(&format!("IPC listening on TCP 127.0.0.1:{}", port));
        Ok(Listener::Tcp(TcpListener::bind(("127.0.0.1", port)).await?))
    } else {
        let path = PathBuf::from(socket);
        if path.exists() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        if let Some(parent) = path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        log(&format!("IPC listening on unix {}", socket));
        Ok(Listener::Unix(UnixListener::bind(&path)?))
    }
}

async fn accept_split(listener: &Listener) -> std::io::Result<(Reader, Writer)> {
    match listener {
        Listener::Unix(l) => {
            let (stream, _) = l.accept().await?;
            let (r, w) = stream.into_split();
            Ok((Reader::Unix(r), Writer::Unix(w)))
        }
        Listener::Tcp(l) => {
            let (stream, _) = l.accept().await?;
            let (r, w) = stream.into_split();
            Ok((Reader::Tcp(r), Writer::Tcp(w)))
        }
    }
}

async fn write_frame(w: &mut Writer, body: &[u8]) -> std::io::Result<()> {
    if body.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame exceeds maxIPCFrameSize",
        ));
    }
    w.write_all(&(body.len() as u32).to_le_bytes()).await?;
    w.write_all(body).await
}

async fn read_frame(r: &mut Reader) -> std::io::Result<Vec<u8>> {
    let mut header = [0u8; 4];
    r.read_exact(&mut header).await?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "frame exceeds maxIPCFrameSize",
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(body)
}

/// Send one MethodCall and await its matching MethodResponse.
/// `pending` is shared with the reader loop; the lock is released immediately
/// after insert so the reader can deliver the reply while we await `rx`.
async fn call(
    w: &mut Writer,
    pending: &Pending,
    id: &str,
    method: &str,
    arguments: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let (tx, rx) = oneshot::channel::<serde_json::Value>();
    {
        let mut map = pending.lock().await;
        map.insert(id.to_string(), tx);
    } // lock dropped here — reader can now remove/send freely

    let request = serde_json::json!({ "id": id, "method": method, "arguments": arguments });
    let body = serde_json::to_vec(&request).map_err(|e| format!("encode {}: {}", method, e))?;
    write_frame(w, &body)
        .await
        .map_err(|e| format!("send {}: {}", method, e))?;

    let response = tokio::time::timeout(timeout, rx)
        .await
        .map_err(|_| format!("{} timed out after {:?}", method, timeout))?
        .map_err(|_| format!("{} response channel closed", method))?;

    if let Some(err) = response.get("error") {
        if !err.is_null() {
            return Err(format!("{} error: {}", method, err));
        }
    }

    // Spilled response: core writes a large reply to a file and returns {file}.
    if let Some(file) = response.get("file").and_then(|v| v.as_str()) {
        let data = tokio::fs::read(file)
            .await
            .map_err(|e| format!("read spilled response {}: {}", file, e))?;
        let _ = tokio::fs::remove_file(file).await;
        return serde_json::from_slice(&data).map_err(|e| format!("decode spill: {}", e));
    }

    Ok(response.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

/// Drain core -> host frames forever; route method replies to callers and
/// consume event frames. Must keep reading or the core's send() eventually
/// blocks. Returns when the link dies.
async fn reader_loop(mut reader: Reader, pending: Pending, verbose: bool) {
    let mut events = 0u64;
    loop {
        let body = match read_frame(&mut reader).await {
            Ok(b) => b,
            Err(e) => {
                log(&format!("IPC read ended: {}", e));
                break;
            }
        };
        let value: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                log(&format!("bad IPC frame ({} bytes): {}", body.len(), e));
                continue;
            }
        };

        if let Some(id) = value.get("id").and_then(|v| v.as_str()) {
            let mut map = pending.lock().await;
            if let Some(tx) = map.remove(id) {
                let _ = tx.send(value);
            }
            continue;
        }

        events += 1;
        if verbose {
            if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
                log(&format!("event[{}] type={}", events, kind));
            }
        }
    }
    pending.lock().await.clear();
}

struct SessionConfig {
    core: String,
    home_dir: String,
    version: i64,
}

/// One core lifetime: spawn -> connect -> handshake -> monitor.
/// `core_addr` is argv[1] passed to the core (unix path or numeric TCP port).
async fn run_once(
    listener: &Listener,
    core_addr: &str,
    cfg: &SessionConfig,
    verbose: bool,
) -> Result<(), String> {
    let mut child: Child = Command::new(&cfg.core)
        .arg(core_addr)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("spawn core {}: {}", cfg.core, e))?;
    log(&format!("spawned core pid={:?}", child.id()));

    let (reader, mut writer) = match tokio::time::timeout(Duration::from_secs(15), accept_split(listener)).await {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => return Err(format!("accept: {}", e)),
        Err(_) => return Err("core did not connect within 15s".to_string()),
    };

    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let reader_pending = pending.clone();
    let reader_task = tokio::spawn(async move {
        reader_loop(reader, reader_pending, verbose).await;
    });

    let result = async {
        call(&mut writer, &pending, "1", "core.init",
            serde_json::json!({ "home-dir": cfg.home_dir, "version": cfg.version }),
            Duration::from_secs(30)).await?;
        log("core.init ok");

        call(&mut writer, &pending, "2", "config.setup",
            serde_json::json!({
                "selected-map": {},
                "options": {
                    "test-url": "https://www.gstatic.com/generate_204",
                    "skip-cert-verify": false,
                    "delay-test-timeout": 5000,
                    "mitm-capture": false
                }
            }),
            Duration::from_secs(60)).await?;
        log("config.setup ok (HTTP controller live)");

        call(&mut writer, &pending, "3", "listener.start",
            serde_json::Value::Null, Duration::from_secs(60)).await?;
        log("listener.start ok - inbound ports + TUN are live");

        let status = child.wait().await;
        Err::<(), String>(format!("core exited: {:?}", status))
    }
    .await;

    let _ = writer.shutdown().await;
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
    }
    reader_task.abort();
    result
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = env::args().collect();

    // Offline preflight: `flclash-core-bridge --validate <config> [-core bin]
    // [-home-dir dir]`. Exits 0 when the core accepts the config, non-zero
    // with the error on stderr.
    if args.len() >= 3 && (args[1] == "--validate" || args[1] == "-validate") {
        let config = args[2].clone();
        let core = flag_value(&args, "-core", "/usr/bin/mihomo");
        let default_home = PathBuf::from(&config)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_string_lossy()
            .into_owned();
        let home = flag_value(&args, "-home-dir", &default_home);
        match validate_config(&core, &home, &config).await {
            Ok(()) => {
                eprintln!("[flclash-bridge] validate ok: {config}");
                std::process::exit(0);
            }
            Err(err) => {
                eprintln!("[flclash-bridge] validate failed: {err}");
                std::process::exit(1);
            }
        }
    }

    let socket = flag_value(&args, "-socket", "/tmp/flclash-core.sock");
    let core = flag_value(&args, "-core", "/usr/bin/mihomo");
    let home_dir = flag_value(&args, "-home-dir", "/etc/flclash");
    let default_config = format!("{}/config.yaml", home_dir.trim_end_matches('/'));
    let config = flag_value(&args, "-config", &default_config);
    let version: i64 = flag_value(&args, "-version", "1").parse().unwrap_or(1);
    let verbose = has_flag(&args, "-verbose");

    log(&format!(
        "start socket={} core={} home-dir={} config={} version={}",
        socket, core, home_dir, config, version
    ));

    if !PathBuf::from(&core).exists() {
        log(&format!("FATAL: core binary not found: {}", core));
        std::process::exit(1);
    }
    if !PathBuf::from(&config).exists() {
        log(&format!("FATAL: config not found: {}", config));
        std::process::exit(1);
    }

    let listener = match bind(&socket).await {
        Ok(l) => l,
        Err(e) => {
            log(&format!("FATAL: bind {}: {}", socket, e));
            std::process::exit(1);
        }
    };

    let cfg = SessionConfig { core, home_dir, version };
    let mut generation = 0u32;
    let mut backoff = Duration::from_secs(1);
    loop {
        generation += 1;
        log(&format!("generation {} starting", generation));
        match run_once(&listener, &socket, &cfg, verbose).await {
            Ok(()) => break,
            Err(e) => log(&format!("generation {} ended: {}", generation, e)),
        }
        log(&format!("respawn in {:?}", backoff));
        sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}
