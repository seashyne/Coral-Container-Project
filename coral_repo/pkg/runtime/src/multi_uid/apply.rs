//! Application of UserNS UID/GID mappings via `newuidmap`/`newgidmap` or direct procfs writes.

use super::parse::format_helper_args;
use crate::RuntimeError;
use coral_oci::LinuxIdMapping;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

/// Applies UserNS UID/GID mappings for `pid`, using `newuidmap`/`newgidmap` whenever mappings
/// require unprivileged subordinate ranges or multiple extents, and falling back to direct
/// `/proc/<pid>/{setgroups,uid_map,gid_map}` writes for root or single-UID self-mapping.
pub(super) fn apply_userns_mappings(
    pid: i32,
    uid_mappings: &[LinuxIdMapping],
    gid_mappings: &[LinuxIdMapping],
    format_direct: impl Fn(&[LinuxIdMapping]) -> String,
) -> Result<(), RuntimeError> {
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let euid = unsafe { libc::geteuid() };
    let egid = unsafe { libc::getegid() };
    let is_root = euid == 0;

    let uid_needs_helper = !is_root && !is_self_single_map(uid_mappings, euid);
    let gid_needs_helper = !is_root && !is_self_single_map(gid_mappings, egid);

    if uid_needs_helper {
        let bin = super::probe::find_executable("newuidmap").ok_or_else(|| {
            RuntimeError::InitFailed(
                "multi-UID rootless mapping requires 'newuidmap' (package 'uidmap')".into(),
            )
        })?;
        run_idmap_helper(&bin, pid, uid_mappings)?;
    } else {
        fs::write(proc_dir.join("uid_map"), format_direct(uid_mappings))
            .map_err(|e| RuntimeError::sys("write /proc/<pid>/uid_map", e))?;
    }

    if gid_needs_helper {
        // Leave `/proc/<pid>/setgroups` at its kernel default ("allow") so `newgidmap`
        // enables `setgroups()`, `su`, `gosu`, and `_apt` inside the container.
        let bin = super::probe::find_executable("newgidmap").ok_or_else(|| {
            RuntimeError::InitFailed(
                "multi-UID rootless mapping requires 'newgidmap' (package 'uidmap')".into(),
            )
        })?;
        run_idmap_helper(&bin, pid, gid_mappings)?;
    } else {
        if !is_root {
            fs::write(proc_dir.join("setgroups"), "deny")
                .map_err(|e| RuntimeError::sys("write /proc/<pid>/setgroups", e))?;
        }
        fs::write(proc_dir.join("gid_map"), format_direct(gid_mappings))
            .map_err(|e| RuntimeError::sys("write /proc/<pid>/gid_map", e))?;
    }

    debug!(
        pid,
        uid_extents = uid_mappings.len(),
        gid_extents = gid_mappings.len(),
        used_newuidmap = uid_needs_helper,
        used_newgidmap = gid_needs_helper,
        "applied UserNS UID/GID mappings"
    );
    Ok(())
}

fn is_self_single_map(mappings: &[LinuxIdMapping], host_id: u32) -> bool {
    mappings.len() == 1 && mappings[0].size == 1 && mappings[0].host_id == host_id
}

pub(super) fn run_idmap_helper(
    binary: &Path,
    pid: i32,
    mappings: &[LinuxIdMapping],
) -> Result<(), RuntimeError> {
    let args = format_helper_args(pid, mappings);
    let output = Command::new(binary)
        .args(&args)
        .output()
        .map_err(|e| RuntimeError::sys(format!("execute '{}'", binary.display()), e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RuntimeError::InitFailed(format!(
            "'{} {}' failed ({}): {}",
            binary.display(),
            args.join(" "),
            output.status,
            stderr.trim()
        )));
    }
    Ok(())
}
