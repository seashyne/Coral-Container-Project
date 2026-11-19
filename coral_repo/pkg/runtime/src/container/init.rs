//! Container PID 1 initialization (`container_init_exec`), OverlayFS workspace preparation,
//! rootfs path resolution, and `$PATH` executable lookup.

use crate::RuntimeError;
use crate::overlay::OverlayDirs;
use crate::pty::attach_slave_stdio;
use crate::rootfs::prepare_rootfs;
use crate::security;
use coral_oci::OciSpec;
use nix::unistd::{chdir, execve, sethostname};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use tracing::debug;

pub(crate) fn prepare_overlay_if_requested(
    bundle_abs: &Path,
    rootfs_abs: &Path,
    spec: &OciSpec,
) -> Result<Option<OverlayDirs>, RuntimeError> {
    let overlay_host_bundle = spec
        .annotations
        .get("coral.overlay.bundle")
        .map(PathBuf::from)
        .unwrap_or_else(|| bundle_abs.to_path_buf());
    if let Some(lower) = spec.annotations.get("coral.overlay.lowerdir") {
        let mut layers = Vec::new();
        for part in lower.split(':').map(str::trim).filter(|s| !s.is_empty()) {
            let abs = fs::canonicalize(Path::new(part))
                .map_err(|e| RuntimeError::sys(format!("resolve overlay lowerdir '{part}'"), e))?;
            layers.push(abs);
        }
        return Ok(Some(OverlayDirs::prepare_multi(
            &overlay_host_bundle,
            layers,
        )?));
    }
    if spec
        .annotations
        .get("coral.overlay")
        .is_some_and(|v| v == "true" || v == "1")
    {
        return Ok(Some(OverlayDirs::prepare(
            &overlay_host_bundle,
            rootfs_abs.to_path_buf(),
        )?));
    }
    Ok(None)
}

/// Runs inside container PID 1 right before `execve`.
pub(crate) fn container_init_exec(
    spec: &OciSpec,
    rootfs: &Path,
    overlay: Option<&OverlayDirs>,
    pty_slave: Option<RawFd>,
    rootless: bool,
    use_net_ns: bool,
) -> Result<(), RuntimeError> {
    if let Some(sfd) = pty_slave {
        attach_slave_stdio(sfd)?;
    }

    if let Some(hostname) = &spec.hostname {
        sethostname(hostname).map_err(|e| RuntimeError::nix("sethostname", e))?;
    }

    if use_net_ns {
        let _ = coral_net::bring_loopback_up();
    }

    let readonly = spec.root.as_ref().map(|r| r.readonly).unwrap_or(false);
    prepare_rootfs(rootfs, overlay, &spec.mounts, readonly, rootless)?;

    let process = spec
        .process
        .as_ref()
        .ok_or_else(|| RuntimeError::InitFailed("missing process in OCI spec".into()))?;

    chdir(Path::new(&process.cwd))
        .map_err(|e| RuntimeError::nix(format!("chdir to {}", process.cwd), e))?;

    let privileged = spec
        .annotations
        .get("coral.privileged")
        .is_some_and(|v| v == "true");

    if process.no_new_privileges || !privileged {
        let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "prctl(PR_SET_NO_NEW_PRIVS)",
                io::Error::last_os_error(),
            ));
        }
    }

    if !privileged {
        security::drop_capabilities()?;
        security::apply_default_seccomp()?;
    }

    let exe = resolve_executable_in_container(&process.args[0], &process.env)?;
    let exe_cstr = CString::new(exe.as_str())
        .map_err(|e| RuntimeError::InitFailed(format!("invalid executable NUL byte: {e}")))?;

    let argv: Vec<CString> = process
        .args
        .iter()
        .map(|s| {
            CString::new(s.as_str())
                .map_err(|e| RuntimeError::InitFailed(format!("invalid arg NUL byte: {e}")))
        })
        .collect::<Result<_, _>>()?;

    let envp: Vec<CString> = process
        .env
        .iter()
        .map(|s| {
            CString::new(s.as_str())
                .map_err(|e| RuntimeError::InitFailed(format!("invalid env NUL byte: {e}")))
        })
        .collect::<Result<_, _>>()?;

    debug!(exe = %exe, args = ?process.args, "execve container entrypoint");
    execve(&exe_cstr, &argv, &envp)
        .map_err(|e| RuntimeError::nix(format!("execve '{exe}'"), e))?;
}

pub(crate) fn resolve_rootfs(bundle: &Path, spec: &OciSpec) -> Result<PathBuf, RuntimeError> {
    let raw = &spec
        .root
        .as_ref()
        .ok_or_else(|| RuntimeError::InitFailed("missing root section".into()))?
        .path;
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        bundle.join(raw)
    };
    fs::canonicalize(&joined)
        .map_err(|e| RuntimeError::sys(format!("resolve rootfs '{}'", joined.display()), e))
}

fn resolve_executable_in_container(prog: &str, env: &[String]) -> Result<String, RuntimeError> {
    if prog.contains('/') {
        return Ok(prog.to_string());
    }
    let path_var = env
        .iter()
        .find_map(|e| e.strip_prefix("PATH="))
        .unwrap_or("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");

    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(prog);
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(RuntimeError::InitFailed(format!(
        "executable '{prog}' not found in container PATH '{path_var}'"
    )))
}
