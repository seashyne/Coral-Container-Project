//! Linux Cgroups v2 resource management (Memory, CPU, PIDs) and live observability stats.

mod fs_io;
mod manager;
#[cfg(test)]
mod tests;

use coral_oci::LinuxResources;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use thiserror::Error;

pub use fs_io::detect_self_cgroup_v2_path;
pub use manager::CgroupManager;

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
