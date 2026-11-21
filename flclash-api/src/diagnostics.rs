//! One-shot, redacted diagnostic bundle.
//!
//! Everything is collected with read-only commands. Credentials (secrets,
//! passwords, UUIDs, tokens, subscription URLs) are replaced before the text
//! leaves the process; the output is plain text so it can be pasted into an
//! issue without a second export step.

use std::process::Command;

use crate::state::AppState;

const MAX_OUTPUT: usize = 256 * 1024;

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .map(|out| {
            let text = if out.stdout.is_empty() {
                String::from_utf8_lossy(&out.stderr).into_owned()
            } else {
                String::from_utf8_lossy(&out.stdout).into_owned()
            };
            text
        })
        .unwrap_or_default()
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn has_key_token(text: &str, key: &str) -> bool {
    let bytes = text.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = text[start..].find(key) {
        let at = start + pos;
        let boundary = at == 0 || {
            let prev = bytes[at - 1];
            !prev.is_ascii_alphanumeric() && prev != b'_'
        };
        if boundary {
            return true;
        }
        start = at + 1;
    }
    false
}

fn redact_value(line: &str) -> String {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    // Configs contain both block (`uuid: x`) and flow (`- { name: n, uuid: x }`)
    // forms. Matching only the line start leaked uuid/url from flow-style
    // proxy entries; any line carrying a credential key is replaced whole.
    // has_key_token avoids treating the `url:` substring of `curl:` as a key.
    const INLINE_KEYS: [&str; 12] = [
        "password:",
        "uuid:",
        "secret:",
        "token=",
        "token:",
        "url:",
        "auth:",
        "auth-str:",
        "auth_str:",
        "obfs-password:",
        "private-key:",
        "subscription-url:",
    ];
    if INLINE_KEYS.iter().any(|key| has_key_token(&lower, key)) {
        return "<redacted: sensitive line>".to_string();
    }
    // UCI export style: `option secret '...'`
    if lower.starts_with("option secret ") || lower.starts_with("list secret ") {
        let prefix = trimmed.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
        return format!("{prefix} <redacted>");
    }
    line.to_string()
}

pub fn redact(text: &str) -> String {
    text.lines().map(redact_value).collect::<Vec<_>>().join("\n")
}

/// Returns the diagnostic text (already redacted and size-capped).
pub fn generate(st: &AppState) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push("== system ==".into());
    parts.push(read("/etc/openwrt_release"));
    parts.push(run("uname", &["-a"]));
    parts.push("== uci (redacted) ==".into());
    parts.push(redact(&run("uci", &["export", "flclash"])));
    parts.push("== runtime config (redacted) ==".into());
    parts.push(redact(&read(&format!(
        "{}/config.yaml",
        st.data_dir.trim_end_matches('/')
    ))));
    parts.push("== nft table inet flclash ==".into());
    parts.push(run("nft", &["list", "table", "inet", "flclash"]));
    parts.push("== ip rule / route ==".into());
    parts.push(run("ip", &["rule"]));
    parts.push(run("ip", &["-6", "rule"]));
    parts.push(run("ip", &["route", "show", "table", "100"]));
    parts.push("== listeners ==".into());
    parts.push(run("netstat", &["-lntup"]));
    parts.push("== recent log ==".into());
    parts.push(run("sh", &["-c", "logread 2>/dev/null | tail -n 200"]));

    let text = redact(&parts.join("\n"));
    if text.len() > MAX_OUTPUT {
        let mut start = text.len() - MAX_OUTPUT;
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        format!("[truncated]\n{}", &text[start..])
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn credentials_are_redacted() {
        let text = "mixed-port: 7892\npassword: hunter2\nuuid: 1234\nurl: https://x/y?token=1\noption secret 'deadbeef'\n";
        let out = redact(text);
        assert!(!out.contains("hunter2"));
        assert!(!out.contains("1234"));
        assert!(!out.contains("token=1"));
        assert!(!out.contains("deadbeef"));
        assert!(out.contains("mixed-port: 7892"));
        // `curl:` must not be mistaken for a `url:` key.
        let log = redact("curl: (7) Failed to connect");
        assert!(log.contains("curl:") && !log.contains("redacted"));
    }
}
