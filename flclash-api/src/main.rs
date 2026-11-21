#![recursion_limit = "256"]
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

mod acl;
mod api;
mod backup;
mod config;
mod convert;
mod diagnostics;
mod core;
mod coremgmt;
mod ctrl;
mod events;
mod ipinfo;
mod loops;
mod mixin;
mod preflight;
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
    let long = format!("-{}", name); // "-listen" -> "--listen"
    for (i, arg) in args.iter().enumerate() {
        if arg == name || arg == &long {
            return args.get(i + 1).cloned().unwrap_or_else(|| default.to_string());
        }
        for prefix in [format!("{}=", name), format!("{}=", long)] {
            if let Some(rest) = arg.strip_prefix(&prefix) {
                return rest.to_string();
            }
        }
    }
    default.to_string()
}

/// Length-independent comparison for the shared secret: `==` short-circuits on
/// the first differing byte and leaks prefix length over the network.
pub(crate) fn secret_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Short-lived jump tokens written by the LuCI rpcd object (0600, root only).
/// They let the WebUI open without the raw API secret ever travelling through
/// ubus/URLs; the tokens are revoked on reboot and capped by the writer.
/// Returns false for an empty/missing token file, so a vanished file cannot
/// turn into "any token is valid".
pub(crate) fn token_in_text(data: &str, provided: &str) -> bool {
    if provided.is_empty() {
        return false;
    }
    data.lines().any(|line| {
        let token = line.trim();
        token.len() == provided.len() && secret_eq(token.as_bytes(), provided.as_bytes())
    })
}

pub(crate) fn jump_token_valid(provided: &str) -> bool {
    if provided.is_empty() {
        return false;
    }
    let Ok(path) = std::env::var("FLCLASH_JUMP_TOKENS") else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    let Ok(data) = std::fs::read_to_string(path) else {
        return false;
    };
    token_in_text(&data, provided)
}

fn api_secret() -> Option<String> {
    std::env::var("FLCLASH_API_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn secret_is_valid(value: &str) -> bool {
    value.len() >= 16 && value.chars().all(|c| c.is_ascii_graphic())
}

/// `require_secret=1` (default) turns an empty/short secret into a hard
/// startup failure: this process can rewrite configs, install cores and drive
/// the firewall, so an unauthenticated listener on 0.0.0.0 is a root console.
fn enforce_secret(secret: &Option<String>) {
    let required = std::env::var("FLCLASH_API_REQUIRE_SECRET")
        .map(|value| value != "0")
        .unwrap_or(true);
    if !required {
        if secret.is_none() {
            eprintln!("WARNING: FLCLASH_API_REQUIRE_SECRET=0 and no API secret; the control plane is unauthenticated");
        }
        return;
    }
    let valid = secret
        .as_deref()
        .map(secret_is_valid)
        .unwrap_or(false);
    if !valid {
        eprintln!(
            "FATAL: refusing to start: FLCLASH_API_SECRET is missing or shorter than 16 printable chars. \
             Generate one with: head -c 32 /dev/urandom | md5sum | cut -d' ' -f1 > /etc/flclash/api.secret && chmod 600 /etc/flclash/api.secret"
        );
        std::process::exit(1);
    }
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
        let allowed = got
            .as_deref()
            .map(|value| {
                secret_eq(value.as_bytes(), secret.as_bytes()) || jump_token_valid(value)
            })
            .unwrap_or(false);
        if !allowed {
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
    let secret = api_secret();
    enforce_secret(&secret);

    let st = AppState::new(controller, ui_dir, data_dir, init_script, secret);

    eprintln!(
        "flclash-api listening on {}, controller {}, ui {} (auth: {})",
        listen,
        st.controller,
        st.ui_dir,
        if st.secret.is_some() { "secret" } else { "disabled" }
    );

    tokio::spawn(loops::sample_status(st.clone()));
    tokio::spawn(loops::sample_traffic(st.clone()));
    tokio::spawn(loops::sample_logs(st.clone()));
    tokio::spawn(loops::sample_dns_log(st.clone()));
    tokio::spawn(loops::sample_proxies(st.clone()));
    tokio::spawn(loops::sample_connections(st.clone()));
    tokio::spawn(loops::sync_proxy(st.clone()));
    tokio::spawn(loops::auto_update(st.clone()));
    tokio::spawn(ipinfo::refresh_outbound(st.clone()));

    let protected = Router::new()
        .route("/status", get(api::status))
        .route("/memory", get(api::memory))
        .route("/rules", get(api::rules))
        .route("/providers/proxies", get(api::providers))
        .route("/providers/proxies/{name}/update", post(api::proxy_provider_update))
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
        .route("/mixin", get(api::mixin_get).put(api::mixin_put).patch(api::mixin_put))
        .route("/config/validate", post(api::config_validate))
        .route("/profiles/{id}/raw", get(api::profile_raw).put(api::profile_raw_put))
        .route("/backup", get(api::backup_list).post(api::backup_create))
        .route("/backup/download/{name}", get(api::backup_download))
        .route("/backup/restore", post(api::backup_restore_snapshot))
        .route("/backup/upload", post(api::backup_upload))
        .route("/backup/{name}", delete(api::backup_delete))
        .route("/system/proxy", get(api::system_proxy))
        .route("/diagnostics", get(api::diagnostics))
        .route("/profiles", get(api::profiles_get))
        .route("/profiles/import", post(api::profiles_import))
        .route("/profiles/import-local", post(api::profiles_import_local))
        .route("/profiles/update-all", post(api::profiles_update_all))
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
        .route("/proxy/mode", axum::routing::put(api::proxy_mode_set))
        .route("/proxy/{action}", post(api::proxy_toggle))
        .route("/proxy/acl", get(api::acl_get).put(api::acl_put))
        .route("/dns/config", get(api::dns_config))
        .route("/providers/rules", get(api::rule_providers))
        .route("/providers/rules/{name}/update", post(api::rule_provider_update))
        .route("/system/geo", post(api::geo_update))
        .route("/core", get(api::core_info))
        .route("/core/switch", post(api::core_switch))
        .route("/core/update", post(api::core_update))
        .route("/core/update/status", get(api::core_update_status))
        .route_layer(middleware::from_fn_with_state(st.clone(), auth));

    // The WebSocket route authenticates itself: browsers cannot set headers on
    // an upgrade request, so auth happens in-band (`auth` message / query /
    // header) together with an Origin check.
    let realtime = Router::new().route("/ws", get(ws::ws_handler));

    // Public metadata: the LuCI page reads the core version with busybox wget
    // (no header support for a quoting-safe secret), and a version string is
    // not sensitive. Everything else stays behind the secret.
    let public = Router::new().route("/version", get(api::version));

    let app: Router<()> = Router::new()
        .nest("/flclash/api", public.merge(protected).merge(realtime))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_policy_rejects_empty_short_and_control_chars() {
        assert!(!secret_is_valid(""));
        assert!(!secret_is_valid("short"));
        assert!(!secret_is_valid("0123456789abcde\n"));
        assert!(secret_is_valid("0123456789abcdef"));
        assert!(secret_is_valid("A_Z-9.~+/=0123456789"));
    }

    #[test]
    fn jump_token_file_matches_exact_lines_only() {
        let file = "aaaa\nbbbb  \ncccc\n";
        assert!(token_in_text(file, "bbbb"));
        assert!(token_in_text(file, "cccc"));
        assert!(!token_in_text(file, ""));
        assert!(!token_in_text(file, "bbbbbbbb"));
        assert!(!token_in_text(file, "dddd"));
    }

    #[test]
    fn constant_time_compare_is_exact() {
        assert!(secret_eq(b"abc", b"abc"));
        assert!(!secret_eq(b"abc", b"abd"));
        assert!(!secret_eq(b"abc", b"ab"));
    }
}
