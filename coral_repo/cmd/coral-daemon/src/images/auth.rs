//! OCI Registry credential store (`~/.config/coral/auth.json`, mode `0600`) for
//! `coral login`, `coral logout`, private `coral pull`, and `coral push`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AuthFile {
    #[serde(default)]
    auths: BTreeMap<String, AuthEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthEntry {
    auth: String,
}

/// Resolves the auth file path (`CORAL_AUTH_FILE` env override, else `$XDG_CONFIG_HOME/coral/auth.json`
/// or `$HOME/.config/coral/auth.json`).
pub fn default_auth_file() -> PathBuf {
    if let Ok(explicit) = std::env::var("CORAL_AUTH_FILE")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral").join("auth.json");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".config")
            .join("coral")
            .join("auth.json");
    }
    coral_runtime::default_state_root().join("auth.json")
}

/// Canonicalizes registry hostnames so `docker.io`, `index.docker.io`, and `registry-1.docker.io`
/// share a single credential entry.
pub fn normalize_registry_key(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let host = trimmed.split('/').next().unwrap_or(trimmed);
    match host {
        "" | "docker.io" | "index.docker.io" | "registry-1.docker.io" => {
            "registry-1.docker.io".to_string()
        }
        other => other.to_string(),
    }
}

/// Returns `"http"` for local loopback registries (`localhost:*`, `127.0.0.1:*`, `[::1]:*`)
/// and `"https"` for all remote registries.
pub fn registry_scheme(registry: &str) -> &'static str {
    let lower = registry.trim().to_ascii_lowercase();
    if lower == "localhost"
        || lower.starts_with("localhost:")
        || lower == "127.0.0.1"
        || lower.starts_with("127.0.0.1:")
        || lower.starts_with("[::1]")
    {
        "http"
    } else {
        "https"
    }
}

pub fn login_registry(registry: &str, username: &str, password: &str) -> Result<String> {
    login_registry_at(&default_auth_file(), registry, username, password)
}

pub fn login_registry_at(
    auth_path: &Path,
    registry: &str,
    username: &str,
    password: &str,
) -> Result<String> {
    let user = username.trim();
    if user.is_empty() {
        bail!("username must not be empty");
    }
    if password.is_empty() {
        bail!("password / access token must not be empty");
    }
    let key = normalize_registry_key(registry);
    let mut file = load_auth_file(auth_path);
    let encoded = base64_encode(format!("{user}:{password}").as_bytes());
    file.auths.insert(key.clone(), AuthEntry { auth: encoded });
    save_auth_file(auth_path, &file)?;
    Ok(key)
}

pub fn logout_registry(registry: &str) -> Result<bool> {
    logout_registry_at(&default_auth_file(), registry)
}

pub fn logout_registry_at(auth_path: &Path, registry: &str) -> Result<bool> {
    let key = normalize_registry_key(registry);
    let mut file = load_auth_file(auth_path);
    let existed = file.auths.remove(&key).is_some();
    if existed {
        save_auth_file(auth_path, &file)?;
    }
    Ok(existed)
}

pub fn load_registry_auth(registry: &str) -> Option<(String, String)> {
    load_registry_auth_at(&default_auth_file(), registry)
}

pub fn load_registry_auth_at(auth_path: &Path, registry: &str) -> Option<(String, String)> {
    let key = normalize_registry_key(registry);
    let file = load_auth_file(auth_path);
    let entry = file.auths.get(&key)?;
    let bytes = base64_decode(&entry.auth)?;
    let decoded = String::from_utf8(bytes).ok()?;
    let (u, p) = decoded.split_once(':')?;
    Some((u.to_string(), p.to_string()))
}

pub fn list_logged_in_registries() -> Vec<(String, String)> {
    let file = load_auth_file(&default_auth_file());
    file.auths
        .into_iter()
        .filter_map(|(reg, entry)| {
            let decoded = String::from_utf8(base64_decode(&entry.auth)?).ok()?;
            let (u, _) = decoded.split_once(':')?;
            Some((reg, u.to_string()))
        })
        .collect()
}

fn load_auth_file(path: &Path) -> AuthFile {
    let Ok(raw) = fs::read_to_string(path) else {
        return AuthFile::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_auth_file(path: &Path, file: &AuthFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create auth config dir '{}'", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(file)?;
    fs::write(&tmp, format!("{json}\n"))
        .with_context(|| format!("write temp auth file '{}'", tmp.display()))?;
    let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    fs::rename(&tmp, path).with_context(|| format!("install auth file '{}'", path.display()))?;
    Ok(())
}

pub(crate) fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

pub(crate) fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !cleaned.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity((cleaned.len() / 4) * 3);
    for chunk in cleaned.as_chunks::<4>().0 {
        let v0 = val(chunk[0])? as u32;
        let v1 = val(chunk[1])? as u32;
        let pad2 = chunk[2] == b'=';
        let pad3 = chunk[3] == b'=';
        let v2 = if pad2 { 0 } else { val(chunk[2])? as u32 };
        let v3 = if pad3 { 0 } else { val(chunk[3])? as u32 };
        let n = (v0 << 18) | (v1 << 12) | (v2 << 6) | v3;
        out.push(((n >> 16) & 0xff) as u8);
        if !pad2 {
            out.push(((n >> 8) & 0xff) as u8);
        }
        if !pad3 {
            out.push((n & 0xff) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn base64_and_auth_store_roundtrip() {
        let enc = base64_encode(b"alice:s3cr3t!");
        assert_eq!(base64_decode(&enc).unwrap(), b"alice:s3cr3t!");

        let dir = tempdir().unwrap();
        let path = dir.path().join("auth.json");
        let reg = login_registry_at(&path, "docker.io", "seashyne", "tok_123").unwrap();
        assert_eq!(reg, "registry-1.docker.io");
        assert_eq!(
            load_registry_auth_at(&path, "index.docker.io"),
            Some(("seashyne".into(), "tok_123".into()))
        );
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(logout_registry_at(&path, "docker.io").unwrap());
        assert!(load_registry_auth_at(&path, "registry-1.docker.io").is_none());
    }
}
