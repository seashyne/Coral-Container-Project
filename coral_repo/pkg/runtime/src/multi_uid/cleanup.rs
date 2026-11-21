//! Rootless-safe recursive directory removal via an unshared Multi-UID UserNS helper.

use super::MultiUidSupport;
use nix::fcntl::OFlag;
use nix::unistd::pipe2;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// Removes `path` recursively, automatically retrying inside an unshared Multi-UID UserNS
/// (`newuidmap`/`newgidmap`) if subordinate-UID-owned subdirectories cause `PermissionDenied`.
pub(super) fn remove_dir_all_rootless_safe(path: &Path) -> io::Result<()> {
    let first_err = match fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
        Err(e) if e.kind() != io::ErrorKind::PermissionDenied => return Err(e),
        Err(e) => e,
    };

    make_tree_writable_best_effort(path);
    match fs::remove_dir_all(path) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() != io::ErrorKind::PermissionDenied => return Err(e),
        Err(_) => {}
    }

    let support = MultiUidSupport::detect();
    if !support.is_available() {
        return Err(first_err);
    }

    let (Some(uidmap_bin), Some(gidmap_bin)) = (&support.newuidmap, &support.newgidmap) else {
        return Err(first_err);
    };
    let uid_maps = support.uid_mappings();
    let gid_maps = support.gid_mappings();

    let (go_r, go_w) =
        pipe2(OFlag::O_CLOEXEC).map_err(|e| io::Error::from_raw_os_error(e as i32))?;

    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "IFS= read -r _ || true; chmod -R u+rwX -- \"$1\" 2>/dev/null || true; exec rm -rf -- \"$1\"",
        "coral-rootless-rm",
    ])
    .arg(path)
    .stdin(Stdio::from(go_r));

    // SAFETY: Inside `pre_exec`, ONLY async-signal-safe `libc::unshare(CLONE_NEWUSER)` runs.
    // We do NOT read pipes inside `pre_exec` because `Command::spawn()` waits for `execve` to
    // succeed before returning to the parent; blocking inside `sh` via stdin instead avoids deadlock,
    // and the subsequent `exec` of `chmod`/`rm` refreshes capabilities for UID 0 in the new UserNS.
    unsafe {
        cmd.pre_exec(|| {
            if libc::unshare(libc::CLONE_NEWUSER) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = cmd.spawn()?;
    let child_pid = child.id() as i32;
    let map_ok = super::apply::run_idmap_helper(uidmap_bin, child_pid, &uid_maps).is_ok()
        && super::apply::run_idmap_helper(gidmap_bin, child_pid, &gid_maps).is_ok();

    drop(go_w);
    let status = child.wait()?;
    if map_ok && status.success() && fs::symlink_metadata(path).is_err() {
        Ok(())
    } else {
        Err(first_err)
    }
}

fn make_tree_writable_best_effort(path: &Path) {
    let mut stack = vec![path.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&cur) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let mode = meta.permissions().mode();
            if mode & 0o700 != 0o700 {
                let _ = fs::set_permissions(&cur, fs::Permissions::from_mode(mode | 0o700));
            }
            if let Ok(entries) = fs::read_dir(&cur) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
        }
    }
}
