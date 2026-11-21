//! Per-container Cgroups v2 directory management.

use super::fs_io::{parse_cpu_stat_usage_usec, read_optional_u64, write_cgroup_file, write_if_controller_present};
use super::{CgroupApplyOutcome, CgroupConfig, CgroupError, CgroupStats, DEFAULT_CGROUP_V2_ROOT, DEFAULT_CPU_PERIOD_US};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Manages per-container Cgroups v2 directories under a configurable base path.
#[derive(Debug, Clone)]
pub struct CgroupManager {
    base_dir: PathBuf,
}

impl CgroupManager {
    /// Creates a `CgroupManager` targeting an explicit cgroup base directory (useful for tests or custom slices).
    pub fn with_root(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Detects the appropriate Cgroups v2 base directory for the current process (root or delegated rootless).
    pub fn detect() -> Self {
        let euid = unsafe { libc::geteuid() };
        if euid == 0 {
            return Self::with_root(Path::new(DEFAULT_CGROUP_V2_ROOT).join("coral"));
        }

        if let Some(rel) = super::detect_self_cgroup_v2_path() {
            let trimmed = rel.trim_start_matches('/');
            let base = Path::new(DEFAULT_CGROUP_V2_ROOT).join(trimmed);
            return Self::with_root(base);
        }

        Self::with_root(Path::new(DEFAULT_CGROUP_V2_ROOT).join("coral"))
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn container_cgroup_path(&self, container_id: &str) -> PathBuf {
        self.base_dir.join(format!("coral-{container_id}"))
    }

    /// Strictly creates a container cgroup, writes limits, and attaches `pid`.
    pub fn apply(
        &self,
        container_id: &str,
        pid: u32,
        config: &CgroupConfig,
    ) -> Result<PathBuf, CgroupError> {
        self.ensure_base_dir()?;

        let cg_path = self.container_cgroup_path(container_id);
        if !cg_path.exists() {
            fs::create_dir(&cg_path).map_err(|source| CgroupError::Io {
                path: cg_path.clone(),
                source,
            })?;
        }

        self.write_limits(&cg_path, config)?;

        let procs_path = cg_path.join("cgroup.procs");
        write_cgroup_file(&procs_path, &pid.to_string())?;

        debug!(path = %cg_path.display(), pid, "attached container PID to cgroup v2");
        Ok(cg_path)
    }

    /// Applies cgroup limits; in unprivileged/rootless environments where systemd does not allow
    /// creating child cgroups inside a leaf scope, logs a warning and returns `Skipped` instead of failing.
    pub fn apply_best_effort(
        &self,
        container_id: &str,
        pid: u32,
        config: &CgroupConfig,
    ) -> Result<CgroupApplyOutcome, CgroupError> {
        let is_root = unsafe { libc::geteuid() } == 0;
        match self.apply(container_id, pid, config) {
            Ok(path) => Ok(CgroupApplyOutcome::Applied { path }),
            Err(CgroupError::Io { path, source }) if !is_root => {
                let kind = source.kind();
                let raw = source.raw_os_error();
                if matches!(
                    kind,
                    io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
                ) || matches!(raw, Some(libc::EBUSY | libc::EROFS | libc::EACCES | libc::EPERM))
                {
                    let reason = format!(
                        "rootless cgroup delegation unavailable at {}: {source}",
                        path.display()
                    );
                    warn!("{reason}");
                    let _ = fs::remove_dir(self.container_cgroup_path(container_id));
                    Ok(CgroupApplyOutcome::Skipped { reason })
                } else {
                    Err(CgroupError::Io { path, source })
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Reads live CPU/Memory/PIDs metrics for a running container cgroup.
    pub fn read_stats(&self, container_id: &str) -> Result<CgroupStats, CgroupError> {
        let cg_path = self.container_cgroup_path(container_id);
        let memory_current_bytes = read_optional_u64(&cg_path.join("memory.current"));
        let pids_current = read_optional_u64(&cg_path.join("pids.current"));
        let cpu_usage_usec = fs::read_to_string(cg_path.join("cpu.stat"))
            .ok()
            .and_then(|s| parse_cpu_stat_usage_usec(&s));

        Ok(CgroupStats {
            memory_current_bytes,
            pids_current,
            cpu_usage_usec,
        })
    }

    /// Removes the container's cgroup directory (ignoring `NotFound`).
    pub fn cleanup(&self, container_id: &str) -> Result<(), CgroupError> {
        let cg_path = self.container_cgroup_path(container_id);
        match fs::remove_dir(&cg_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(CgroupError::Io {
                path: cg_path,
                source,
            }),
        }
    }

    fn ensure_base_dir(&self) -> Result<(), CgroupError> {
        if !self.base_dir.exists() {
            fs::create_dir_all(&self.base_dir).map_err(|source| CgroupError::Io {
                path: self.base_dir.clone(),
                source,
            })?;
        }

        // Best-effort enable controllers on parent subtree_control if available.
        let subtree = self.base_dir.join("cgroup.subtree_control");
        if subtree.exists() {
            let _ = fs::write(&subtree, "+cpu +memory +pids");
        }
        Ok(())
    }

    fn write_limits(&self, cg_path: &Path, config: &CgroupConfig) -> Result<(), CgroupError> {
        if let Some(mem_max) = config.memory_max {
            if mem_max <= 0 {
                return Err(CgroupError::InvalidConfig("memory_max must be > 0".into()));
            }
            write_if_controller_present(&cg_path.join("memory.max"), &mem_max.to_string())?;
        }

        if let Some(mem_high) = config.memory_high {
            if mem_high <= 0 {
                return Err(CgroupError::InvalidConfig("memory_high must be > 0".into()));
            }
            write_if_controller_present(&cg_path.join("memory.high"), &mem_high.to_string())?;
        }

        if config.cpu_quota.is_some() || config.cpu_period.is_some() {
            let period = config.cpu_period.unwrap_or(DEFAULT_CPU_PERIOD_US);
            let value = match config.cpu_quota {
                Some(q) if q > 0 => format!("{q} {period}"),
                _ => format!("max {period}"),
            };
            write_if_controller_present(&cg_path.join("cpu.max"), &value)?;
        }

        if let Some(weight) = config.cpu_weight {
            let clamped = weight.clamp(1, 10_000);
            write_if_controller_present(&cg_path.join("cpu.weight"), &clamped.to_string())?;
        }

        if let Some(pids_max) = config.pids_max {
            if pids_max <= 0 {
                return Err(CgroupError::InvalidConfig("pids_max must be > 0".into()));
            }
            write_if_controller_present(&cg_path.join("pids.max"), &pids_max.to_string())?;
        }

        Ok(())
    }
}
