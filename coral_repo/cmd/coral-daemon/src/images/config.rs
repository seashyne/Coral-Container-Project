//! OCI Image Config (`image-config.json`) persistence and Docker-compatible
//! `ENTRYPOINT`, `CMD`, `ENV`, `WORKDIR`, `USER`, `STOPSIGNAL` inheritance.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const IMAGE_CONFIG_FILE: &str = "image-config.json";

/// Subset of the OCI Image Specification v1.0 `config` object honored when running an image.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase", default)]
pub struct OciImageConfig {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub working_dir: String,
    pub user: String,
    pub labels: HashMap<String, String>,
    pub stop_signal: String,
    #[serde(default, deserialize_with = "deserialize_vec_or_map_keys")]
    pub exposed_ports: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_vec_or_map_keys")]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub shell: Vec<String>,
    #[serde(default)]
    pub onbuild: Vec<String>,
    #[serde(default)]
    pub healthcheck_cmd: Option<String>,
    #[serde(default)]
    pub healthcheck_interval_ms: u64,
    #[serde(default)]
    pub healthcheck_retries: u32,
}

fn deserialize_vec_or_map_keys<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = serde_json::Value::deserialize(deserializer)?;
    match val {
        serde_json::Value::Array(arr) => Ok(arr
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()),
        serde_json::Value::Object(map) => {
            let mut keys: Vec<String> = map.into_iter().map(|(k, _)| k).collect();
            keys.sort();
            Ok(keys)
        }
        _ => Ok(Vec::new()),
    }
}

impl OciImageConfig {
    /// Parses `OciImageConfig` from an OCI / Docker Registry v2 image config JSON blob
    /// (supporting both top-level `{"config": {...}}` and unwrapped `{...}`).
    pub fn from_oci_blob(bytes: &[u8]) -> Result<Self> {
        let root: serde_json::Value =
            serde_json::from_slice(bytes).context("parse OCI image config JSON")?;
        let cfg_val = root.get("config").unwrap_or(&root);
        let mut parsed: Self =
            serde_json::from_value(cfg_val.clone()).unwrap_or_default();
        if parsed.stop_signal.is_empty()
            && let Some(sig) = cfg_val.get("StopSignal").and_then(|v| v.as_str())
        {
            parsed.stop_signal = sig.to_string();
        }
        if parsed.onbuild.is_empty()
            && let Some(arr) = cfg_val.get("OnBuild").and_then(|v| v.as_array())
        {
            parsed.onbuild = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
        if parsed.healthcheck_cmd.is_none()
            && let Some(hc) = cfg_val.get("Healthcheck")
            && let Some(test_arr) = hc.get("Test").and_then(|v| v.as_array())
        {
            let tokens: Vec<&str> = test_arr.iter().filter_map(|v| v.as_str()).collect();
            if matches!(tokens.first().copied(), Some("CMD-SHELL" | "CMD")) && tokens.len() >= 2 {
                parsed.healthcheck_cmd = Some(tokens[1..].join(" "));
            }
            if let Some(ns) = hc.get("Interval").and_then(|v| v.as_u64())
                && ns > 0
            {
                parsed.healthcheck_interval_ms = (ns / 1_000_000).max(500);
            }
            if let Some(r) = hc.get("Retries").and_then(|v| v.as_u64())
                && r > 0
            {
                parsed.healthcheck_retries = r as u32;
            }
        }
        Ok(parsed)
    }

    /// Loads `image-config.json` adjacent to an extracted `rootfs` directory (or inside `rootfs/../`).
    pub fn load_for_rootfs(rootfs: &Path) -> Self {
        let Some(path) = config_path_for_rootfs(rootfs) else {
            return Self::default();
        };
        let Ok(raw) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Saves `image-config.json` alongside an image's `rootfs` directory.
    pub fn save_for_rootfs(&self, rootfs: &Path) -> Result<()> {
        let Some(path) = config_path_for_rootfs(rootfs) else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, format!("{json}\n"))
            .with_context(|| format!("write '{}'", path.display()))?;
        Ok(())
    }
}

pub fn config_path_for_rootfs(rootfs: &Path) -> Option<PathBuf> {
    rootfs.parent().map(|p| p.join(IMAGE_CONFIG_FILE))
}

/// Applies Docker/OCI command resolution semantics:
/// - Explicit `--entrypoint` replaces image `ENTRYPOINT` (`[]` or `[""]` clears it).
/// - If user `cmd` is non-empty, final argv = `entrypoint + user_cmd`.
/// - If user `cmd` is empty, final argv = `entrypoint + image.cmd` (or `["/bin/sh"]` if both are empty).
pub fn resolve_entrypoint_and_cmd(
    img: &OciImageConfig,
    entrypoint_override: Option<&[String]>,
    user_cmd: &[String],
) -> Vec<String> {
    let ep: &[String] = match entrypoint_override {
        Some(ov) if ov.len() == 1 && ov[0].is_empty() => &[],
        Some(ov) => ov,
        None => &img.entrypoint,
    };

    let cmd: &[String] = if !user_cmd.is_empty() {
        user_cmd
    } else {
        &img.cmd
    };

    let mut out = Vec::with_capacity(ep.len() + cmd.len());
    out.extend(ep.iter().cloned());
    out.extend(cmd.iter().cloned());
    if out.is_empty() {
        out.push("/bin/sh".into());
    }
    out
}

/// Merges base OCI spec env, image `ENV` entries, and CLI `-e KEY=VAL` overrides (later wins).
pub fn merge_env_vars(base: &[String], image_envs: &[String], user_envs: &[String]) -> Vec<String> {
    let mut order = Vec::<String>::new();
    let mut map = HashMap::<String, String>::new();

    for entry in base.iter().chain(image_envs.iter()).chain(user_envs.iter()) {
        if let Some((k, v)) = entry.split_once('=') {
            let key = k.trim().to_string();
            if !map.contains_key(&key) {
                order.push(key.clone());
            }
            map.insert(key, v.to_string());
        } else if !entry.trim().is_empty() {
            let key = entry.trim().to_string();
            if let Ok(val) = std::env::var(&key) {
                if !map.contains_key(&key) {
                    order.push(key.clone());
                }
                map.insert(key, val);
            }
        }
    }

    order
        .into_iter()
        .filter_map(|k| map.remove(&k).map(|v| format!("{k}={v}")))
        .collect()
}

/// Resolves numeric `uid:gid` (or username/groupname against `<rootfs>/etc/passwd` & `/etc/group`).
/// Returns `None` if no non-empty user was requested.
pub fn resolve_container_user(rootfs: &Path, user_spec: &str) -> Option<(u32, u32, Option<String>)> {
    let raw = user_spec.trim();
    if raw.is_empty() {
        return None;
    }
    let (u_part, g_part) = match raw.split_once(':') {
        Some((u, g)) => (u.trim(), Some(g.trim())),
        None => (raw, None),
    };

    let mut resolved_uid: Option<u32> = u_part.parse::<u32>().ok();
    let mut resolved_gid: Option<u32> = g_part.and_then(|g| g.parse::<u32>().ok());
    let mut resolved_home: Option<String> = None;

    if let Ok(passwd) = fs::read_to_string(rootfs.join("etc/passwd")) {
        for line in passwd.lines() {
            let cols: Vec<&str> = line.split(':').collect();
            if cols.len() >= 6 {
                let name = cols[0];
                let uid_ok = cols[2].parse::<u32>().ok();
                let gid_ok = cols[3].parse::<u32>().ok();
                let matched = match resolved_uid {
                    Some(u) => uid_ok == Some(u),
                    None => name == u_part,
                };
                if matched {
                    resolved_uid = resolved_uid.or(uid_ok);
                    if resolved_gid.is_none() && g_part.is_none() {
                        resolved_gid = gid_ok;
                    }
                    if !cols[5].trim().is_empty() {
                        resolved_home = Some(cols[5].trim().to_string());
                    }
                    break;
                }
            }
        }
    }

    if resolved_gid.is_none()
        && let Some(gname) = g_part
        && let Ok(group) = fs::read_to_string(rootfs.join("etc/group"))
    {
        for line in group.lines() {
            let cols: Vec<&str> = line.split(':').collect();
            if cols.len() >= 3 && cols[0] == gname {
                resolved_gid = cols[2].parse::<u32>().ok();
                break;
            }
        }
    }

    let uid = resolved_uid.unwrap_or(0);
    let gid = resolved_gid.unwrap_or(uid);
    Some((uid, gid, resolved_home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_nginx_and_python_entrypoint_cmd_and_env() {
        let blob = br#"{
          "config": {
            "Env": ["PATH=/usr/local/bin:/usr/bin:/bin", "NGINX_VERSION=1.27.1"],
            "Entrypoint": ["/docker-entrypoint.sh"],
            "Cmd": ["nginx", "-g", "daemon off;"],
            "WorkingDir": "/usr/share/nginx/html",
            "User": "101:101"
          }
        }"#;
        let cfg = OciImageConfig::from_oci_blob(blob).unwrap();
        assert_eq!(
            resolve_entrypoint_and_cmd(&cfg, None, &[]),
            vec!["/docker-entrypoint.sh", "nginx", "-g", "daemon off;"]
        );
        assert_eq!(
            resolve_entrypoint_and_cmd(&cfg, None, &["nginx".into(), "-v".into()]),
            vec!["/docker-entrypoint.sh", "nginx", "-v"]
        );
        assert_eq!(
            resolve_entrypoint_and_cmd(&cfg, Some(&["/bin/sh".into()]), &["-c".into(), "id".into()]),
            vec!["/bin/sh", "-c", "id"]
        );

        let merged = merge_env_vars(
            &["PATH=/bin".into(), "TERM=xterm".into()],
            &cfg.env,
            &["NGINX_VERSION=override".into()],
        );
        assert!(merged.contains(&"PATH=/usr/local/bin:/usr/bin:/bin".to_string()));
        assert!(merged.contains(&"NGINX_VERSION=override".to_string()));
    }

    #[test]
    fn persists_image_config_and_resolves_rootfs_users() {
        let dir = tempdir().unwrap();
        let rootfs = dir.path().join("node-22/rootfs");
        fs::create_dir_all(rootfs.join("etc")).unwrap();
        fs::write(
            rootfs.join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh\nnode:x:1000:1000:Linux User:/home/node:/bin/sh\n",
        )
        .unwrap();

        let cfg = OciImageConfig {
            entrypoint: vec!["docker-entrypoint.sh".into()],
            cmd: vec!["node".into()],
            working_dir: "/app".into(),
            user: "node".into(),
            ..OciImageConfig::default()
        };
        cfg.save_for_rootfs(&rootfs).unwrap();
        let loaded = OciImageConfig::load_for_rootfs(&rootfs);
        assert_eq!(loaded, cfg);

        let (uid, gid, home) = resolve_container_user(&rootfs, &loaded.user).unwrap();
        assert_eq!((uid, gid), (1000, 1000));
        assert_eq!(home.as_deref(), Some("/home/node"));
    }
}
