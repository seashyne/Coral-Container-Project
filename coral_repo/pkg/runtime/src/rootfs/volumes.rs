//! Persistent Named Volumes (`coral volume`) and Docker-style `-v` bind/volume spec parsing.

use crate::RuntimeError;
use coral_oci::Mount;
use std::fs;
use std::path::{Path, PathBuf};

/// Resolves the default persistent directory for Coral named volumes.
/// - `CORAL_VOLUMES_DIR` environment variable when set
/// - Root (`euid == 0`): `/var/lib/coral/volumes`
/// - Rootless: `$XDG_STATE_HOME/coral/volumes` or `$HOME/.local/state/coral/volumes`
pub fn default_volumes_dir() -> PathBuf {
    if let Ok(custom) = std::env::var("CORAL_VOLUMES_DIR")
        && !custom.trim().is_empty()
    {
        return PathBuf::from(custom);
    }
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return PathBuf::from("/var/lib/coral/volumes");
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral").join("volumes");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("coral")
            .join("volumes");
    }
    crate::default_state_root().join("volumes")
}

pub fn is_valid_volume_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Creates (or ensures) a named volume `<volumes_dir>/<name>/_data` and returns its data path.
pub fn ensure_named_volume(volumes_dir: &Path, name: &str) -> Result<PathBuf, RuntimeError> {
    if !is_valid_volume_name(name) {
        return Err(RuntimeError::InitFailed(format!(
            "invalid volume name '{name}' (must start with alphanumeric and contain only [a-zA-Z0-9_.-])"
        )));
    }
    let data_dir = volumes_dir.join(name).join("_data");
    fs::create_dir_all(&data_dir)
        .map_err(|e| RuntimeError::sys(format!("create named volume '{name}'"), e))?;
    fs::canonicalize(&data_dir)
        .map_err(|e| RuntimeError::sys(format!("resolve named volume '{name}'"), e))
}

/// Lists all named volumes in `volumes_dir` as `(name, data_path)` sorted by name.
pub fn list_named_volumes(volumes_dir: &Path) -> Result<Vec<(String, PathBuf)>, RuntimeError> {
    if !volumes_dir.exists() {
        return Ok(Vec::new());
    }
    let mut list = Vec::new();
    for entry in fs::read_dir(volumes_dir)
        .map_err(|e| RuntimeError::sys("read volumes dir", e))?
        .flatten()
    {
        let p = entry.path();
        if p.is_dir()
            && let Some(name) = entry.file_name().to_str()
            && is_valid_volume_name(name)
        {
            let data = p.join("_data");
            let resolved = fs::canonicalize(&data).unwrap_or(data);
            list.push((name.to_string(), resolved));
        }
    }
    list.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(list)
}

/// Removes a named volume `<volumes_dir>/<name>`.
pub fn remove_named_volume(volumes_dir: &Path, name: &str) -> Result<(), RuntimeError> {
    if !is_valid_volume_name(name) {
        return Err(RuntimeError::InitFailed(format!(
            "invalid volume name '{name}'"
        )));
    }
    let dir = volumes_dir.join(name);
    if !dir.exists() {
        return Err(RuntimeError::NotFound(format!("volume '{name}'")));
    }
    crate::multi_uid::remove_dir_all_rootless_safe(&dir)
        .map_err(|e| RuntimeError::sys(format!("remove volume '{name}'"), e))
}

/// Parses a Docker-style volume specification `HOST_PATH_OR_VOLUME_NAME:CONTAINER_PATH[:ro|:rw]`
/// into an OCI `Mount`. If the source does not contain `/` and is a valid volume name,
/// automatically provisions a persistent named volume under `default_volumes_dir()`.
pub fn parse_volume_spec(raw: &str) -> Result<Mount, RuntimeError> {
    parse_volume_spec_in(raw, &default_volumes_dir())
}

pub fn parse_volume_spec_in(raw: &str, volumes_dir: &Path) -> Result<Mount, RuntimeError> {
    let parts: Vec<&str> = raw.split(':').collect();
    let (host, dest, ro) = match parts.as_slice() {
        [h, d] => (*h, *d, false),
        [h, d, mode] => {
            let ro = match *mode {
                "ro" => true,
                "rw" => false,
                other => {
                    return Err(RuntimeError::InitFailed(format!(
                        "invalid volume mode '{other}' in '{raw}' (expected 'ro' or 'rw')"
                    )));
                }
            };
            (*h, *d, ro)
        }
        _ => {
            return Err(RuntimeError::InitFailed(format!(
                "invalid volume spec '{raw}' (expected SOURCE:CONTAINER_PATH[:ro|:rw])"
            )));
        }
    };

    let host = host.trim();
    if host.is_empty() || !dest.starts_with('/') {
        return Err(RuntimeError::InitFailed(format!(
            "invalid volume spec '{raw}' (container destination must be an absolute path)"
        )));
    }
    let abs_host = if !host.contains('/') && host != "." && host != ".." && is_valid_volume_name(host) {
        ensure_named_volume(volumes_dir, host)?
    } else {
        fs::canonicalize(Path::new(host))
            .map_err(|e| RuntimeError::sys(format!("resolve host volume path '{host}'"), e))?
    };
    let mut options = vec!["rbind".to_string()];
    options.push(if ro { "ro".to_string() } else { "rw".to_string() });

    Ok(Mount {
        destination: dest.to_string(),
        typ: Some("bind".to_string()),
        source: Some(abs_host.to_string_lossy().into_owned()),
        options,
    })
}
