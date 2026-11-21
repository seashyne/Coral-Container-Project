//! Deterministic SHA-256 Merkle tree digest over an image rootfs.

use super::util::sha256_hex;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Computes a deterministic SHA-256 Merkle tree digest over every file, symlink, and
/// directory inside `rootfs` (including relative path, permissions, and file content).
pub fn compute_image_tree_digest(rootfs: &Path) -> Result<String> {
    if !rootfs.exists() {
        bail!("image rootfs '{}' does not exist", rootfs.display());
    }
    let mut entries = Vec::new();
    collect_paths(rootfs, Path::new(""), &mut entries)?;
    entries.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"coral-image-tree-v1\n");
    for rel in entries {
        let full = rootfs.join(&rel);
        let meta = fs::symlink_metadata(&full)
            .with_context(|| format!("stat '{}'", full.display()))?;
        let rel_str = rel.to_string_lossy();
        let mode = meta.mode() & 0o7777;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&full)?;
            hasher.update(format!("L:{rel_str}:{mode:o}:{}\n", target.to_string_lossy()).as_bytes());
        } else if meta.is_dir() {
            hasher.update(format!("D:{rel_str}:{mode:o}\n").as_bytes());
        } else if meta.is_file() {
            let data = fs::read(&full)?;
            let file_hash = sha256_hex(&data);
            hasher.update(format!("F:{rel_str}:{mode:o}:{}:{file_hash}\n", data.len()).as_bytes());
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn collect_paths(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let full = root.join(rel);
    for entry in fs::read_dir(&full)? {
        let entry = entry?;
        let child_rel = rel.join(entry.file_name());
        let meta = fs::symlink_metadata(entry.path())?;
        out.push(child_rel.clone());
        if meta.is_dir() && !meta.file_type().is_symlink() {
            collect_paths(root, &child_rel, out)?;
        }
    }
    Ok(())
}
