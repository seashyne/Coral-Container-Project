//! Persistent OCI image store (`images.json`), local rootfs registration, hardlink-deduplicated
//! directory size calculation, and sub-modules for OCI Registry v2 pull and layer extraction.

pub mod archive;
pub mod auth;
pub mod config;
pub mod extract;
pub mod progress;
pub mod push;
pub mod reference;
pub mod registry;
pub mod security;

pub use archive::{load_image_archive, save_image_archive, tag_image};
pub use auth::{list_logged_in_registries, load_registry_auth, login_registry, logout_registry};
pub use config::{
    OciImageConfig, merge_env_vars, resolve_container_user, resolve_entrypoint_and_cmd,
};
pub use extract::extract_layer;
pub use progress::{PullPhase, PullProgress, PullProgressReporter, render_line};
pub use push::{PushResult, push_image};
pub use reference::{ImageRef, normalize_lookup_tag};
pub use registry::pull_image;
pub use security::{
    ImageSignatureEnvelope, KeypairReport, VerifyReport, compute_image_tree_digest,
    default_key_paths, generate_keypair, generate_spdx_sbom, sign_image, verify_image,
};

use anyhow::{Context, Result, bail};
use coral_api::Image;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Persistent image record stored in `<images_dir>/images.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredImage {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: u64,
    pub rootfs_path: String,
    /// True when the rootfs directory was created/extracted by `coral-daemon` inside `images_dir`
    /// and may safely be deleted when the image is removed.
    #[serde(default)]
    pub managed: bool,
}

impl From<&StoredImage> for Image {
    fn from(s: &StoredImage) -> Self {
        Self {
            id: s.id.clone(),
            repo_tags: s.repo_tags.clone(),
            size: s.size,
            rootfs_path: s.rootfs_path.clone(),
        }
    }
}

/// Resolves the default persistent directory for pulled Linux base images.
/// - Root (`euid == 0`): `/var/lib/coral/images`
/// - Rootless: `$XDG_STATE_HOME/coral/images` or `$HOME/.local/state/coral/images`
pub fn default_images_dir() -> PathBuf {
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return PathBuf::from("/var/lib/coral/images");
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral").join("images");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("coral")
            .join("images");
    }
    coral_runtime::default_state_root().join("images")
}

/// Loads persisted image metadata from `<images_dir>/images.json`, dropping missing rootfs dirs.
pub fn load_index(images_dir: &Path) -> Vec<StoredImage> {
    let path = images_dir.join("images.json");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(items) = serde_json::from_str::<Vec<StoredImage>>(&raw) else {
        return Vec::new();
    };
    items
        .into_iter()
        .filter(|img| Path::new(&img.rootfs_path).is_dir())
        .collect()
}

/// Atomically persists image metadata to `<images_dir>/images.json`.
pub fn save_index(images_dir: &Path, images: &[StoredImage]) -> Result<()> {
    fs::create_dir_all(images_dir)
        .with_context(|| format!("create images dir '{}'", images_dir.display()))?;
    let tmp = images_dir.join("images.json.tmp");
    let target = images_dir.join("images.json");
    let json = serde_json::to_string_pretty(images)?;
    fs::write(&tmp, format!("{json}\n"))
        .with_context(|| format!("write '{}'", tmp.display()))?;
    fs::rename(&tmp, &target).with_context(|| format!("rename '{}'", target.display()))?;
    Ok(())
}

/// Finds an image in `images` by ID, exact tag, or normalized tag (e.g. `"alpine"` -> `"alpine:latest"`).
pub fn find_image<'a>(images: &'a [StoredImage], query: &str) -> Option<&'a StoredImage> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = normalize_lookup_tag(trimmed);
    images.iter().find(|img| {
        img.id == trimmed
            || img
                .repo_tags
                .iter()
                .any(|t| t == trimmed || normalize_lookup_tag(t) == normalized)
    })
}

/// Inserts or replaces an image in `images`, returning any orphaned managed rootfs paths.
pub fn upsert_image(images: &mut Vec<StoredImage>, new_img: StoredImage) -> Vec<String> {
    let new_tags: Vec<String> = new_img
        .repo_tags
        .iter()
        .map(|t| normalize_lookup_tag(t))
        .collect();
    let mut orphaned_managed = Vec::new();

    images.retain_mut(|existing| {
        existing
            .repo_tags
            .retain(|t| !new_tags.contains(&normalize_lookup_tag(t)));
        let drop_it = existing.repo_tags.is_empty();
        if drop_it && existing.managed {
            orphaned_managed.push(existing.rootfs_path.clone());
        }
        !drop_it
    });

    images.push(new_img);
    orphaned_managed
}

/// Removes any image matching `query` (by ID or tag), returning `(removed_count, orphaned_managed_paths)`.
pub fn remove_matching_images(images: &mut Vec<StoredImage>, query: &str) -> (usize, Vec<String>) {
    let trimmed = query.trim();
    let normalized = normalize_lookup_tag(trimmed);
    let mut removed_count = 0usize;
    let mut removed_managed = Vec::new();

    images.retain(|img| {
        let matched = img.id == trimmed
            || img
                .repo_tags
                .iter()
                .any(|t| t == trimmed || normalize_lookup_tag(t) == normalized);
        if matched {
            removed_count += 1;
            if img.managed {
                removed_managed.push(img.rootfs_path.clone());
            }
        }
        !matched
    });

    (removed_count, removed_managed)
}

/// Deletes a managed rootfs directory under `images_dir` if no remaining image references it.
pub fn cleanup_unused_managed_dir(
    images_dir: &Path,
    remaining: &[StoredImage],
    rootfs_path: &str,
) -> Result<()> {
    if remaining.iter().any(|img| img.rootfs_path == rootfs_path) {
        return Ok(());
    }
    let Ok(images_dir_abs) = fs::canonicalize(images_dir) else {
        return Ok(());
    };
    let Ok(rootfs_abs) = fs::canonicalize(Path::new(rootfs_path)) else {
        return Ok(());
    };
    if !rootfs_abs.starts_with(&images_dir_abs) || rootfs_abs == images_dir_abs {
        return Ok(());
    }
    let target = match rootfs_abs.parent() {
        Some(parent) if parent != images_dir_abs && parent.parent() == Some(&images_dir_abs) => {
            parent.to_path_buf()
        }
        _ => rootfs_abs,
    };
    remove_dir_force(&target)
}

/// Builds a `StoredImage` record for an existing local rootfs directory (unmanaged).
pub fn register_local_rootfs(image_name: &str, rootfs: &Path) -> Result<StoredImage> {
    let abs = fs::canonicalize(rootfs)
        .with_context(|| format!("rootfs '{}' does not exist", rootfs.display()))?;
    if !abs.is_dir() {
        bail!("rootfs '{}' is not a directory", rootfs.display());
    }
    let size = dir_size(&abs);
    Ok(StoredImage {
        id: format!("coral-local/{image_name}"),
        repo_tags: vec![image_name.to_string()],
        size,
        rootfs_path: abs.to_string_lossy().into_owned(),
        managed: false,
    })
}

/// Prepares the empty `scratch` base image directory inside `images_dir`.
pub fn ensure_scratch_image(images_dir: &Path) -> Result<StoredImage> {
    let rootfs = images_dir.join("scratch").join("rootfs");
    fs::create_dir_all(&rootfs)
        .with_context(|| format!("create scratch rootfs '{}'", rootfs.display()))?;
    let abs = fs::canonicalize(&rootfs)?;
    Ok(StoredImage {
        id: "sha256:coral-scratch".to_string(),
        repo_tags: vec!["scratch".to_string(), "scratch:latest".to_string()],
        size: 0,
        rootfs_path: abs.to_string_lossy().into_owned(),
        managed: true,
    })
}

/// Recursively removes a directory even if some subdirectories inside are read-only
/// or owned by subordinate UIDs/GIDs in a Multi-UID rootless container.
pub fn remove_dir_force(path: &Path) -> Result<()> {
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    coral_runtime::remove_dir_all_rootless_safe(path)
        .with_context(|| format!("remove dir '{}'", path.display()))
}

/// Computes total unique size in bytes of regular files in a directory tree
/// (without following symlinks, and counting hardlinked inodes only once).
pub fn dir_size(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut seen_inodes: HashSet<(u64, u64)> = HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(meta) = fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(p);
            } else if meta.is_file() && seen_inodes.insert((meta.dev(), meta.ino())) {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn scratch_and_index_persistence_roundtrip() {
        let dir = tempdir().unwrap();
        let scratch = ensure_scratch_image(dir.path()).unwrap();
        assert_eq!(scratch.size, 0);
        assert!(Path::new(&scratch.rootfs_path).is_dir());

        save_index(dir.path(), std::slice::from_ref(&scratch)).unwrap();
        let loaded = load_index(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "sha256:coral-scratch");
    }

    #[test]
    fn dir_size_deduplicates_hardlinks() {
        let dir = tempdir().unwrap();
        let bin = dir.path().join("busybox");
        fs::write(&bin, vec![0u8; 4096]).unwrap();
        fs::hard_link(&bin, dir.path().join("sh")).unwrap();
        fs::hard_link(&bin, dir.path().join("ls")).unwrap();
        assert_eq!(dir_size(dir.path()), 4096);
    }
}
