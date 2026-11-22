//! Rootless-safe OCI tar / tar+gzip layer extraction with merged-`/usr` symlink
//! traversal containment and OCI whiteout (`.wh.*` / `.wh..wh..opq`) handling.

#[cfg(test)]
mod tests;

use super::remove_dir_force;
use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use tar::{Archive, EntryType};
use tracing::debug;

const MAX_SYMLINK_HOPS: usize = 40;

/// Safely extracts an OCI tar (plain, gzip, or zstd) layer into `rootfs` in rootless-compatible mode.
pub fn extract_layer(blob_path: &Path, rootfs: &Path, media_type: &str) -> Result<()> {
    let mut file = File::open(blob_path)
        .with_context(|| format!("open layer blob '{}'", blob_path.display()))?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;

    let is_gzip = media_type.contains("gzip") || (n >= 2 && magic[..2] == [0x1f, 0x8b]);
    let is_zstd =
        media_type.contains("zstd") || (n == 4 && magic == [0x28, 0xb5, 0x2f, 0xfd]);

    if is_gzip {
        let decoder = GzDecoder::new(file);
        extract_tar_stream(decoder, rootfs)
    } else if is_zstd {
        drop(file);
        let mut child = std::process::Command::new("zstd")
            .args(["-d", "-c"])
            .arg(blob_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn 'zstd -d -c' to decompress OCI zstd layer")?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("failed to capture zstd stdout"))?;
        let extract_res = extract_tar_stream(&mut stdout, rootfs);
        // The tar reader stops at the end-of-archive marker, so the
        // decompressor may still have output pending. Drain the pipe before
        // waiting, otherwise it is closed under its feet and it dies with
        // SIGPIPE — which surfaced as an intermittent
        // "zstd decompression failed" on valid archives.
        let _ = io::copy(&mut stdout, &mut io::sink());
        let out = child.wait_with_output().context("wait for zstd process")?;
        extract_res?;
        if !out.status.success() {
            bail!(
                "zstd decompression failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    } else {
        extract_tar_stream(file, rootfs)
    }
}

fn extract_tar_stream<R: Read>(reader: R, rootfs: &Path) -> Result<()> {
    let mut archive = Archive::new(reader);
    let entries = archive.entries().context("read tar entries")?;

    for entry_res in entries {
        let mut entry = entry_res.context("read tar entry")?;
        let raw_path = entry.path().context("read tar entry path")?.into_owned();
        let comps = normalize_tar_components(&raw_path)?;
        if comps.is_empty() {
            continue;
        }

        let (parent_comps, leaf) = comps.split_at(comps.len() - 1);
        let leaf_name = &leaf[0];
        let leaf_str = leaf_name.to_string_lossy();

        // Handle OCI whiteouts before writing files.
        if leaf_str == ".wh..wh..opq" {
            let parent_dir = resolve_dir_in_rootfs(rootfs, parent_comps, false)?;
            if parent_dir.is_dir() {
                clear_dir_contents(&parent_dir)?;
            }
            continue;
        }
        if let Some(removed_name) = leaf_str.strip_prefix(".wh.") {
            let parent_dir = resolve_dir_in_rootfs(rootfs, parent_comps, false)?;
            let target = parent_dir.join(removed_name);
            remove_path_if_exists(&target)?;
            continue;
        }

        let entry_type = entry.header().entry_type();
        let mode = entry.header().mode().unwrap_or(0o644);
        let parent_dir = resolve_dir_in_rootfs(rootfs, parent_comps, true)?;
        let target = parent_dir.join(leaf_name);

        match entry_type {
            EntryType::Directory => {
                if let Ok(meta) = fs::symlink_metadata(&target)
                    && !meta.is_dir()
                {
                    remove_path_if_exists(&target)?;
                }
                fs::create_dir_all(&target)
                    .with_context(|| format!("create dir '{}'", target.display()))?;
                let safe_mode = (mode & 0o777) | 0o700;
                let _ = fs::set_permissions(&target, fs::Permissions::from_mode(safe_mode));
            }
            EntryType::Regular | EntryType::Continuous => {
                remove_path_if_exists(&target)?;
                let mut out = File::create(&target)
                    .with_context(|| format!("create file '{}'", target.display()))?;
                io::copy(&mut entry, &mut out)
                    .with_context(|| format!("write file '{}'", target.display()))?;
                let safe_mode = (mode & 0o777) | 0o600;
                let _ = fs::set_permissions(&target, fs::Permissions::from_mode(safe_mode));
            }
            EntryType::Symlink => {
                let link_target = entry
                    .link_name()
                    .context("read symlink target")?
                    .ok_or_else(|| anyhow!("symlink '{}' missing link target", raw_path.display()))?
                    .into_owned();
                remove_path_if_exists(&target)?;
                symlink(&link_target, &target).with_context(|| {
                    format!(
                        "create symlink '{}' -> '{}'",
                        target.display(),
                        link_target.display()
                    )
                })?;
            }
            EntryType::Link => {
                let link_raw = entry
                    .link_name()
                    .context("read hardlink target")?
                    .ok_or_else(|| anyhow!("hardlink '{}' missing link target", raw_path.display()))?
                    .into_owned();
                let link_comps = normalize_tar_components(&link_raw)?;
                if link_comps.is_empty() {
                    continue;
                }
                let (lp, ll) = link_comps.split_at(link_comps.len() - 1);
                let resolved_lp = resolve_dir_in_rootfs(rootfs, lp, true)?;
                let link_src = resolved_lp.join(&ll[0]);
                remove_path_if_exists(&target)?;
                if fs::hard_link(&link_src, &target).is_err() && link_src.exists() {
                    let _ = fs::copy(&link_src, &target);
                }
            }
            EntryType::Char | EntryType::Block | EntryType::Fifo => {
                debug!(path = %raw_path.display(), "skipping special device/fifo entry");
            }
            _ => {}
        }
    }

    Ok(())
}

fn normalize_tar_components(path: &Path) -> Result<Vec<OsString>> {
    let mut comps = Vec::new();
    for c in path.components() {
        match c {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => comps.push(part.to_os_string()),
            Component::ParentDir | Component::Prefix(_) => {
                bail!("unsafe tar entry path '{}'", path.display());
            }
        }
    }
    Ok(comps)
}

/// Resolves a sequence of directory components strictly inside `rootfs`, following any
/// intermediate symlinks (relative OR absolute) without ever escaping `rootfs`.
fn resolve_dir_in_rootfs(
    rootfs: &Path,
    components: &[OsString],
    create_missing: bool,
) -> Result<PathBuf> {
    let mut resolved: Vec<OsString> = Vec::new();
    let mut pending: Vec<OsString> = components.iter().rev().cloned().collect();
    let mut hops = 0usize;

    while let Some(part) = pending.pop() {
        let mut candidate = rootfs.to_path_buf();
        for r in &resolved {
            candidate.push(r);
        }
        candidate.push(&part);

        match fs::symlink_metadata(&candidate) {
            Ok(meta) if meta.file_type().is_symlink() => {
                hops += 1;
                if hops > MAX_SYMLINK_HOPS {
                    bail!("too many symlink levels inside rootfs while resolving '{part:?}'");
                }
                let link_target = fs::read_link(&candidate)
                    .with_context(|| format!("read symlink '{}'", candidate.display()))?;
                if link_target.is_absolute() {
                    resolved.clear();
                }
                let mut target_parts = Vec::new();
                for c in link_target.components() {
                    match c {
                        Component::RootDir | Component::CurDir => {}
                        Component::Normal(p) => target_parts.push(p.to_os_string()),
                        Component::ParentDir => {
                            if target_parts.pop().is_none() {
                                resolved.pop();
                            }
                        }
                        Component::Prefix(_) => {}
                    }
                }
                for p in target_parts.into_iter().rev() {
                    pending.push(p);
                }
            }
            Ok(meta) if meta.is_dir() => {
                let mode = meta.permissions().mode();
                if mode & 0o700 != 0o700 {
                    let _ =
                        fs::set_permissions(&candidate, fs::Permissions::from_mode(mode | 0o700));
                }
                resolved.push(part);
            }
            Ok(_) => {
                if create_missing {
                    let _ = fs::remove_file(&candidate);
                    fs::create_dir(&candidate)
                        .with_context(|| format!("create directory '{}'", candidate.display()))?;
                }
                resolved.push(part);
            }
            Err(_) => {
                if create_missing {
                    fs::create_dir_all(&candidate)
                        .with_context(|| format!("create directory '{}'", candidate.display()))?;
                }
                resolved.push(part);
            }
        }
    }

    let mut out = rootfs.to_path_buf();
    for r in resolved {
        out.push(r);
    }
    Ok(out)
}

fn clear_dir_contents(dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        remove_path_if_exists(&entry.path())?;
    }
    Ok(())
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.is_dir() && !meta.file_type().is_symlink() {
                remove_dir_force(path)?;
            } else {
                fs::remove_file(path)
                    .with_context(|| format!("remove '{}'", path.display()))?;
            }
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
