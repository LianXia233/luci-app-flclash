//! Backup & restore for the FlClash data directory.
//!
//! A backup is a gzip'd ustar archive of a *whitelisted* set of files:
//! `config.yaml` (generated kernel config), `profiles.json` + `profiles/`
//! (subscriptions and their converted bodies), `mixin.yaml` (user overrides)
//! and `uci-flclash.txt` (a snapshot of the UCI section, when `uci` exists).
//!
//! Restore never trusts the archive: entries are validated (relative path,
//! whitelisted prefix, size/count caps), the current state is snapshotted first,
//! the YAML is parsed before anything is written, and the apply runs as a
//! background task so a slow core restart cannot time out the request.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde_json::{json, Value};

pub const MAX_ENTRIES: usize = 256;
pub const MAX_TOTAL_BYTES: usize = 24 << 20;
pub const MAX_ENTRY_BYTES: usize = 8 << 20;
const KEEP_SNAPSHOTS: usize = 12;

/// Files that may be restored. Anything else is rejected by name.
/// uci-flclash.txt is the UCI section export collected by snapshot(); without
/// it on this list every real-device snapshot failed to restore, because
/// collect() always packs the export on hosts where `uci` exists.
const ALLOWED: [&str; 5] = ["config.yaml", "profiles.json", "mixin.yaml", "index.json", "uci-flclash.txt"];
const ALLOWED_PREFIXES: [&str; 1] = ["profiles/"];

fn is_allowed(name: &str) -> bool {
    ALLOWED.contains(&name)
        || ALLOWED_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

fn safe_rel(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

fn octal(bytes: &[u8]) -> u64 {
    let text = String::from_utf8_lossy(bytes);
    let digits: String = text.chars().take_while(|c| ('0'..='7').contains(c)).collect();
    u64::from_str_radix(&digits, 8).unwrap_or(0)
}

/// Validates the ustar header checksum: sum of every byte with the checksum
/// field treated as spaces, accepted as either unsigned or signed. A corrupted
/// archive used to be parsed anyway, potentially writing shifted bytes into
/// the data directory.
fn checksum_ok(header: &[u8]) -> bool {
    let stored = octal(&header[148..156]);
    if stored == 0 {
        // Old writers sometimes leave the field zeroed; the whitelist and the
        // YAML/JSON validation still apply, so treat it as "unmarked".
        return true;
    }
    let mut unsigned: u64 = 0;
    let mut signed: i64 = 0;
    for (index, byte) in header.iter().enumerate() {
        let value = if (148..156).contains(&index) { b' ' } else { *byte };
        unsigned += value as u64;
        signed += value as i8 as i64;
    }
    stored == unsigned || (signed > 0 && stored as i64 == signed)
}

/// A tar numeric field: octal digits padded to the field width, with the last
/// byte always NUL.
fn pad_octal(value: u64, width: usize) -> Vec<u8> {
    let mut field = vec![0u8; width];
    let octal_text = format!("{:o}", value);
    let pad = (width - 1).saturating_sub(octal_text.len());
    let text = format!("{}{}", "0".repeat(pad), octal_text);
    let bytes = text.as_bytes();
    let n = bytes.len().min(width - 1);
    field[..n].copy_from_slice(&bytes[..n]);
    field
}

fn header(name: &str, size: usize) -> Vec<u8> {
    let mut h = vec![0u8; 512];
    let name_bytes = name.as_bytes();
    let (prefix, base) = if name_bytes.len() > 100 {
        // ustar splits long names into prefix + name at a '/'.
        match name_bytes[..100].iter().rposition(|b| *b == b'/') {
            Some(pos) => (&name_bytes[..pos], &name_bytes[pos + 1..]),
            None => (&b""[..], name_bytes),
        }
    } else {
        (&b""[..], name_bytes)
    };
    h[0..base.len()].copy_from_slice(base);
    h[345..345 + prefix.len()].copy_from_slice(prefix);
    h[100..108].copy_from_slice(&pad_octal(0o644, 8));
    h[108..116].copy_from_slice(&pad_octal(0, 8));
    h[116..124].copy_from_slice(&pad_octal(0, 8));
    h[124..136].copy_from_slice(&pad_octal(size as u64, 12));
    h[136..148].copy_from_slice(&pad_octal(0, 12));
    h[148..156].copy_from_slice(b"0000000\0");
    h[156..157].copy_from_slice(b"0");
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // Checksum: the field itself is counted as eight spaces, not zeros.
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    let text = format!("{:06o}\0 ", sum);
    h[148..156].copy_from_slice(text.as_bytes());
    h
}

/// Builds a `.tar.gz` from `(relative-name, bytes)` pairs.
pub fn pack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut tar = Vec::new();
    for (name, data) in entries {
        tar.extend_from_slice(&header(name, data.len()));
        tar.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        tar.extend(std::iter::repeat(0u8).take(pad));
    }
    tar.extend_from_slice(&[0u8; 1024]);
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&tar).expect("gzip write");
    enc.finish().expect("gzip finish")
}

/// Unpacks `data`, enforcing the restore policy. Returns `(name, bytes)`.
pub fn unpack(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut dec = GzDecoder::new(data);
    let mut raw = Vec::new();
    dec.read_to_end(&mut raw).map_err(|e| format!("不是有效的 gzip 备份: {}", e))?;
    let mut out = Vec::new();
    let mut total = 0usize;
    let mut offset = 0usize;
    while offset + 512 <= raw.len() {
        let h = &raw[offset..offset + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        if !checksum_ok(h) {
            return Err("备份文件头校验和错误".into());
        }
        let size = octal(&h[124..135]) as usize;


        let typeflag = h[156];
        let base = String::from_utf8_lossy(&h[0..100]).trim_end_matches('\0').to_string();
        let prefix = String::from_utf8_lossy(&h[345..500]).trim_end_matches('\0').to_string();
        let name = if prefix.is_empty() { base } else { format!("{}/{}", prefix, base) };
        offset += 512;
        if typeflag == b'5' {
            continue;
        }
        if size == 0 {
            continue;
        }
        if size > MAX_ENTRY_BYTES || offset + size > raw.len() {
            return Err("备份内单个文件过大或已损坏".into());
        }
        total += size;
        if total > MAX_TOTAL_BYTES || out.len() >= MAX_ENTRIES {
            return Err("备份内容超出限制".into());
        }
        if !is_allowed(&name) || safe_rel(&name).is_none() {
            return Err(format!("备份中包含不允许的文件: {}", name));
        }
        out.push((name, raw[offset..offset + size].to_vec()));
        offset += size + (512 - size % 512) % 512;
        // 512-byte alignment: when size is a multiple of 512 there is no pad.

    }
    if out.is_empty() {
        return Err("备份为空".into());
    }
    Ok(out)
}

fn read_file(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// Drops `option secret`/`list secret` from a `uci export` body. The API
/// secret lives in a 0600 file now; a snapshot taken from an older install
/// (or restored from one) must not put it back into world-readable UCI.
fn strip_uci_secret(data: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("option secret ")
            || trimmed.starts_with("option secret=")
            || trimmed.starts_with("list secret ")
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.into_bytes()
}

/// Collects the whitelisted files that exist under `data_dir`.
pub fn collect(data_dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut entries = Vec::new();
    for name in ["config.yaml", "profiles.json", "mixin.yaml", "index.json"] {
        if let Some(data) = read_file(&data_dir.join(name)) {
            entries.push((name.to_string(), data));
        }
    }
    if let Ok(list) = std::fs::read_dir(data_dir.join("profiles")) {
        for item in list.flatten() {
            let path = item.path();
            if !path.is_file() {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|v| v.to_str()) else { continue };
            // Skip write_atomic temp files (".<name>.<pid>.<n>.tmp").
            if file_name.starts_with('.') {
                continue;
            }
            if let Some(data) = read_file(&path) {
                entries.push((format!("profiles/{}", file_name), data));
            }
        }
    }
    if let Ok(output) = std::process::Command::new("uci").args(["export", "flclash"]).output() {
        if output.status.success() && !output.stdout.is_empty() {
            entries.push(("uci-flclash.txt".into(), strip_uci_secret(&output.stdout)));
        }
    }
    entries
}

pub fn dir(st: &crate::state::AppState) -> PathBuf {
    std::path::Path::new(&st.data_dir).join("backups")
}

/// Creates `<data>/backups/<label-ts>.tar.gz` and returns its metadata.
pub fn snapshot(st: &crate::state::AppState, label: &str, prefix: &str) -> Result<Value, String> {
    let entries = collect(std::path::Path::new(&st.data_dir));
    if entries.is_empty() {
        return Err("没有可备份的配置文件".into());
    }
    let folder = dir(st);
    std::fs::create_dir_all(&folder).map_err(|e| format!("创建备份目录失败: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700));
    }
    let ts = crate::state::now_secs();
    let safe_label: String = label
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(24)
        .collect();
    let label_part = if prefix.is_empty() {
        if safe_label.is_empty() { "manual".to_string() } else { safe_label }
    } else {
        prefix.trim_end_matches('-').to_string()
    };
    let name = format!("{}-{}.tar.gz", label_part, ts);
    let bytes = pack(&entries);
    crate::config::write_atomic(&folder.join(&name), &bytes)?;
    prune(&folder);
    Ok(json!({
        "name": name,
        "size": bytes.len(),
        "files": entries.len(),
        "createdAt": ts,
    }))
}

fn prune(folder: &Path) {
    let Ok(list) = std::fs::read_dir(folder) else { return };
    let mut items: Vec<(i64, PathBuf)> = list
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.ends_with(".tar.gz") {
                return None;
            }
            let modified = item.metadata().ok()?.modified().ok()?;
            let ts = modified
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            Some((ts, path))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in items.into_iter().skip(KEEP_SNAPSHOTS) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn list(st: &crate::state::AppState) -> Value {
    let folder = dir(st);
    let Ok(list) = std::fs::read_dir(&folder) else {
        return json!({"snapshots": [], "dir": folder});
    };
    let mut items: Vec<Value> = list
        .flatten()
        .filter_map(|item| {
            let path = item.path();
            let name = path.file_name()?.to_str()?.to_string();
            if !name.ends_with(".tar.gz") {
                return None;
            }
            let meta = item.metadata().ok()?;
            let ts = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or_else(crate::state::now_secs);
            Some(json!({
                "name": name,
                "size": meta.len(),
                "createdAt": ts,
            }))
        })
        .collect();
    items.sort_by(|a, b| {
        b["createdAt"].as_i64().unwrap_or(0).cmp(&a["createdAt"].as_i64().unwrap_or(0))
    });
    json!({"snapshots": items, "dir": folder})
}

/// Restores an archive into `data_dir` after validating every entry.
/// The previous state is snapshotted first; the caller reloads the core.
pub fn restore(st: &crate::state::AppState, data: &[u8]) -> Result<Value, String> {
    let entries = unpack(data)?;
    let mut parsed = 0usize;
    for (name, body) in &entries {
        if name.ends_with(".yaml") || name.ends_with(".json") {
            if name.ends_with(".json") {
                serde_json::from_slice::<Value>(body)
                    .map_err(|e| format!("{} 校验失败: {}", name, e))?;
            } else {
                let doc: serde_yaml::Value = serde_yaml::from_slice(body)
                    .map_err(|e| format!("{} 不是有效的 YAML: {}", name, e))?;
                if !doc.is_mapping() {
                    return Err(format!("{} 顶层必须是映射", name));
                }
            }
            parsed += 1;
        }
    }
    if parsed == 0 {
        return Err("备份中没有可识别的配置".into());
    }

    // Pre-restore snapshot keeps the current data recoverable.
    let _ = snapshot(st, "pre-restore", "pre-restore-");
    std::fs::create_dir_all(&st.data_dir).map_err(|e| format!("创建数据目录失败: {}", e))?;

    let staging = std::path::Path::new(&st.data_dir).join(".restore");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| format!("创建暂存目录失败: {}", e))?;
    for (name, body) in &entries {
        let rel = safe_rel(name).ok_or_else(|| format!("非法路径: {}", name))?;
        let target = staging.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
        }
        std::fs::write(&target, body).map_err(|e| format!("写入失败: {}", e))?;
    }

    // Validate the restored config before it becomes live. The candidate lives
    // in data_dir so relative rule-provider/Geo paths resolve like the real
    // config; a failed preflight leaves the current config untouched.
    let staged_config = staging.join("config.yaml");
    if staged_config.is_file() {
        let body = std::fs::read(&staged_config).map_err(|e| format!("读取备份配置失败: {e}"))?;
        let candidate = std::path::Path::new(&st.data_dir).join("config.yaml.restore-candidate");
        crate::config::write_atomic(&candidate, &body)?;
        let check = crate::preflight::validate_config_blocking(&candidate);
        let _ = std::fs::remove_file(&candidate);
        if let Err(err) = check {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(format!("备份中的配置预检失败: {err}"));
        }
    }

    // Move into place file by file; a failure leaves the previous file intact
    // until its own rename succeeds.
    let mut restored = Vec::new();
    for (name, _) in &entries {
        let rel = safe_rel(name).ok_or_else(|| format!("非法路径: {}", name))?;
        let source = staging.join(&rel);
        let target = std::path::Path::new(&st.data_dir).join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::rename(&source, &target).map_err(|e| format!("应用 {} 失败: {}", name, e))?;
        restored.push(name.clone());
    }
    let _ = std::fs::remove_dir_all(&staging);

    // Re-import the UCI half of the snapshot (ACL lists, ports, core choice).
    // Only a genuine flclash export is accepted, and `uci import flclash`
    // cannot touch any other package regardless of the payload.
    let mut uci_restored = false;
    if let Some((_, body)) = entries.iter().find(|(name, _)| name == "uci-flclash.txt") {
        if body.starts_with(b"package flclash") {
            let body = strip_uci_secret(body);
            let import = std::process::Command::new("uci")
                .args(["import", "flclash"])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            if let Ok(mut child) = import {
                if let Some(stdin) = child.stdin.as_mut() {
                    let _ = stdin.write_all(&body);
                }
                let ok = child.wait().map(|s| s.success()).unwrap_or(false);
                if ok {
                    let _ = std::process::Command::new("uci")
                        .args(["commit", "flclash"])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                    uci_restored = true;
                }
            }
        }
    }

    Ok(json!({
        "ok": true,
        "files": restored,
        "uciRestored": uci_restored,
        "bytes": data.len(),
    }))
}

/// Resolves a snapshot name inside the backup folder (no traversal).
pub fn snapshot_path(st: &crate::state::AppState, name: &str) -> Option<PathBuf> {
    if !name.ends_with(".tar.gz") || name.contains('/') || name.contains('\\') || name.contains("..") {
        return None;
    }
    let path = dir(st).join(name);
    path.is_file().then_some(path)
}

pub fn read_snapshot(st: &crate::state::AppState, name: &str) -> Result<Vec<u8>, String> {
    let path = snapshot_path(st, name).ok_or_else(|| format!("备份不存在: {}", name))?;
    std::fs::read(&path).map_err(|e| format!("读取备份失败: {}", e))
}

pub fn delete(st: &crate::state::AppState, name: &str) -> Result<(), String> {
    let path = snapshot_path(st, name).ok_or_else(|| format!("备份不存在: {}", name))?;
    std::fs::remove_file(path).map_err(|e| format!("删除失败: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_unpack_roundtrip() {
        let entries = vec![
            ("config.yaml".to_string(), b"mixed-port: 7892\n".to_vec()),
            ("profiles/sub1.yaml".to_string(), b"proxies: []\n".to_vec()),
            ("long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/file.yaml"
                .replace("long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/long/", "profiles/"), vec![0u8; 3]),
        ];
        let bytes = pack(&entries);
        let back = unpack(&bytes).expect("unpack");
        assert_eq!(back.len(), 3);
        assert_eq!(back[0].1, entries[0].1);
    }

    #[test]
    fn rejects_traversal_and_unknown_files() {
        let bad = vec![
            ("../etc/passwd".to_string(), vec![1u8]),
        ];
        let bytes = pack(&bad);
        let err = unpack(&bytes).unwrap_err();
        assert!(err.contains("不允许"));

        let unknown = vec![("authorized_keys".to_string(), vec![1u8])];
        assert!(unpack(&pack(&unknown)).is_err());
    }

    #[test]
    fn rejects_invalid_yaml() {
        let entries = vec![("config.yaml".to_string(), b"a: [1,\n".to_vec())];
        let bytes = pack(&entries);
        let st = crate::state::AppState::new(
            "127.0.0.1:9".into(),
            "/tmp/none".into(),
            "/tmp/flclash-backup-test".into(),
            "/bin/true".into(),
            None,
        );
        let err = restore(&st, &bytes).unwrap_err();
        assert!(err.contains("YAML"));
        let _ = std::fs::remove_dir_all("/tmp/flclash-backup-test");
    }

    #[test]
    fn uci_export_drops_api_secret() {
        let body = b"package flclash\n\nconfig flclash 'api'\n\toption listen '0.0.0.0:9098'\n\toption secret 'deadbeef'\n\toption secret_file '/etc/flclash/api.secret'\n";
        let out = String::from_utf8(strip_uci_secret(body)).unwrap();
        assert!(!out.contains("deadbeef"));
        assert!(!out.contains("option secret '"));
        assert!(out.contains("option secret_file"));
        assert!(out.contains("option listen"));
    }

    #[test]
    fn rejects_corrupted_header_checksum() {
        let entries = vec![("config.yaml".to_string(), b"mixed-port: 7892\n".to_vec())];
        let gz = pack(&entries);
        let mut dec = GzDecoder::new(&gz[..]);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).unwrap();
        // Flip a byte inside the ustar name field: the header checksum no
        // longer matches, so the archive must be rejected before unpacking.
        raw[0] ^= 0xff;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&raw).unwrap();
        let bad = enc.finish().unwrap();
        let err = unpack(&bad).unwrap_err();
        assert!(err.contains("校验和"), "{}", err);
    }
}
