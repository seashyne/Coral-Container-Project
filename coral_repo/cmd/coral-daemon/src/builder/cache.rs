//! Build-step cache keys, source hashing, and cached-rootfs save/restore.

use super::context::safe_join_context;
use super::dockerignore::DockerIgnore;
use crate::images;
use anyhow::{Result, bail};
use coral_runtime::overlay::commit_overlay_to_rootfs;
use coral_runtime::rootfs::safe_join_rootfs;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub(super) fn compute_step_key(
    parent_key: &str,
    verb: &str,
    arg: &str,
    workdir: &str,
    envs: &[String],
    extra: &[u8],
) -> String {
    let mut h = Sha256::new();
    h.update(parent_key.as_bytes());
    h.update(b"|");
    h.update(verb.as_bytes());
    h.update(b"|");
    h.update(arg.as_bytes());
    h.update(b"|");
    h.update(workdir.as_bytes());
    for e in envs {
        h.update(b"|");
        h.update(e.as_bytes());
    }
    if !extra.is_empty() {
        h.update(b"|");
        h.update(extra);
    }
    format!("{:x}", h.finalize())
}

pub(super) fn hash_sources(
    src_root: &Path,
    sources: &[String],
    is_stage_src: bool,
    extra_tag: &[u8],
) -> Result<Vec<u8>> {
    let mut h = Sha256::new();
    if !extra_tag.is_empty() {
        h.update(extra_tag);
        h.update(b"|");
    }
    let ignore = if !is_stage_src {
        DockerIgnore::load(src_root)
    } else {
        DockerIgnore::default()
    };
    for src_rel in sources {
        let src_path = if is_stage_src {
            safe_join_rootfs(src_root, src_rel)?
        } else {
            safe_join_context(src_root, src_rel)?
        };
        if fs::symlink_metadata(&src_path).is_err() {
            bail!(
                "source '{src_rel}' not found in {}",
                if is_stage_src { "stage rootfs" } else { "build context" }
            );
        }
        hash_path_recursive(src_root, &src_path, &ignore, &mut h)?;
    }
    Ok(h.finalize().to_vec())
}

pub(super) fn hash_path_recursive(
    ctx_root: &Path,
    cur: &Path,
    ignore: &DockerIgnore,
    h: &mut Sha256,
) -> Result<()> {
    let rel = cur.strip_prefix(ctx_root).unwrap_or(cur);
    if !rel.as_os_str().is_empty() && ignore.is_ignored(rel) {
        return Ok(());
    }
    h.update(rel.to_string_lossy().as_bytes());
    let meta = fs::symlink_metadata(cur)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(cur)?;
        h.update(target.to_string_lossy().as_bytes());
    } else if meta.is_file() {
        let bytes = fs::read(cur)?;
        h.update(&bytes);
    } else if meta.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(cur)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            hash_path_recursive(ctx_root, &entry.path(), ignore, h)?;
        }
    }
    Ok(())
}

pub(super) fn save_cached_rootfs(staging_rootfs: &Path, staging_dir: &Path, cache_dir: &Path) -> Result<()> {
    if cache_dir.exists() {
        let _ = images::remove_dir_force(cache_dir);
    }
    fs::create_dir_all(cache_dir)?;
    let empty_upper = staging_dir.join("empty-cache-upper");
    fs::create_dir_all(&empty_upper)?;
    let res = commit_overlay_to_rootfs(staging_rootfs, &empty_upper, cache_dir);
    let _ = fs::remove_dir_all(&empty_upper);
    res.map_err(Into::into)
}

pub(super) fn restore_cached_rootfs(
    cache_dir: &Path,
    staging_dir: &Path,
    staging_rootfs: &Path,
) -> Result<()> {
    if staging_rootfs.exists() {
        let _ = images::remove_dir_force(staging_rootfs);
    }
    fs::create_dir_all(staging_rootfs)?;
    let empty_upper = staging_dir.join("empty-cache-upper");
    fs::create_dir_all(&empty_upper)?;
    let res = commit_overlay_to_rootfs(cache_dir, &empty_upper, staging_rootfs);
    let _ = fs::remove_dir_all(&empty_upper);
    res.map_err(Into::into)
}
