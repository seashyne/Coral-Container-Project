//! Bidirectional file/directory copy between host and container (`coral cp <SRC> <DST>`),
//! supporting both live running containers (via `setns` exec stream) and stopped containers
//! (OverlayFS CoW upperdir + base rootfs merge).

use crate::daemon_client::try_connect_channel;
use crate::local_run::resolve_local_container_overlay;
use anyhow::{Context, Result, anyhow, bail};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ContainerStatusRequest};
use coral_runtime::StateStore;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpEndpoint {
    Host(PathBuf),
    Container { id: String, path: String },
}

pub fn parse_cp_endpoint(raw: &str) -> CpEndpoint {
    if let Some((left, right)) = raw.split_once(':')
        && !left.is_empty()
        && !left.contains('/')
        && !left.starts_with('.')
        && !right.is_empty()
    {
        return CpEndpoint::Container {
            id: left.to_string(),
            path: right.to_string(),
        };
    }
    CpEndpoint::Host(PathBuf::from(raw))
}

fn sanitize_container_rel_path(raw: &str) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for comp in Path::new(raw).components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(seg) => clean.push(seg),
            Component::ParentDir => {
                bail!("container path '{raw}' must not contain parent traversal ('..')");
            }
            Component::Prefix(_) => {
                bail!("invalid container path '{raw}'");
            }
        }
    }
    Ok(clean)
}

struct ResolvedContainer {
    id: String,
    lower_rootfs: PathBuf,
    upperdir: PathBuf,
    is_running: bool,
    runtime_root: Option<PathBuf>,
}

async fn resolve_container_target(
    addr: &str,
    state_root: &Path,
    id: &str,
) -> Result<ResolvedContainer> {
    if let Some(ch) = try_connect_channel(addr).await {
        let mut rt = RuntimeServiceClient::new(ch);
        if let Ok(resp) = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
        {
            let inner = resp.into_inner();
            let is_running = inner
                .status
                .as_ref()
                .is_some_and(|s| s.state == ContainerState::ContainerRunning as i32);
            let inspect = inner
                .info
                .get("inspectJson")
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok());
            let lower = inspect
                .as_ref()
                .and_then(|v| v["rootfsPath"].as_str())
                .map(PathBuf::from)
                .or_else(|| inner.status.as_ref().map(|s| PathBuf::from(&s.image_ref)))
                .unwrap_or_default();
            let bundle = inner
                .info
                .get("bundleDir")
                .map(PathBuf::from)
                .unwrap_or_default();
            let upperdir = bundle.join("overlay/upper");
            let runtime_root = bundle
                .parent()
                .and_then(|p| p.parent())
                .map(|sd| sd.join("runtime-state"));
            return Ok(ResolvedContainer {
                id: id.to_string(),
                lower_rootfs: lower,
                upperdir,
                is_running,
                runtime_root,
            });
        }
    }

    let (lower_rootfs, upperdir) = resolve_local_container_overlay(state_root, id)?;
    let daemon_rt = state_root.join("daemon/runtime-state");
    let (is_running, runtime_root) = if let Ok(st) = StateStore::new(state_root).load(id) {
        (
            st.status == coral_oci::ContainerStatus::Running,
            Some(state_root.to_path_buf()),
        )
    } else if let Ok(st) = StateStore::new(&daemon_rt).load(id) {
        (
            st.status == coral_oci::ContainerStatus::Running,
            Some(daemon_rt),
        )
    } else {
        (false, None)
    };

    Ok(ResolvedContainer {
        id: id.to_string(),
        lower_rootfs,
        upperdir,
        is_running,
        runtime_root,
    })
}

pub async fn cmd_cp(addr: &str, state_root: &Path, src: &str, dst: &str) -> Result<()> {
    match (parse_cp_endpoint(src), parse_cp_endpoint(dst)) {
        (CpEndpoint::Host(host_src), CpEndpoint::Container { id, path }) => {
            if !host_src.exists() {
                bail!("host source '{}' does not exist", host_src.display());
            }
            let rel = sanitize_container_rel_path(&path)?;
            let target = resolve_container_target(addr, state_root, &id).await?;
            copy_host_to_container(&target, &host_src, &rel)?;
            println!("Copied '{}' -> '{id}:/{}'", host_src.display(), rel.display());
            Ok(())
        }
        (CpEndpoint::Container { id, path }, CpEndpoint::Host(host_dst)) => {
            let rel = sanitize_container_rel_path(&path)?;
            let target = resolve_container_target(addr, state_root, &id).await?;
            copy_container_to_host(&target, &rel, &host_dst)?;
            println!("Copied '{id}:/{}' -> '{}'", rel.display(), host_dst.display());
            Ok(())
        }
        (CpEndpoint::Host(_), CpEndpoint::Host(_)) => {
            bail!("one of SRC or DST must specify a container path (`<CONTAINER_ID>:<PATH>`)")
        }
        (CpEndpoint::Container { .. }, CpEndpoint::Container { .. }) => {
            bail!("copying directly between two containers is not supported; copy via host")
        }
    }
}

fn copy_host_to_container(target: &ResolvedContainer, host_src: &Path, ctr_rel: &Path) -> Result<()> {
    // Live running container: stream through `coral exec --local` so Linux OverlayFS
    // never sees out-of-band writes to upperdir while mounted.
    if target.is_running
        && let Some(rt_root) = &target.runtime_root
        && StateStore::new(rt_root).load(&target.id).is_ok()
    {
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

fn copy_host_to_live_container(
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

fn copy_container_to_host(target: &ResolvedContainer, ctr_rel: &Path, host_dst: &Path) -> Result<()> {
    // 1. Live container: read from inside its actual mount namespace (sees /tmp tmpfs, volumes, and overlay merged view).
    if target.is_running
        && let Some(rt_root) = &target.runtime_root
        && StateStore::new(rt_root).load(&target.id).is_ok()
    {
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
        if probe.status.success() {
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
                    host_dst.join(ctr_rel.file_name().ok_or_else(|| anyhow!("invalid source filename"))?)
                } else {
                    if let Some(p) = host_dst.parent()
                        && !p.as_os_str().is_empty()
                    {
                        fs::create_dir_all(p)?;
                    }
                    host_dst.to_path_buf()
                };
                fs::write(&final_dst, &out.stdout)?;
                return Ok(());
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
                return Ok(());
            }
        }
    }

    // 2. Exited / static container: read merged view of OverlayFS upperdir over lower_rootfs.
    let upper_candidate = target.upperdir.join(ctr_rel);
    let lower_candidate = target.lower_rootfs.join(ctr_rel);

    if is_whiteout_in_upper(&target.upperdir, ctr_rel) {
        bail!("path '/{}' was deleted inside container '{}'", ctr_rel.display(), target.id);
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
        dst.join(src.file_name().ok_or_else(|| anyhow!("missing source filename"))?)
    } else {
        if let Some(p) = dst.parent()
            && !p.as_os_str().is_empty()
        {
            fs::create_dir_all(p)?;
        }
        dst.to_path_buf()
    };
    fs::copy(src, &final_dst).with_context(|| {
        format!(
            "copy '{}' -> '{}'",
            src.display(),
            final_dst.display()
        )
    })?;
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

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_and_container_cp_endpoints() {
        assert_eq!(
            parse_cp_endpoint("ctr-123:/etc/motd"),
            CpEndpoint::Container {
                id: "ctr-123".into(),
                path: "/etc/motd".into(),
            }
        );
        assert_eq!(
            parse_cp_endpoint("./local.txt"),
            CpEndpoint::Host(PathBuf::from("./local.txt"))
        );
        assert_eq!(
            parse_cp_endpoint("/tmp/out.txt"),
            CpEndpoint::Host(PathBuf::from("/tmp/out.txt"))
        );
    }

    #[test]
    fn rejects_parent_traversal_in_container_path() {
        assert!(sanitize_container_rel_path("/etc/motd").is_ok());
        assert!(sanitize_container_rel_path("/../../etc/shadow").is_err());
    }
}
