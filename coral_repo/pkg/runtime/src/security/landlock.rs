//! Pure-Rust Rootless Linux Landlock LSM (`SYS_landlock_*`, kernel 5.13+) and
//! optional AppArmor exec profile attachment (`--security-opt apparmor=<profile>`).

use crate::RuntimeError;
use std::ffi::CString;
use std::fs;
use std::io;
use std::path::Path;
use tracing::{debug, warn};

const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;
const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

// Landlock ABI v1 filesystem access rights (kernel 5.13+).
const LL_ACCESS_FS_EXECUTE: u64 = 1 << 0;
const LL_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LL_ACCESS_FS_READ_FILE: u64 = 1 << 2;
const LL_ACCESS_FS_READ_DIR: u64 = 1 << 3;
const LL_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LL_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LL_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LL_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LL_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LL_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const LL_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LL_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const LL_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;

const LL_FS_READONLY: u64 = LL_ACCESS_FS_EXECUTE | LL_ACCESS_FS_READ_FILE | LL_ACCESS_FS_READ_DIR;
const LL_FS_ALL_V1: u64 = LL_FS_READONLY
    | LL_ACCESS_FS_WRITE_FILE
    | LL_ACCESS_FS_REMOVE_DIR
    | LL_ACCESS_FS_REMOVE_FILE
    | LL_ACCESS_FS_MAKE_CHAR
    | LL_ACCESS_FS_MAKE_DIR
    | LL_ACCESS_FS_MAKE_REG
    | LL_ACCESS_FS_MAKE_SOCK
    | LL_ACCESS_FS_MAKE_FIFO
    | LL_ACCESS_FS_MAKE_BLOCK
    | LL_ACCESS_FS_MAKE_SYM;

#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}

#[repr(C, packed)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LandlockMode {
    /// Disabled (default for standard containers).
    #[default]
    Off,
    /// Enforce Landlock LSM filesystem confinement (rootless-safe on kernel 5.13+).
    /// Read-only on `/` + read-write only on `/tmp`, `/var`, `/run`, `/dev`, `/home`, `/root`,
    /// `/app`, `/data`, `/work` (and container workdir).
    Enforce,
}

/// Probes the host kernel's Linux Landlock LSM ABI version (`Ok(Some(abi))` if available,
/// `Ok(None)` if unsupported or disabled by kernel boot parameters).
pub fn probe_landlock_abi() -> Option<u32> {
    let rc = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<LandlockRulesetAttr>(),
            0usize,
            LANDLOCK_CREATE_RULESET_VERSION,
        )
    };
    if rc >= 1 {
        Some(rc as u32)
    } else {
        None
    }
}

/// Enforces rootless Linux Landlock LSM confinement inside the container right before `execve`.
/// Requires `PR_SET_NO_NEW_PRIVS` to already be enabled.
pub fn apply_landlock(mode: LandlockMode, readonly_rootfs: bool, cwd: &str) -> Result<(), RuntimeError> {
    if mode == LandlockMode::Off {
        return Ok(());
    }
    let Some(abi) = probe_landlock_abi() else {
        warn!("Landlock LSM requested but kernel does not expose Landlock ABI; continuing");
        return Ok(());
    };

    let attr = LandlockRulesetAttr {
        handled_access_fs: LL_FS_ALL_V1,
    };
    let ruleset_fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0u32,
        )
    };
    if ruleset_fd < 0 {
        return Err(RuntimeError::sys(
            "landlock_create_ruleset",
            io::Error::last_os_error(),
        ));
    }
    let ruleset_fd = ruleset_fd as i32;

    // Allow read + execute across the pivoted container rootfs (`/`).
    let _ = add_path_rule(ruleset_fd, "/", LL_FS_READONLY);

    // Allow write only inside standard writable container paths unless rootfs is read-only.
    let mut writable = vec!["/tmp", "/run", "/dev/shm", "/dev/null", "/dev/tty", "/dev/pts"];
    if !readonly_rootfs {
        writable.extend_from_slice(&["/var", "/home", "/root", "/app", "/data", "/work", "/srv"]);
        if cwd != "/" && !writable.contains(&cwd) {
            writable.push(cwd);
        }
    }
    for path in writable {
        let _ = add_path_rule(ruleset_fd, path, LL_FS_ALL_V1);
    }

    let rc = unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset_fd, 0u32) };
    unsafe { libc::close(ruleset_fd) };
    if rc != 0 {
        return Err(RuntimeError::sys(
            "landlock_restrict_self",
            io::Error::last_os_error(),
        ));
    }

    debug!(abi, readonly_rootfs, "enforced rootless Linux Landlock LSM");
    Ok(())
}

fn add_path_rule(ruleset_fd: i32, path: &str, allowed_access: u64) -> bool {
    if !Path::new(path).exists() {
        return false;
    }
    let Ok(cpath) = CString::new(path) else {
        return false;
    };
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
    if fd < 0 {
        return false;
    }
    let rule = LandlockPathBeneathAttr {
        allowed_access,
        parent_fd: fd,
    };
    let rc = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset_fd,
            LANDLOCK_RULE_PATH_BENEATH,
            &rule as *const LandlockPathBeneathAttr,
            0u32,
        )
    };
    unsafe { libc::close(fd) };
    rc == 0
}

/// Applies an optional AppArmor exec profile (`--security-opt apparmor=<profile>`).
pub fn apply_apparmor_profile(profile: Option<&str>) -> Result<(), RuntimeError> {
    let Some(name) = profile.map(str::trim).filter(|s| !s.is_empty() && *s != "unconfined") else {
        return Ok(());
    };
    let attr_paths = ["/proc/self/attr/apparmor/exec", "/proc/self/attr/exec"];
    let payload = format!("changeprofile {name}");
    for p in attr_paths {
        if Path::new(p).exists() {
            fs::write(p, payload.as_bytes()).map_err(|e| {
                RuntimeError::sys(format!("apply AppArmor profile '{name}' via {p}"), e)
            })?;
            debug!(profile = name, "scheduled AppArmor exec profile transition");
            return Ok(());
        }
    }
    warn!(profile = name, "AppArmor LSM interface not present on host kernel");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_landlock_abi_is_safe_to_call_on_any_linux_kernel() {
        // Returns Some(abi >= 1) on Linux 5.13+ with Landlock enabled, or None otherwise.
        if let Some(abi) = probe_landlock_abi() {
            assert!(abi >= 1);
        }
    }
}
