//! Static (stopped container) copies: OverlayFS upperdir merged over the base rootfs.

use super::endpoint::ResolvedContainer;
use super::live::{copy_from_live_container, copy_host_to_live_container, live_runtime_root};
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::path::{Component, Path};

pub(super) fn copy_host_to_container(
    target: &ResolvedContainer,
    host_src: &Path,
    ctr_rel: &Path,
) -> Result<()> {
    // Live running container: stream through `coral exec --local` so Linux OverlayFS
    // never sees out-of-band writes to upperdir while mounted.
    if let Some(rt_root) = live_runtime_root(target) {
        return copy_host_to_live_container(rt_root, &target.id, host_src, ctr_rel);
    }

    let base = if target.upperdir.exists() || target.upperdir.parent().is_some_and(Path::exists) {
        fs::create_dir_all(&target.upperdir)?;
        &target.upperdir
    } else {
        &target.lower_rootfs
    };
    let dest = base.join(ctr_rel);
    copy_fs_entry(host_src, &dest)
}

pub(super) fn copy_container_to_host(
    target: &ResolvedContainer,
    ctr_rel: &Path,
    host_dst: &Path,
) -> Result<()> {
    // 1. Live container: read from inside its actual mount namespace (sees /tmp tmpfs, volumes, and overlay merged view).
    if let Some(rt_root) = live_runtime_root(target)
        && copy_from_live_container(target, rt_root, ctr_rel, host_dst)?
    {
        return Ok(());
    }

    // 2. Exited / static container: read merged view of OverlayFS upperdir over lower_rootfs.
    let upper_candidate = target.upperdir.join(ctr_rel);
    let lower_candidate = target.lower_rootfs.join(ctr_rel);

    if is_whiteout_in_upper(&target.upperdir, ctr_rel) {
        bail!(
            "path '/{}' was deleted inside container '{}'",
            ctr_rel.display(),
            target.id
        );
    }

    let upper_exists = upper_candidate.exists();
    let lower_exists = lower_candidate.exists();
    if !upper_exists && !lower_exists {
        bail!(
            "path '/{}' not found in container '{}'",
            ctr_rel.display(),
            target.id
        );
    }

    if (upper_exists && upper_candidate.is_dir()) || (!upper_exists && lower_candidate.is_dir()) {
        fs::create_dir_all(host_dst)?;
        if lower_exists && lower_candidate.is_dir() {
            copy_dir_recursive(&lower_candidate, host_dst)?;
        }
        if upper_exists && upper_candidate.is_dir() {
            copy_dir_recursive(&upper_candidate, host_dst)?;
        }
        return Ok(());
    }

    let src_file = if upper_exists {
        &upper_candidate
    } else {
        &lower_candidate
    };
    copy_fs_entry(src_file, host_dst)
}

fn is_whiteout_in_upper(upperdir: &Path, rel: &Path) -> bool {
    let mut cur = upperdir.to_path_buf();
    for comp in rel.components() {
        if let Component::Normal(seg) = comp {
            let wh = cur.join(format!(".wh.{}", seg.to_string_lossy()));
            if wh.exists() {
                return true;
            }
            cur.push(seg);
        }
    }
    false
}

fn copy_fs_entry(src: &Path, dst: &Path) -> Result<()> {
    if src.is_dir() {
        fs::create_dir_all(dst)?;
        return copy_dir_recursive(src, dst);
    }
    let final_dst = if dst.is_dir() {
        dst.join(
            src.file_name()
                .ok_or_else(|| anyhow!("missing source filename"))?,
        )
    } else {
        if let Some(p) = dst.parent()
            && !p.as_os_str().is_empty()
        {
            fs::create_dir_all(p)?;
        }
        dst.to_path_buf()
    };
    fs::copy(src, &final_dst)
        .with_context(|| format!("copy '{}' -> '{}'", src.display(), final_dst.display()))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with(".wh.") {
            continue;
        }
        let s = entry.path();
        let d = dst.join(&name);
        if s.is_dir() {
            copy_dir_recursive(&s, &d)?;
        } else if s.is_file() {
            fs::copy(&s, &d)?;
        }
    }
    Ok(())
}
