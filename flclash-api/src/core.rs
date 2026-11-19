//! Core capability model.
//!
//! mihomo and sing-box are both driven through the Clash controller API, but
//! they do not offer the same features (sing-box has no runtime mode switch
//! and no rule providers). The WebUI needs to know that instead of probing
//! and guessing, and it must never be shown a feature that is faked.

use serde_json::{json, Value};

/// Reads the configured core from UCI (`flclash.main.core_type`), defaulting
/// to mihomo (the only binary shipped in the package).
pub fn configured_core() -> String {
    let out = std::process::Command::new("uci")
        .args(["-q", "get", "flclash.main.core_type"])
        .output();
    match out {
        Ok(out) => {
            let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if value == "sing-box" {
                "sing-box".into()
            } else {
                "mihomo".into()
            }
        }
        Err(_) => "mihomo".into(),
    }
}

/// Capability matrix for the core that is currently configured.
///
/// `runtimeMode` means "mode can be switched through the controller without a
/// restart"; sing-box gets it as `false` because the converter bakes the
/// outbound mode into the generated route table.
pub fn capabilities(core: &str) -> Value {
    let mihomo = core != "sing-box";
    json!({
        "core": if mihomo { "mihomo" } else { "sing-box" },
        "clashApi": true,
        "proxies": true,
        "proxyProviders": true,
        "ruleProviders": mihomo,
        "rules": true,
        "connections": true,
        "logStream": true,
        "trafficStream": true,
        "runtimeMode": mihomo,
        "runtimeTun": true,
        "dnsQuery": mihomo,
        "fakeIpFlush": true,
    })
}

/// `GET /core/capabilities` payload — includes the live controller version
/// when the core answers, so the UI can tell "configured" from "running".
pub async fn payload(st: &crate::state::AppState) -> Value {
    let core = configured_core();
    let mut body = capabilities(&core);
    body["configured"] = json!(true);
    let reachable = crate::ctrl::ctrl_req(
        &st.controller,
        st.secret.as_deref(),
        "GET",
        "/version",
        None,
        std::time::Duration::from_secs(2),
    )
    .await
    .map(|reply| reply.status < 500)
    .unwrap_or(false);
    body["running"] = json!(reachable);
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singbox_does_not_claim_runtime_mode() {
        let caps = capabilities("sing-box");
        assert_eq!(caps["runtimeMode"], false);
        assert_eq!(caps["ruleProviders"], false);
        assert_eq!(caps["core"], "sing-box");
    }

    #[test]
    fn mihomo_supports_runtime_switching() {
        let caps = capabilities("mihomo");
        assert_eq!(caps["runtimeMode"], true);
        assert_eq!(caps["ruleProviders"], true);
    }
}
