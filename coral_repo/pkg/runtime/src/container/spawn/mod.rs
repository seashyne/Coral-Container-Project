//! Double-fork + `unshare` + UserNS pipe synchronization + Cgroups v2 attach
//! for foreground (`run`) and detached (`create`) container execution.

mod detached;
mod foreground;
#[cfg(test)]
mod tests;

use super::init::resolve_target_user_in;
use crate::RuntimeError;
use crate::namespace::{NamespacePlan, build_namespace_plan, expand_multi_uid_mappings};
use nix::fcntl::{OFlag, open};
use nix::sys::stat::Mode;
use nix::sys::wait::WaitStatus;
use nix::unistd::{pipe2, read, write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use coral_oci::OciSpec;

static FORWARD_SIGNAL_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal_handler(sig: libc::c_int) {
    let pid = FORWARD_SIGNAL_PID.load(Ordering::Relaxed);
    if pid > 0 {
        unsafe {
            let _ = libc::kill(pid, sig);
        }
    }
}

struct SignalForwardGuard {
    saved: [(libc::c_int, libc::sigaction); 4],
}

impl SignalForwardGuard {
    fn install(target_pid: i32) -> Self {
        FORWARD_SIGNAL_PID.store(target_pid, Ordering::SeqCst);
        let sigs = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT];
        let mut saved: [(libc::c_int, libc::sigaction); 4] =
            unsafe { std::mem::zeroed() };
        for (i, &sig) in sigs.iter().enumerate() {
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = forward_signal_handler as *const () as usize;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                let _ = libc::sigaction(sig, &sa, &mut old);
                saved[i] = (sig, old);
            }
        }
        Self { saved }
    }
}

impl Drop for SignalForwardGuard {
    fn drop(&mut self) {
        FORWARD_SIGNAL_PID.store(0, Ordering::SeqCst);
        for (sig, old) in &self.saved {
            unsafe {
                let _ = libc::sigaction(*sig, old, std::ptr::null_mut());
            }
        }
    }
}

pub(crate) fn new_cloexec_pipe() -> Result<(OwnedFd, OwnedFd), RuntimeError> {
    pipe2(OFlag::O_CLOEXEC).map_err(|e| RuntimeError::nix("pipe2(O_CLOEXEC)", e))
}

pub(crate) fn redirect_stdio_to_devnull() {
    if let Ok(fd) = open("/dev/null", OFlag::O_RDWR, Mode::empty()) {
        unsafe {
            let _ = libc::dup2(fd, libc::STDIN_FILENO);
            let _ = libc::dup2(fd, libc::STDOUT_FILENO);
            let _ = libc::dup2(fd, libc::STDERR_FILENO);
            if fd > libc::STDERR_FILENO {
                let _ = libc::close(fd);
            }
        }
    }
}

pub(crate) fn write_all_fd(fd: &OwnedFd, mut buf: &[u8]) -> Result<(), RuntimeError> {
    while !buf.is_empty() {
        let n = write(fd, buf).map_err(|e| RuntimeError::nix("write sync pipe", e))?;
        buf = &buf[n..];
    }
    Ok(())
}

pub(crate) fn read_exact_fd(fd: &OwnedFd, mut buf: &mut [u8]) -> Result<(), RuntimeError> {
    while !buf.is_empty() {
        let n = read(fd.as_raw_fd(), buf).map_err(|e| RuntimeError::nix("read sync pipe", e))?;
        if n == 0 {
            return Err(RuntimeError::InitFailed("unexpected EOF on sync pipe".into()));
        }
        buf = &mut buf[n..];
    }
    Ok(())
}

pub(crate) fn read_to_string_fd(fd: &OwnedFd) -> String {
    let mut out = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match read(fd.as_raw_fd(), &mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn wait_status_to_exit_code(status: WaitStatus) -> i32 {
    match status {
        WaitStatus::Exited(_, code) => code,
        WaitStatus::Signaled(_, sig, _) => 128 + (sig as i32),
        _ => 1,
    }
}

/// Resolves the UserNS mapping plan, preferring full Multi-UID subordinate ranges
/// (`newuidmap`/`newgidmap` + `/etc/subuid`/`/etc/subgid`) and automatically falling
/// back to single-UID mapping when unavailable or explicitly disabled.
fn resolve_namespace_plan(spec: &OciSpec, rootfs: &Path) -> NamespacePlan {
    let mut ns_plan = build_namespace_plan(spec);
    if !expand_multi_uid_mappings(&mut ns_plan, spec) {
        adjust_single_uid_mapping(&mut ns_plan, spec, rootfs);
    }
    ns_plan
}

/// Maps requested non-root `uid`/`gid` directly to host caller UID/GID when operating in
/// single-UID rootless UserNS (`0 host 1`), allowing `postgres`/`redis`/`nginx` non-root
/// services to run natively without `newuidmap`.
fn adjust_single_uid_mapping(ns_plan: &mut NamespacePlan, spec: &OciSpec, rootfs: &Path) {
    if !ns_plan.use_user_ns {
        return;
    }
    let single_uid_default = ns_plan.uid_mappings.len() == 1
        && ns_plan.uid_mappings[0].container_id == 0
        && ns_plan.uid_mappings[0].size == 1;
    let single_gid_default = ns_plan.gid_mappings.len() == 1
        && ns_plan.gid_mappings[0].container_id == 0
        && ns_plan.gid_mappings[0].size == 1;
    if !single_uid_default && !single_gid_default {
        return;
    }
    let spec_user = spec.process.as_ref().map(|p| (p.user.uid, p.user.gid));
    let (target_uid, target_gid, _) = resolve_target_user_in(
        rootfs,
        spec.annotations.get("coral.user").map(String::as_str),
        spec_user,
    );
    if single_uid_default && target_uid != 0 {
        ns_plan.uid_mappings[0].container_id = target_uid;
    }
    if single_gid_default && target_gid != 0 {
        ns_plan.gid_mappings[0].container_id = target_gid;
    }
}
