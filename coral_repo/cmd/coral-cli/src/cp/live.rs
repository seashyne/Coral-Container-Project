//! Live-container copies driven through `coral exec --local` (streaming files and
//! directories through the container's own mount namespace).

use super::endpoint::ResolvedContainer;
use anyhow::{Context, Result, bail};
use coral_runtime::StateStore;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub(super) fn copy_host_to_live_container(
    runtime_root: &Path,
    cid: &str,
    host_src: &Path,
    ctr_rel: &Path,
) -> Result<()> {
    let coral_exe = std::env::current_exe().context("resolve current coral binary")?;
    let abs_ctr = format!("/{}", ctr_rel.to_string_lossy().trim_start_matches('/'));

    if host_src.is_file() {
        let data = fs::read(host_src)
            .with_context(|| format!("read host file '{}'", host_src.display()))?;
        let fname = host_src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let quoted_dst = sh_quote(&abs_ctr);
        let quoted_fname = sh_quote(&fname);
        let script = format!(
            "if [ -d {quoted_dst} ]; then target={quoted_dst}/{quoted_fname}; else target={quoted_dst}; fi; \
             mkdir -p \"$(dirname \"$target\")\" && cat > \"$target\""
        );
        let mut child = Command::new(&coral_exe)
            .arg("--root")
            .arg(runtime_root)
            .arg("exec")
            .arg("--local")
            .arg(cid)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawn coral exec for cp")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&data)?;
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(
                "live cp into '{cid}:{abs_ctr}' failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        return Ok(());
    }

    // Directory copy into live container: pack a tar stream in memory and extract inside container.
    let mut tar_buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_buf);
        builder.append_dir_all(".", host_src)?;
        builder.finish()?;
    }
    let quoted_dst = sh_quote(&abs_ctr);
    let script = format!("mkdir -p {quoted_dst} && tar -xf - -C {quoted_dst}");
    let mut child = Command::new(&coral_exe)
        .arg("--root")
        .arg(runtime_root)
        .arg("exec")
        .arg("--local")
        .arg(cid)
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn coral exec tar for cp")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&tar_buf)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "live directory cp into '{cid}:{abs_ctr}' failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Reads a path out of a live container (its actual mount namespace sees /tmp tmpfs,
/// volumes and the merged overlay view). Returns `false` when the path could not be
/// classified, so the caller can fall back to the static OverlayFS merge.
pub(super) fn copy_from_live_container(
    target: &ResolvedContainer,
    rt_root: &Path,
    ctr_rel: &Path,
    host_dst: &Path,
) -> Result<bool> {
    let coral_exe = std::env::current_exe().context("resolve current coral binary")?;
    let abs_ctr = format!("/{}", ctr_rel.to_string_lossy().trim_start_matches('/'));
    let quoted_src = sh_quote(&abs_ctr);
    let probe = Command::new(&coral_exe)
        .arg("--root")
        .arg(rt_root)
        .arg("exec")
        .arg("--local")
        .arg(&target.id)
        .arg("--")
        .arg("/bin/sh")
        .arg("-c")
        .arg(format!("if [ -d {quoted_src} ]; then echo DIR; elif [ -f {quoted_src} ]; then echo FILE; else exit 2; fi"))
        .output()?;
    if !probe.status.success() {
        return Ok(false);
    }
    let kind = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    if kind == "FILE" {
        let out = Command::new(&coral_exe)
            .arg("--root")
            .arg(rt_root)
            .arg("exec")
            .arg("--local")
            .arg(&target.id)
            .arg("--")
            .arg("/bin/cat")
            .arg(&abs_ctr)
            .output()?;
        if !out.status.success() {
            bail!(
                "read '{}:{abs_ctr}' failed: {}",
                target.id,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let final_dst = if host_dst.is_dir() {
            host_dst.join(
                ctr_rel
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("invalid source filename"))?,
            )
        } else {
            if let Some(p) = host_dst.parent()
                && !p.as_os_str().is_empty()
            {
                fs::create_dir_all(p)?;
            }
            host_dst.to_path_buf()
        };
        fs::write(&final_dst, &out.stdout)?;
        return Ok(true);
    } else if kind == "DIR" {
        let out = Command::new(&coral_exe)
            .arg("--root")
            .arg(rt_root)
            .arg("exec")
            .arg("--local")
            .arg(&target.id)
            .arg("--")
            .arg("/bin/tar")
            .arg("-C")
            .arg(&abs_ctr)
            .arg("-cf")
            .arg("-")
            .arg(".")
            .output()?;
        if !out.status.success() {
            bail!(
                "tar '{}:{abs_ctr}' failed: {}",
                target.id,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        fs::create_dir_all(host_dst)?;
        let mut archive = tar::Archive::new(out.stdout.as_slice());
        archive.unpack(host_dst)?;
        return Ok(true);
    }
    Ok(false)
}

/// True when the container is live and its runtime state is reachable for `setns` exec.
pub(super) fn live_runtime_root(target: &ResolvedContainer) -> Option<&Path> {
    if target.is_running
        && let Some(rt_root) = &target.runtime_root
        && StateStore::new(rt_root).load(&target.id).is_ok()
    {
        return Some(rt_root.as_path());
    }
    None
}

pub(super) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
