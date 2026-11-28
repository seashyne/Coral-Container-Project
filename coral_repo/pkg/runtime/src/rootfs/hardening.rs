//! OCI `maskedPaths` and `readonlyPaths` kernel hardening for `/proc` and `/sys`.

use nix::mount::{MsFlags, mount};
use std::path::Path;
use tracing::debug;

/// Locks the pivoted container root filesystem (`/`) read-only while preserving existing
/// mount flags (matching `runc`/`crun` rootless remount behavior).
pub fn remount_rootfs_readonly() {
    let mut flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c"/".as_ptr(), &mut st) } == 0 {
        let f = st.f_flag;
        if (f & libc::ST_NOSUID) != 0 {
            flags |= MsFlags::MS_NOSUID;
        }
        if (f & libc::ST_NODEV) != 0 {
            flags |= MsFlags::MS_NODEV;
        }
        if (f & libc::ST_NOEXEC) != 0 {
            flags |= MsFlags::MS_NOEXEC;
        }
    }
    if mount(None::<&str>, "/", None::<&str>, flags, None::<&str>).is_err() {
        let _ = mount(
            None::<&str>,
            "/",
            None::<&str>,
            MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY,
            None::<&str>,
        );
    }
}

/// Standard OCI/Docker sensitive kernel paths masked inside non-privileged containers.
pub const DEFAULT_MASKED_PATHS: &[&str] = &[
    "/proc/acpi",
    "/proc/asound",
    "/proc/kcore",
    "/proc/keys",
    "/proc/latency_stats",
    "/proc/timer_list",
    "/proc/timer_stats",
    "/proc/sched_debug",
    "/proc/scsi",
    "/sys/firmware",
    "/sys/devices/virtual/powercap",
];

/// Standard OCI/Docker kernel control paths remounted read-only inside non-privileged containers.
pub const DEFAULT_READONLY_PATHS: &[&str] = &[
    "/proc/bus",
    "/proc/fs",
    "/proc/irq",
    "/proc/sys",
    "/proc/sysrq-trigger",
];

/// Applies OCI `maskedPaths` and `readonlyPaths` inside the pivoted container mount namespace.
///
/// Missing paths are skipped gracefully; on restricted rootless kernels where a specific
/// sub-mount returns `EPERM`/`EACCES`, the failure is logged at `debug` level without breaking
/// container startup (matching `runc` rootless behavior).
pub fn apply_proc_sys_hardening(
    rootless: bool,
    custom_masked: &[String],
    custom_readonly: &[String],
) {
    let masked: Vec<&str> = if custom_masked.is_empty() {
        DEFAULT_MASKED_PATHS.to_vec()
    } else {
        custom_masked.iter().map(String::as_str).collect()
    };

    let readonly: Vec<&str> = if custom_readonly.is_empty() {
        DEFAULT_READONLY_PATHS.to_vec()
    } else {
        custom_readonly.iter().map(String::as_str).collect()
    };

    for raw in masked {
        mask_path(Path::new(raw));
    }
    for raw in readonly {
        make_path_readonly(Path::new(raw), rootless);
    }
}

fn mask_path(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if meta.is_dir() {
        let res = mount(
            Some("tmpfs"),
            path,
            Some("tmpfs"),
            MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
            Some("size=0,mode=0555"),
        );
        if let Err(e) = res {
            debug!(path = %path.display(), err = %e, "could not mask dir with ro tmpfs");
        }
    } else {
        let res = mount(
            Some(Path::new("/dev/null")),
            path,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        );
        if let Err(e) = res {
            debug!(path = %path.display(), err = %e, "could not mask file with /dev/null");
        }
    }
}

fn make_path_readonly(path: &Path, rootless: bool) {
    if !path.exists() {
        return;
    }
    if let Err(e) = mount(
        Some(path),
        path,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    ) {
        debug!(path = %path.display(), err = %e, "could not self-bind readonly path");
        return;
    }
    let mut flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY | MsFlags::MS_REC;
    if rootless {
        flags |= MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC;
    }
    if let Err(e) = mount(None::<&str>, path, None::<&str>, flags, None::<&str>) {
        // Fallback without extra flags for rootful or kernels with locked mount attributes.
        let fallback = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
        if let Err(e2) = mount(None::<&str>, path, None::<&str>, fallback, None::<&str>) {
            debug!(
                path = %path.display(),
                err = %e,
                fallback_err = %e2,
                "could not remount path read-only"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_oci_masked_and_readonly_lists_cover_critical_proc_and_sys_paths() {
        assert!(DEFAULT_MASKED_PATHS.contains(&"/proc/kcore"));
        assert!(DEFAULT_MASKED_PATHS.contains(&"/proc/keys"));
        assert!(DEFAULT_MASKED_PATHS.contains(&"/sys/firmware"));
        assert!(DEFAULT_READONLY_PATHS.contains(&"/proc/sys"));
        assert!(DEFAULT_READONLY_PATHS.contains(&"/proc/bus"));
        assert!(DEFAULT_READONLY_PATHS.contains(&"/proc/irq"));
    }
}
