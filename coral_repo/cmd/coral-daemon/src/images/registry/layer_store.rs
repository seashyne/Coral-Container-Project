//! Shared Content-Addressable Extracted Layer Store (`<images_dir>/.layers/sha256/<hex>/`),
//! zero-copy hardlink rootfs materialization, and multi-lowerdir OverlayFS manifest (`layers.json`).

use crate::images::extract::extract_layer;
use crate::images::remove_dir_force;
use anyhow::{Context, Result};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use tracing::debug;

const LAYERS_MANIFEST_FILE: &str = "layers.json";
const LAYER_COMPLETE_MARKER: &str = ".coral-layer-complete";

/// Resolves `<images_dir>/.layers/sha256/<hex>` for a valid `sha256:<hex>` digest.
pub fn extracted_layer_dir(images_dir: &Path, digest: &str) -> Option<PathBuf> {
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(images_dir.join(".layers").join("sha256").join(hex))
}

/// Extracts a verified layer blob ONCE into `<images_dir>/.layers/sha256/<hex>/` (or reuses
/// the existing directory when already extracted), returning the read-only layer directory path.
pub fn extract_or_reuse_layer(
    images_dir: &Path,
    blob_path: &Path,
    digest: &str,
    media_type: &str,
) -> Result<Option<PathBuf>> {
    let Some(final_dir) = extracted_layer_dir(images_dir, digest) else {
        return Ok(None);
    };
    let marker = final_dir.join(LAYER_COMPLETE_MARKER);
    if final_dir.is_dir() && marker.is_file() {
        debug!(digest = %digest, dir = %final_dir.display(), "reusing shared extracted layer");
        return Ok(Some(final_dir));
    }

    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create layer store dir '{}'", parent.display()))?;
    }
    let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let tmp_root = images_dir.join(".layers").join(".tmp");
    fs::create_dir_all(&tmp_root)?;
    let tmp_dir = tmp_root.join(format!("layer-{}-{ts}", std::process::id()));
    fs::create_dir_all(&tmp_dir)?;

    if let Err(err) = extract_layer(blob_path, &tmp_dir, media_type) {
        let _ = remove_dir_force(&tmp_dir);
        return Err(err);
    }
    fs::write(tmp_dir.join(LAYER_COMPLETE_MARKER), b"ok\n")?;

    if final_dir.exists() {
        if marker.is_file() {
            let _ = remove_dir_force(&tmp_dir);
            return Ok(Some(final_dir));
        }
        let _ = remove_dir_force(&final_dir);
    }
    if fs::rename(&tmp_dir, &final_dir).is_err() {
        // Another concurrent pull may have installed the identical layer first.
        let _ = remove_dir_force(&tmp_dir);
        if final_dir.is_dir() && marker.is_file() {
            return Ok(Some(final_dir));
        }
        anyhow::bail!("install shared extracted layer '{}'", final_dir.display());
    }
    Ok(Some(final_dir))
}

/// Materializes `dst_rootfs` from ordered base-to-top extracted layer directories using zero-copy
/// `fs::hard_link` (same inode = 0 duplicate data blocks on disk), while honoring OCI whiteouts.
pub fn materialize_hardlink_rootfs(base_to_top: &[PathBuf], dst_rootfs: &Path) -> Result<()> {
    fs::create_dir_all(dst_rootfs)?;
    for layer_dir in base_to_top {
        apply_layer_hardlinks(layer_dir, layer_dir, dst_rootfs)?;
    }
    Ok(())
}

fn apply_layer_hardlinks(layer_root: &Path, cur: &Path, dst_rootfs: &Path) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(cur)?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());

    // Pass 1: handle opaque whiteout `.wh..wh..opq` before copying files into this dir.
    let rel_dir = cur.strip_prefix(layer_root).unwrap_or(Path::new(""));
    let dst_dir = if rel_dir.as_os_str().is_empty() {
        dst_rootfs.to_path_buf()
    } else {
        dst_rootfs.join(rel_dir)
    };
    if entries.iter().any(|e| e.file_name() == ".wh..wh..opq") && dst_dir.is_dir() {
        for old in fs::read_dir(&dst_dir)?.flatten() {
            let p = old.path();
            if p.is_dir() && !fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                let _ = remove_dir_force(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }

    for entry in entries {
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();
        if name == LAYER_COMPLETE_MARKER || name == ".wh..wh..opq" {
            continue;
        }
        if let Some(target_name) = name.strip_prefix(".wh.") {
            let gone = dst_dir.join(target_name);
            if gone.exists() || fs::symlink_metadata(&gone).is_ok() {
                if gone.is_dir() && !fs::symlink_metadata(&gone).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                    let _ = remove_dir_force(&gone);
                } else {
                    let _ = fs::remove_file(&gone);
                }
            }
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst_dir.join(&name_os);
        let meta = fs::symlink_metadata(&src_path)?;
        let ft = meta.file_type();
        if ft.is_symlink() {
            if dst_path.is_dir() && !fs::symlink_metadata(&dst_path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                let _ = remove_dir_force(&dst_path);
            } else {
                let _ = fs::remove_file(&dst_path);
            }
            let target = fs::read_link(&src_path)?;
            if let Some(parent) = dst_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let _ = symlink(&target, &dst_path);
        } else if ft.is_dir() {
            if let Ok(dm) = fs::symlink_metadata(&dst_path)
                && !dm.is_dir()
            {
                let _ = fs::remove_file(&dst_path);
            }
            fs::create_dir_all(&dst_path)?;
            let _ = fs::set_permissions(&dst_path, meta.permissions());
            apply_layer_hardlinks(layer_root, &src_path, dst_rootfs)?;
        } else if ft.is_file() {
            if let Some(parent) = dst_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if let Ok(dm) = fs::symlink_metadata(&dst_path) {
                if dm.is_dir() {
                    let _ = remove_dir_force(&dst_path);
                } else {
                    let _ = fs::remove_file(&dst_path);
                }
            }
            if fs::hard_link(&src_path, &dst_path).is_err() {
                fs::copy(&src_path, &dst_path)?;
                let _ = fs::set_permissions(&dst_path, meta.permissions());
            }
        }
    }
    Ok(())
}

/// Returns `true` when none of the extracted layer directories contain OCI `.wh.` whiteout
/// marker files, meaning the stack can be mounted directly as kernel `lowerdir=top:...:base`.
pub fn layers_safe_for_direct_overlay(layers: &[PathBuf]) -> bool {
    if layers.len() <= 1 {
        return !layers.is_empty();
    }
    fn has_whiteout(dir: &Path) -> bool {
        let Ok(rd) = fs::read_dir(dir) else {
            return false;
        };
        for entry in rd.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(".wh.") {
                return true;
            }
            let p = entry.path();
            if let Ok(m) = fs::symlink_metadata(&p)
                && m.is_dir()
                && has_whiteout(&p)
            {
                return true;
            }
        }
        false
    }
    !layers[1..].iter().any(|l| has_whiteout(l))
}

/// Saves the OverlayFS `lowerdir` list (`top..base` order) to `<rootfs>/../layers.json`.
pub fn save_image_layers_manifest(rootfs_path: &Path, top_to_base: &[PathBuf]) -> Result<()> {
    let Some(parent) = rootfs_path.parent() else {
        return Ok(());
    };
    if top_to_base.is_empty() {
        return Ok(());
    }
    let paths: Vec<String> = top_to_base
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let json = serde_json::to_string_pretty(&paths)?;
    fs::write(parent.join(LAYERS_MANIFEST_FILE), format!("{json}\n"))?;
    Ok(())
}

/// Loads valid OverlayFS `lowerdir` directories (`top..base`) from `<rootfs>/../layers.json`.
pub fn load_image_layers_manifest(rootfs_path: &Path) -> Option<Vec<PathBuf>> {
    let manifest = rootfs_path.parent()?.join(LAYERS_MANIFEST_FILE);
    let raw = fs::read_to_string(&manifest).ok()?;
    let list: Vec<String> = serde_json::from_str(&raw).ok()?;
    if list.is_empty() {
        return None;
    }
    let paths: Vec<PathBuf> = list.into_iter().map(PathBuf::from).collect();
    if paths.iter().all(|p| p.is_dir()) {
        Some(paths)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    use tempfile::tempdir;

    fn make_gz_layer(files: &[(&str, &[u8])]) -> (Vec<u8>, String) {
        let mut tar_buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_buf);
            for (path, content) in files {
                let mut hdr = tar::Header::new_gnu();
                hdr.set_size(content.len() as u64);
                hdr.set_mode(0o644);
                hdr.set_cksum();
                builder.append_data(&mut hdr, *path, *content).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, &tar_buf).unwrap();
        let gz = enc.finish().unwrap();
        let mut h = Sha256::new();
        h.update(&gz);
        (gz, format!("sha256:{:x}", h.finalize()))
    }

    #[test]
    fn shared_layer_store_deduplicates_inodes_and_persists_overlay_lowerdirs() {
        let dir = tempdir().unwrap();
        let (base_gz, base_digest) = make_gz_layer(&[("etc/base.txt", b"shared-base")]);
        let (top_gz, top_digest) = make_gz_layer(&[("app/main.txt", b"app-top")]);

        let b1 = dir.path().join("base.tar.gz");
        let b2 = dir.path().join("top.tar.gz");
        fs::write(&b1, &base_gz).unwrap();
        fs::write(&b2, &top_gz).unwrap();

        let l1 = extract_or_reuse_layer(dir.path(), &b1, &base_digest, "application/gzip")
            .unwrap()
            .unwrap();
        let l2 = extract_or_reuse_layer(dir.path(), &b2, &top_digest, "application/gzip")
            .unwrap()
            .unwrap();
        // Second call reuses existing layer dir directly.
        let l1_again = extract_or_reuse_layer(dir.path(), &b1, &base_digest, "application/gzip")
            .unwrap()
            .unwrap();
        assert_eq!(l1, l1_again);

        let r1 = dir.path().join("img1/rootfs");
        let r2 = dir.path().join("img2/rootfs");
        materialize_hardlink_rootfs(&[l1.clone(), l2.clone()], &r1).unwrap();
        materialize_hardlink_rootfs(std::slice::from_ref(&l1), &r2).unwrap();

        let ino1 = fs::metadata(r1.join("etc/base.txt")).unwrap().ino();
        let ino2 = fs::metadata(r2.join("etc/base.txt")).unwrap().ino();
        let ino_shared = fs::metadata(l1.join("etc/base.txt")).unwrap().ino();
        assert_eq!(ino1, ino_shared);
        assert_eq!(ino2, ino_shared);

        save_image_layers_manifest(&r1, &[l2.clone(), l1.clone()]).unwrap();
        assert_eq!(load_image_layers_manifest(&r1).unwrap(), vec![l2, l1]);
    }
}
