//! OverlayFS Copy-on-Write management, upper-layer diff inspection (`coral diff`),
//! and CoW layer commit (`coral commit`).

use nix::mount::{MsFlags, mount};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use crate::RuntimeError;

/// Prepared OverlayFS directory set inside a container bundle (`upper/`, `work/`, `merged/`).
/// Supports both single-layer and multi-lowerdir (`lowerdir=top:...:base`) OverlayFS stacks.
#[derive(Debug, Clone)]
pub struct OverlayDirs {
    pub lowerdirs: Vec<PathBuf>,
    pub upperdir: PathBuf,
    pub workdir: PathBuf,
    pub merged: PathBuf,
}

impl OverlayDirs {
    /// Creates `overlay/{upper,work,merged}` under `bundle_dir` for a single lowerdir.
    pub fn prepare(bundle_dir: &Path, lowerdir: PathBuf) -> Result<Self, RuntimeError> {
        Self::prepare_multi(bundle_dir, vec![lowerdir])
    }

    /// Creates `overlay/{upper,work,merged}` under `bundle_dir` for a multi-layer stack
    /// (`lowerdirs[0]` is the topmost read-only layer, `lowerdirs[n-1]` is the base layer).
    pub fn prepare_multi(bundle_dir: &Path, lowerdirs: Vec<PathBuf>) -> Result<Self, RuntimeError> {
        if lowerdirs.is_empty() {
            return Err(RuntimeError::InitFailed(
                "OverlayFS requires at least one lowerdir".into(),
            ));
        }
        let root = bundle_dir.join("overlay");
        let upperdir = root.join("upper");
        let workdir = root.join("work");
        let merged = root.join("merged");
        fs::create_dir_all(&upperdir)
            .map_err(|e| RuntimeError::sys("create overlay upperdir", e))?;
        fs::create_dir_all(&workdir).map_err(|e| RuntimeError::sys("create overlay workdir", e))?;
        fs::create_dir_all(&merged).map_err(|e| RuntimeError::sys("create overlay merged", e))?;
        Ok(Self {
            lowerdirs,
            upperdir,
            workdir,
            merged,
        })
    }
}

/// Mounts an OverlayFS filesystem onto `target` using a single `lowerdir`.
pub fn mount_overlay(
    lowerdir: &Path,
    upperdir: &Path,
    workdir: &Path,
    target: &Path,
    rootless: bool,
) -> Result<(), RuntimeError> {
    mount_overlay_multi(&[lowerdir.to_path_buf()], upperdir, workdir, target, rootless)
}

/// Mounts a single- or multi-lowerdir OverlayFS filesystem onto `target`.
/// When `rootless == true`, appends `userxattr` so Linux 5.11+ permits unprivileged
/// OverlayFS inside a User + Mount namespace without requiring root or FUSE.
pub fn mount_overlay_multi(
    lowerdirs: &[PathBuf],
    upperdir: &Path,
    workdir: &Path,
    target: &Path,
    rootless: bool,
) -> Result<(), RuntimeError> {
    if lowerdirs.is_empty() {
        return Err(RuntimeError::InitFailed(
            "OverlayFS requires at least one lowerdir".into(),
        ));
    }
    fs::create_dir_all(upperdir).map_err(|e| RuntimeError::sys("create overlay upperdir", e))?;
    fs::create_dir_all(workdir).map_err(|e| RuntimeError::sys("create overlay workdir", e))?;
    fs::create_dir_all(target).map_err(|e| RuntimeError::sys("create overlay target", e))?;

    let lower_joined = lowerdirs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");

    let mut data = format!(
        "lowerdir={},upperdir={},workdir={}",
        lower_joined,
        upperdir.display(),
        workdir.display()
    );
    if rootless {
        data.push_str(",userxattr");
    }

    mount(
        Some("overlay"),
        target,
        Some("overlay"),
        MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
        Some(data.as_str()),
    )
    .map_err(|e| RuntimeError::nix("mount overlayfs", e))
}

/// A filesystem change recorded in a container's Copy-on-Write OverlayFS `upperdir`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OverlayChange {
    /// `"A"` (Added), `"C"` (Changed/Modified), or `"D"` (Deleted whiteout).
    pub kind: String,
    /// Absolute path inside the container (e.g. `"/etc/nginx.conf"`).
    pub path: String,
    /// File size in bytes (`0` for directories and deletions).
    pub size: u64,
}

impl std::fmt::Display for OverlayChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.kind, self.path)
    }
}

/// Inspects a container's OverlayFS `upperdir` against its base `lowerdir`, returning all
/// Added (`A`), Changed (`C`), and Deleted (`D`) paths sorted lexicographically.
pub fn diff_overlay(upperdir: &Path, lowerdir: Option<&Path>) -> Result<Vec<OverlayChange>, RuntimeError> {
    if !upperdir.is_dir() {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    collect_diff_recursive(upperdir, upperdir, lowerdir, &mut changes)?;
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

fn collect_diff_recursive(
    upper_root: &Path,
    cur: &Path,
    lower_root: Option<&Path>,
    out: &mut Vec<OverlayChange>,
) -> Result<(), RuntimeError> {
    let entries = fs::read_dir(cur).map_err(|e| RuntimeError::sys("read overlay upperdir", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| RuntimeError::sys("read overlay upperdir entry", e))?;
        let full = entry.path();
        let Ok(rel) = full.strip_prefix(upper_root) else {
            continue;
        };
        let container_path = format!("/{}", rel.to_string_lossy());
        let meta = fs::symlink_metadata(&full)
            .map_err(|e| RuntimeError::sys("stat overlay upperdir entry", e))?;
        let ft = meta.file_type();

        // Kernel OverlayFS represents deleted files as character devices with major/minor 0:0 (rdev == 0).
        if (ft.is_char_device() && meta.rdev() == 0)
            || rel
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".wh."))
        {
            let clean_path = if let Some(fname) = rel.file_name().and_then(|n| n.to_str())
                && let Some(stripped) = fname.strip_prefix(".wh.")
            {
                let parent = rel.parent().unwrap_or_else(|| Path::new(""));
                if parent.as_os_str().is_empty() {
                    format!("/{stripped}")
                } else {
                    format!("/{}/{stripped}", parent.to_string_lossy())
                }
            } else {
                container_path
            };
            out.push(OverlayChange {
                kind: "D".into(),
                path: clean_path,
                size: 0,
            });
            continue;
        }

        let existed_in_lower = lower_root.is_some_and(|l| fs::symlink_metadata(l.join(rel)).is_ok());
        let kind = if existed_in_lower { "C" } else { "A" };
        let size = if ft.is_file() { meta.len() } else { 0 };

        // Record files, symlinks, and newly added or leaf directories.
        if !ft.is_dir() || !existed_in_lower {
            out.push(OverlayChange {
                kind: kind.into(),
                path: container_path,
                size,
            });
        }

        if ft.is_dir() && !ft.is_symlink() {
            collect_diff_recursive(upper_root, &full, lower_root, out)?;
        }
    }
    Ok(())
}

/// Merges `lowerdir` and `upperdir` into a standalone rootfs directory at `dest_rootfs`
/// (used by `coral commit` to snapshot a container's OverlayFS CoW state into a new image).
pub fn commit_overlay_to_rootfs(
    lowerdir: &Path,
    upperdir: &Path,
    dest_rootfs: &Path,
) -> Result<(), RuntimeError> {
    fs::create_dir_all(dest_rootfs)
        .map_err(|e| RuntimeError::sys("create commit target rootfs", e))?;
    let mut inodes: HashMap<(u64, u64), PathBuf> = HashMap::new();
    copy_tree_layer(lowerdir, lowerdir, dest_rootfs, false, &mut inodes)?;
    if upperdir.is_dir() {
        let mut upper_inodes: HashMap<(u64, u64), PathBuf> = HashMap::new();
        copy_tree_layer(upperdir, upperdir, dest_rootfs, true, &mut upper_inodes)?;
    }
    Ok(())
}

fn copy_tree_layer(
    src_root: &Path,
    cur_src: &Path,
    dest_root: &Path,
    is_upper: bool,
    seen_hardlinks: &mut HashMap<(u64, u64), PathBuf>,
) -> Result<(), RuntimeError> {
    let entries = fs::read_dir(cur_src).map_err(|e| RuntimeError::sys("read layer dir", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| RuntimeError::sys("read layer entry", e))?;
        let src_path = entry.path();
        let Ok(rel) = src_path.strip_prefix(src_root) else {
            continue;
        };
        let dst_path = dest_root.join(rel);
        let meta = fs::symlink_metadata(&src_path)
            .map_err(|e| RuntimeError::sys("stat layer entry", e))?;
        let ft = meta.file_type();

        // Handle OverlayFS whiteout deletion in upperdir.
        if is_upper && ft.is_char_device() && meta.rdev() == 0 {
            if let Ok(dmeta) = fs::symlink_metadata(&dst_path) {
                if dmeta.is_dir() && !dmeta.file_type().is_symlink() {
                    let _ = crate::multi_uid::remove_dir_all_rootless_safe(&dst_path);
                } else {
                    let _ = fs::remove_file(&dst_path);
                }
            }
            continue;
        }

        if ft.is_symlink() {
            let target = fs::read_link(&src_path)
                .map_err(|e| RuntimeError::sys("read symlink in layer", e))?;
            remove_existing_entry(&dst_path);
            symlink(&target, &dst_path)
                .map_err(|e| RuntimeError::sys("create symlink in commit rootfs", e))?;
        } else if ft.is_dir() {
            if let Ok(dmeta) = fs::symlink_metadata(&dst_path)
                && !dmeta.is_dir()
            {
                remove_existing_entry(&dst_path);
            }
            fs::create_dir_all(&dst_path)
                .map_err(|e| RuntimeError::sys("create dir in commit rootfs", e))?;
            let mode = (meta.permissions().mode() & 0o777) | 0o700;
            let _ = fs::set_permissions(&dst_path, fs::Permissions::from_mode(mode));
            copy_tree_layer(src_root, &src_path, dest_root, is_upper, seen_hardlinks)?;
        } else if ft.is_file() {
            remove_existing_entry(&dst_path);
            let key = (meta.dev(), meta.ino());
            if meta.nlink() > 1
                && let Some(existing) = seen_hardlinks.get(&key)
                && fs::hard_link(existing, &dst_path).is_ok()
            {
                continue;
            }
            fs::copy(&src_path, &dst_path)
                .map_err(|e| RuntimeError::sys("copy file in commit rootfs", e))?;
            let mode = (meta.permissions().mode() & 0o777) | 0o600;
            let _ = fs::set_permissions(&dst_path, fs::Permissions::from_mode(mode));
            if meta.nlink() > 1 {
                seen_hardlinks.insert(key, dst_path);
            }
        }
    }
    Ok(())
}

fn remove_existing_entry(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.is_dir() && !meta.file_type().is_symlink() {
            let _ = crate::multi_uid::remove_dir_all_rootless_safe(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn diff_and_commit_overlay_tracks_added_modified_and_deleted_files() {
        let tmp = tempdir().unwrap();
        let lower = tmp.path().join("lower");
        let upper = tmp.path().join("upper");
        let committed = tmp.path().join("committed");
        fs::create_dir_all(lower.join("etc")).unwrap();
        fs::create_dir_all(upper.join("etc")).unwrap();

        fs::write(lower.join("etc/keep.conf"), "keep").unwrap();
        fs::write(lower.join("etc/mod.conf"), "v1").unwrap();
        fs::write(upper.join("etc/mod.conf"), "v2").unwrap();
        fs::write(upper.join("added.txt"), "hello").unwrap();

        let diffs = diff_overlay(&upper, Some(&lower)).unwrap();
        assert!(diffs.iter().any(|d| d.kind == "A" && d.path == "/added.txt"));
        assert!(diffs.iter().any(|d| d.kind == "C" && d.path == "/etc/mod.conf"));

        commit_overlay_to_rootfs(&lower, &upper, &committed).unwrap();
        assert_eq!(
            fs::read_to_string(committed.join("etc/keep.conf")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(committed.join("etc/mod.conf")).unwrap(),
            "v2"
        );
        assert_eq!(
            fs::read_to_string(committed.join("added.txt")).unwrap(),
            "hello"
        );
    }
}
