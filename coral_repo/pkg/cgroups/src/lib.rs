//! Linux Cgroups v2 resource management (Memory, CPU, PIDs) and live observability stats.

use coral_oci::LinuxResources;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, warn};

const DEFAULT_CGROUP_V2_ROOT: &str = "/sys/fs/cgroup";
const DEFAULT_CPU_PERIOD_US: u64 = 100_000;

#[derive(Debug, Error)]
pub enum CgroupError {
    #[error("cgroup I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid cgroup configuration: {0}")]
    InvalidConfig(String),
}

/// Resource limits translated into Linux Cgroups v2 knobs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CgroupConfig {
    /// Hard memory limit in bytes (`memory.max`).
    pub memory_max: Option<i64>,
    /// Soft memory throttle threshold in bytes (`memory.high`).
    pub memory_high: Option<i64>,
    /// CPU quota in microseconds per period (`cpu.max` first field).
    pub cpu_quota: Option<i64>,
    /// CPU period in microseconds (`cpu.max` second field, default 100000).
    pub cpu_period: Option<u64>,
    /// Cgroups v2 CPU weight in range 1..=10000 (`cpu.weight`).
    pub cpu_weight: Option<u64>,
    /// Maximum number of processes/threads (`pids.max`).
    pub pids_max: Option<i64>,
}

impl CgroupConfig {
    /// Converts OCI `LinuxResources` into Cgroups v2 parameters.
    pub fn from_oci(resources: &LinuxResources) -> Self {
        let mut cfg = Self::default();

        if let Some(mem) = &resources.memory {
            cfg.memory_max = mem.limit;
            cfg.memory_high = mem.reservation;
        }
        if let Some(cpu) = &resources.cpu {
            cfg.cpu_quota = cpu.quota;
            cfg.cpu_period = cpu.period;
            cfg.cpu_weight = cpu.shares.map(oci_shares_to_v2_weight);
        }
        if let Some(pids) = &resources.pids {
            cfg.pids_max = Some(pids.limit);
        }

        cfg
    }
}

/// Standard OCI conversion from cgroup v1 `cpu.shares` [2..262144] to cgroup v2 `cpu.weight` [1..10000].
pub fn oci_shares_to_v2_weight(shares: u64) -> u64 {
    let clamped = shares.clamp(2, 262_144);
    1 + ((clamped - 2) * 9_999) / 262_142
}

/// Live resource usage metrics read from Cgroups v2 files.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CgroupStats {
    pub memory_current_bytes: Option<u64>,
    pub pids_current: Option<u64>,
    pub cpu_usage_usec: Option<u64>,
}

/// Outcome of applying cgroup resource limits to a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CgroupApplyOutcome {
    /// Cgroup directory was created, limits written, and PID attached.
    Applied { path: PathBuf },
    /// Rootless / unprivileged host did not delegate a writable subtree; skipped safely.
    Skipped { reason: String },
}

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

        if let Some(rel) = detect_self_cgroup_v2_path() {
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

fn write_cgroup_file(path: &Path, content: &str) -> Result<(), CgroupError> {
    fs::write(path, content).map_err(|source| CgroupError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn write_if_controller_present(path: &Path, content: &str) -> Result<(), CgroupError> {
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

fn read_optional_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse::<u64>().ok()
}

fn parse_cpu_stat_usage_usec(content: &str) -> Option<u64> {
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

fn parse_proc_cgroup_v2(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(path) = line.strip_prefix("0::") {
            return Some(path.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use coral_oci::{LinuxCpu, LinuxMemory, LinuxPids};
    use tempfile::tempdir;

    #[test]
    fn converts_oci_shares_to_v2_weight() {
        assert_eq!(oci_shares_to_v2_weight(2), 1);
        assert_eq!(oci_shares_to_v2_weight(1024), 39);
        assert_eq!(oci_shares_to_v2_weight(262_144), 10_000);
    }

    #[test]
    fn applies_limits_reads_stats_and_cleans_up_in_mock_cgroupfs() {
        let dir = tempdir().unwrap();
        let mgr = CgroupManager::with_root(dir.path());

        let resources = LinuxResources {
            memory: Some(LinuxMemory {
                limit: Some(104_857_600),
                reservation: Some(67_108_864),
                swap: None,
            }),
            cpu: Some(LinuxCpu {
                shares: Some(1024),
                quota: Some(50_000),
                period: Some(100_000),
            }),
            pids: Some(LinuxPids { limit: 64 }),
        };
        let cfg = CgroupConfig::from_oci(&resources);

        let cg_path = mgr.apply("test1", 4242, &cfg).unwrap();
        assert_eq!(
            fs::read_to_string(cg_path.join("memory.max")).unwrap(),
            "104857600"
        );
        assert_eq!(
            fs::read_to_string(cg_path.join("memory.high")).unwrap(),
            "67108864"
        );
        assert_eq!(
            fs::read_to_string(cg_path.join("cpu.max")).unwrap(),
            "50000 100000"
        );
        assert_eq!(fs::read_to_string(cg_path.join("cpu.weight")).unwrap(), "39");
        assert_eq!(fs::read_to_string(cg_path.join("pids.max")).unwrap(), "64");
        assert_eq!(
            fs::read_to_string(cg_path.join("cgroup.procs")).unwrap(),
            "4242"
        );

        // Simulate kernel-populated stats files
        fs::write(cg_path.join("memory.current"), "8388608\n").unwrap();
        fs::write(cg_path.join("pids.current"), "3\n").unwrap();
        fs::write(
            cg_path.join("cpu.stat"),
            "usage_usec 123456\nuser_usec 100000\nsystem_usec 23456\n",
        )
        .unwrap();

        let stats = mgr.read_stats("test1").unwrap();
        assert_eq!(stats.memory_current_bytes, Some(8_388_608));
        assert_eq!(stats.pids_current, Some(3));
        assert_eq!(stats.cpu_usage_usec, Some(123_456));

        // Remove mock files and clean up directory
        for entry in fs::read_dir(&cg_path).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        mgr.cleanup("test1").unwrap();
        assert!(!cg_path.exists());
    }

    #[test]
    fn parses_proc_self_cgroup_v2_line() {
        let sample = "0::/user.slice/user-1000.slice/session-1.scope\n";
        assert_eq!(
            parse_proc_cgroup_v2(sample).as_deref(),
            Some("/user.slice/user-1000.slice/session-1.scope")
        );
    }
}
