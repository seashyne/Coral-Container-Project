//! Container PID 1 initialization (`container_init_exec`), OverlayFS workspace preparation,
//! rootfs path resolution, and `$PATH` executable lookup.

use crate::RuntimeError;
use crate::overlay::OverlayDirs;
use crate::pty::attach_slave_stdio;
use crate::rootfs::prepare_rootfs;
use crate::security;
use coral_oci::OciSpec;
use nix::unistd::{Gid, Uid, chdir, execve, setgid, sethostname, setuid};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

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

    let process = spec
        .process
        .as_ref()
        .ok_or_else(|| RuntimeError::InitFailed("missing process in OCI spec".into()))?;

    let mut sec_cfg =
        security::SecurityConfig::from_annotations(&spec.annotations, process.no_new_privileges)?;
    // Pre-load any host-side custom Seccomp JSON profile BEFORE pivot_root hides the host FS.
    sec_cfg.seccomp = sec_cfg.seccomp.preload_before_pivot()?;

    let readonly = spec.root.as_ref().map(|r| r.readonly).unwrap_or(false)
        || spec
            .annotations
            .get("coral.readonly_rootfs")
            .is_some_and(|v| v == "true" || v == "1");
    prepare_rootfs(rootfs, overlay, &spec.mounts, readonly, rootless)?;

    // Apply OCI maskedPaths & readonlyPaths defense-in-depth on /proc and /sys after pivot_root.
    if !sec_cfg.privileged {
        let (masked, ro_paths) = spec
            .linux
            .as_ref()
            .map(|l| (l.masked_paths.as_slice(), l.readonly_paths.as_slice()))
            .unwrap_or((&[], &[]));
        crate::rootfs::hardening::apply_proc_sys_hardening(rootless, masked, ro_paths);
    }

    // Resolve optional `coral.user` (username or uid[:gid]) inside the pivoted container rootfs.
    let (target_uid, target_gid, user_home) = resolve_target_user(
        spec.annotations.get("coral.user").map(String::as_str),
        Some((process.user.uid, process.user.gid)),
    );

    if process.cwd != "/" {
        let _ = fs::create_dir_all(&process.cwd);
    }
    chdir(Path::new(&process.cwd))
        .map_err(|e| RuntimeError::nix(format!("chdir to {}", process.cwd), e))?;

    if sec_cfg.no_new_privileges {
        let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "prctl(PR_SET_NO_NEW_PRIVS)",
                io::Error::last_os_error(),
            ));
        }
    }

    let cap_mask = if !sec_cfg.privileged {
        let mask = security::compute_cap_mask(&sec_cfg.cap_drop, &sec_cfg.cap_add)?;
        // Phase 1: Drop disallowed caps from bounding set while still euid 0 (with CAP_SETPCAP).
        security::drop_bounding_set(mask);
        Some(mask)
    } else {
        None
    };

    if (target_uid != 0 || target_gid != 0)
        && is_id_mapped("/proc/self/uid_map", target_uid)
        && is_id_mapped("/proc/self/gid_map", target_gid)
    {
        if nix::unistd::geteuid().as_raw() == 0 {
            let extra_gids: Vec<Gid> = spec
                .process
                .as_ref()
                .map(|p| &p.user)
                .map(|u| {
                    u.additional_gids
                        .iter()
                        .copied()
                        .filter(|g| is_id_mapped("/proc/self/gid_map", *g))
                        .map(Gid::from_raw)
                        .collect()
                })
                .unwrap_or_default();
            let _ = nix::unistd::setgroups(&extra_gids);
        }
        if nix::unistd::getegid().as_raw() != target_gid {
            setgid(Gid::from_raw(target_gid))
                .map_err(|e| RuntimeError::nix(format!("setgid({target_gid})"), e))?;
        }
        if nix::unistd::geteuid().as_raw() != target_uid {
            setuid(Uid::from_raw(target_uid))
                .map_err(|e| RuntimeError::nix(format!("setuid({target_uid})"), e))?;
        }
    } else if target_uid != 0 || target_gid != 0 {
        warn!(
            uid = target_uid,
            gid = target_gid,
            "requested non-root container UID/GID is not mapped in single-UID rootless userns; continuing as container UID 0"
        );
    }

    // Phase 2: Enforce final effective/permitted capabilities, LSM (AppArmor/Landlock), and Seccomp-BPF.
    if let Some(mask) = cap_mask {
        if nix::unistd::geteuid().as_raw() == 0 {
            security::apply_capset(mask)?;
        }
        security::apply_apparmor_profile(sec_cfg.apparmor.as_deref())?;
        security::apply_landlock(sec_cfg.landlock, readonly, &process.cwd)?;
        security::apply_seccomp(&sec_cfg.seccomp)?;
    }

    let mut env_vec = process.env.clone();
    if !env_vec.iter().any(|e| e.starts_with("HOME=")) {
        let home = user_home.unwrap_or_else(|| {
            if target_uid == 0 {
                "/root".to_string()
            } else {
                "/tmp".to_string()
            }
        });
        env_vec.push(format!("HOME={home}"));
    }

    let exe = resolve_executable_in_container(&process.args[0], &env_vec)?;
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

    let envp: Vec<CString> = env_vec
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

fn resolve_target_user(
    user_ann: Option<&str>,
    spec_user: Option<(u32, u32)>,
) -> (u32, u32, Option<String>) {
    resolve_target_user_in(Path::new("/"), user_ann, spec_user)
}

pub(crate) fn resolve_target_user_in(
    root: &Path,
    user_ann: Option<&str>,
    spec_user: Option<(u32, u32)>,
) -> (u32, u32, Option<String>) {
    if let Some(raw) = user_ann.map(str::trim).filter(|s| !s.is_empty()) {
        let (u_part, g_part) = match raw.split_once(':') {
            Some((u, g)) => (u.trim(), Some(g.trim())),
            None => (raw, None),
        };
        let mut uid = u_part.parse::<u32>().ok();
        let mut gid = g_part.and_then(|g| g.parse::<u32>().ok());
        let mut home = None;

        if let Ok(passwd) = fs::read_to_string(root.join("etc/passwd")) {
            for line in passwd.lines() {
                let cols: Vec<&str> = line.split(':').collect();
                if cols.len() >= 6 {
                    let row_uid = cols[2].parse::<u32>().ok();
                    let row_gid = cols[3].parse::<u32>().ok();
                    let matched = match uid {
                        Some(u) => row_uid == Some(u),
                        None => cols[0] == u_part,
                    };
                    if matched {
                        uid = uid.or(row_uid);
                        if gid.is_none() && g_part.is_none() {
                            gid = row_gid;
                        }
                        if !cols[5].trim().is_empty() {
                            home = Some(cols[5].trim().to_string());
                        }
                        break;
                    }
                }
            }
        }
        if gid.is_none()
            && let Some(gname) = g_part
            && let Ok(group) = fs::read_to_string(root.join("etc/group"))
        {
            for line in group.lines() {
                let cols: Vec<&str> = line.split(':').collect();
                if cols.len() >= 3 && cols[0] == gname {
                    gid = cols[2].parse::<u32>().ok();
                    break;
                }
            }
        }
        let u = uid.unwrap_or(0);
        let g = gid.unwrap_or(u);
        return (u, g, home);
    }

    if let Some((u, g)) = spec_user {
        return (u, g, None);
    }
    (0, 0, None)
}

fn is_id_mapped(map_path: &str, id: u32) -> bool {
    let Ok(content) = fs::read_to_string(map_path) else {
        return true;
    };
    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let Some(inside) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(_outside) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(len) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if id >= inside && id < inside.saturating_add(len) {
            return true;
        }
    }
    false
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
