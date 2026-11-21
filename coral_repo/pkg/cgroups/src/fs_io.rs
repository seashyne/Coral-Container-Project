//! Cgroups v2 filesystem helpers: knob writes, stats reads, and `/proc/self/cgroup` parsing.

use super::{CgroupError, DEFAULT_CGROUP_V2_ROOT};
use std::fs;
use std::path::Path;
use tracing::debug;

pub(super) fn write_cgroup_file(path: &Path, content: &str) -> Result<(), CgroupError> {
    fs::write(path, content).map_err(|source| CgroupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

pub(super) fn write_if_controller_present(path: &Path, content: &str) -> Result<(), CgroupError> {
    // On real cgroupfs, controller files exist iff that controller is enabled in subtree_control.
    // On regular filesystems (tests), create/write directly if parent exists.
    if path.exists() || !is_real_cgroupfs(path) {
        write_cgroup_file(path, content)?;
    } else {
        debug!(path = %path.display(), "cgroup controller file absent; skipping knob");
    }
    Ok(())
}

fn is_real_cgroupfs(path: &Path) -> bool {
    path.starts_with(DEFAULT_CGROUP_V2_ROOT)
}

pub(super) fn read_optional_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}

pub(super) fn parse_cpu_stat_usage_usec(content: &str) -> Option<u64> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("usage_usec ") {
            return rest.trim().parse::<u64>().ok();
        }
    }
    None
}

/// Parses `/proc/self/cgroup` unified v2 line (`0::<path>`).
pub fn detect_self_cgroup_v2_path() -> Option<String> {
    let content = fs::read_to_string("/proc/self/cgroup").ok()?;
    parse_proc_cgroup_v2(&content)
}

pub(super) fn parse_proc_cgroup_v2(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            return Some(path.trim().to_string());
        }
    }
    None
}
