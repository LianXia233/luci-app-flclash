//! flclash-api (Rust/Tokio): the WebUI backend for luci-app-flclash.
//!
//! Drop-in replacement for the Go bridge: same binary name, same CLI flags
//! (-listen / -controller / -ui / -data / -init), same /flclash/api contract,
//! same /flclash/ static origin — so init.d, ACL, LuCI and the WebUI stay
//! byte-for-byte unchanged.
//!
//! Layout (one concern per module):
//!   state.rs    — shared state, /proc helpers, formatting, snapshot cache
//!   events.rs   — event envelope + broadcast bus (single producer model)
//!   tasks.rs    — long-running operation registry (progress on `tasks` topic)
//!   ws.rs       — WebSocket transport: auth, subscriptions, gap recovery
//!   core.rs     — core capability matrix (mihomo vs sing-box)
//!   ctrl.rs     — loopback controller client + curl-backed fetch/stream
//!   convert.rs  — subscription format detection and node parsing
//!   config.rs   — config.yaml view + settings persistence
//!   profiles.rs — profile store, import/update/apply, service control
//!   loops.rs    — background samplers and maintenance loops
//!   ipinfo.rs   — outbound IP profile lookup (CF-first multi-source chain)
//!   api.rs      — HTTP handlers

mod api;
mod config;
mod convert;
mod core;
mod ctrl;
mod events;
mod ipinfo;
mod loops;
mod profiles;
mod singbox;
mod state;
mod tasks;
mod ws;

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use state::AppState;

/// Accepts both `-flag value` and `-flag=value` (and `--flag`), matching the
/// flag style the Go bridge accepted from /etc/init.d/flclash-api.
fn flag_value(args: &[String], name: &str, default: &str) -> String {
    for (i, arg) in args.iter().enumerate() {
        if arg == name {
            return args.get(i + 1).cloned().unwrap_or_else(|| default.to_string());
        }
        for prefix in [format!("{}=", name), format!("-{}", name)] {
            if let Some(rest) = arg.strip_prefix(&prefix) {
                return rest.to_string();
            }
        }
    }
    default.to_string()
}

async fn auth(
    State(st): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(secret) = &st.secret {
        let got = req
            .headers()
            .get("x-flclash-secret")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                req.headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.trim_start_matches("Bearer ").trim().to_string())
            });
        if got.as_deref() != Some(secret.as_str()) {
            return (StatusCode::UNAUTHORIZED, Json(json!({"error": "unauthorized"})))
                .into_response();
        }
    }
    next.run(req).await
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Offline conversion helper: `flclash-api --singbox-convert <in.yaml> <out.json>`.
    // Used by init.d and core_switch so a subscription is translated to
    // sing-box losslessly (proxies/groups/rules/DNS/TUN) at startup and on
    // every core switch, without requiring a running API process.
    if args.len() >= 4 && (args[1] == "--singbox-convert" || args[1] == "-singbox-convert") {
        match std::fs::read_to_string(&args[2])
            .map_err(|e| format!("读取失败: {}", e))
            .and_then(|yaml| singbox::convert(&yaml))
            .and_then(|json| std::fs::write(&args[3], json).map_err(|e| format!("写入失败: {}", e)))
        {
            Ok(()) => {
                eprintln!("singbox convert ok: {} -> {}", args[2], args[3]);
                return;
            }
            Err(e) => {
                eprintln!("singbox convert failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    let listen = flag_value(&args, "-listen", "0.0.0.0:9098");
    let controller = flag_value(&args, "-controller", "127.0.0.1:9097")
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .to_string();
    let ui_dir = flag_value(&args, "-ui", "/www/flclash");
    let data_dir = flag_value(&args, "-data", "/etc/flclash");
    let init_script = flag_value(&args, "-init", "/etc/init.d/flclash-core");
    let secret = std::env::var("FLCLASH_API_SECRET")
        .ok()
        .filter(|s| !s.is_empty());

    let st = AppState::new(controller, ui_dir, data_dir, init_script, secret);

    eprintln!(
        "flclash-api listening on {}, controller {}, ui {}",
        listen, st.controller, st.ui_dir
    );

    tokio::spawn(loops::sample_status(st.clone()));
    tokio::spawn(loops::sample_traffic(st.clone()));
    tokio::spawn(loops::sample_logs(st.clone()));
    tokio::spawn(loops::sample_totals(st.clone()));
    tokio::spawn(loops::sample_proxies(st.clone()));
    tokio::spawn(loops::sample_connections(st.clone()));
    tokio::spawn(loops::sync_proxy(st.clone()));
    tokio::spawn(loops::auto_update(st.clone()));
    tokio::spawn(ipinfo::refresh_outbound(st.clone()));

    let protected = Router::new()
        .route("/status", get(api::status))
        .route("/version", get(api::version))
        .route("/memory", get(api::memory))
        .route("/rules", get(api::rules))
        .route("/providers/proxies", get(api::providers))
        .route("/dns/resolve", get(api::dns_resolve))
        .route("/dns/queries", get(api::dns_queries))
        .route("/dns/flush", post(api::dns_flush))
        .route("/config", get(api::config_get).put(api::config_put).patch(api::config_put))
        .route("/proxies", get(api::proxies))
        .route("/proxies/select", post(api::proxies_select))
        .route("/proxies/delay", post(api::proxies_delay))
        .route("/proxies/group-delay", post(api::proxies_group_delay))
        .route("/connections", get(api::connections))
        .route("/connections/{id}", delete(api::connection_close))
        .route("/logs", get(api::logs))
        .route("/tasks", get(api::tasks_list))
        .route("/tasks/{id}", get(api::task_get))
        .route("/core/capabilities", get(api::core_capabilities))
        .route("/ws/stats", get(api::ws_stats))
        .route("/system/proxy", get(api::system_proxy))
        .route("/profiles", get(api::profiles_get))
        .route("/profiles/import", post(api::profiles_import))
        .route("/profiles/{id}/apply", post(api::profiles_apply))
        .route("/profiles/{id}/update", post(api::profiles_update))
        .route(
            "/profiles/{id}",
            axum::routing::patch(api::profiles_edit)
                .put(api::profiles_edit)
                .delete(api::profiles_delete),
        )
        .route("/service/{action}", post(api::service_action))
        .route("/proxy", get(api::proxy_status))
        .route("/proxy/{action}", post(api::proxy_toggle))
        .route_layer(middleware::from_fn_with_state(st.clone(), auth));

    // The WebSocket route authenticates itself: browsers cannot set headers on
    // an upgrade request, so auth happens in-band (`auth` message / query /
    // header) together with an Origin check.
    let realtime = Router::new().route("/ws", get(ws::ws_handler));

    let app: Router<()> = Router::new()
        .nest("/flclash/api", protected.merge(realtime))
        .route("/", get(api::static_index))
        .route("/flclash", get(api::static_index))
        .route("/flclash/", get(api::static_root))
        .route("/flclash/{*path}", get(api::static_file))
        .layer(axum::extract::DefaultBodyLimit::max(crate::state::MAX_BODY))
        .with_state(st.clone());

    let server = tokio::net::TcpListener::bind(&listen)
        .await
        .expect("failed to bind listen address");
    axum::serve(server, app).await.expect("server error");
}
