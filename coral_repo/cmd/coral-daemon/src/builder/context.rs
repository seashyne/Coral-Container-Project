//! Build-context path handling and base-image resolution.

use crate::images;
use anyhow::{Context, Result, anyhow, bail};
use coral_runtime::overlay::commit_overlay_to_rootfs;
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) async fn resolve_or_pull_base_image(base_ref: &str, images_dir: &Path) -> Result<PathBuf> {
    let index = images::load_index(images_dir);
    if let Some(found) = images::find_image(&index, base_ref) {
        let p = PathBuf::from(&found.rootfs_path);
        if p.is_dir() {
            return Ok(p);
        }
    }
    if Path::new(base_ref).is_dir() {
        return Ok(fs::canonicalize(base_ref)?);
    }
    // No progress reporter: this pull is an implicit `FROM` base-image fetch during a build,
    // driven by the builder rather than by `coral pull`, so there is no `coral.progress_id`.
    let pulled = images::pull_image(base_ref, images_dir, None).await?;
    let path = PathBuf::from(&pulled.rootfs_path);
    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, pulled);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    let _ = images::save_index(images_dir, &index);
    Ok(path)
}

pub(super) fn safe_join_context(ctx_dir: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = fs::canonicalize(ctx_dir)
        .with_context(|| format!("resolve build context '{}'", ctx_dir.display()))?;
    for comp in Path::new(rel).components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(p) => out.push(p),
            Component::ParentDir | Component::Prefix(_) => {
                bail!("unsafe path '{rel}' outside build context");
            }
        }
    }
    Ok(out)
}

pub(super) fn copy_context_item(src: &Path, dst: &Path, force_dir: bool) -> Result<()> {
    let meta = fs::symlink_metadata(src)
        .with_context(|| format!("stat copy source '{}'", src.display()))?;
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        let empty = Path::new("/nonexistent-upper");
        commit_overlay_to_rootfs(src, empty, dst)?;
        Ok(())
    } else if meta.file_type().is_symlink() && !src.exists() {
        let link_target = fs::read_link(src)?;
        let target_file = if force_dir || dst.is_dir() {
            fs::create_dir_all(dst)?;
            dst.join(
                src.file_name()
                    .ok_or_else(|| anyhow!("invalid source filename"))?,
            )
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            dst.to_path_buf()
        };
        let _ = fs::remove_file(&target_file);
        std::os::unix::fs::symlink(&link_target, &target_file)?;
        Ok(())
    } else {
        let target_file = if force_dir || dst.is_dir() {
            fs::create_dir_all(dst)?;
            dst.join(
                src.file_name()
                    .ok_or_else(|| anyhow!("invalid source filename"))?,
            )
        } else {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            dst.to_path_buf()
        };
        fs::copy(src, &target_file)?;
        Ok(())
    }
}
