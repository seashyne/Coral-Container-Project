//! Host capability probing: `no_new_privs` state and setuid-capable helper discovery.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub(super) fn has_no_new_privs() -> bool {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return false;
    };
    status.lines().any(|line| {
        line.strip_prefix("NoNewPrivs:")
            .is_some_and(|v| v.trim() == "1")
    })
}

pub(super) fn find_executable(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for fallback in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        let p = PathBuf::from(fallback);
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }

    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable_suid_capable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable_suid_capable(path: &std::path::Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || (meta.permissions().mode() & 0o111) == 0 {
        return false;
    }
    let Ok(cpath) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut st) };
    !(rc == 0 && (st.f_flag & libc::ST_NOSUID) != 0)
}
