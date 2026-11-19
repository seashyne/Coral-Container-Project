//! Live container resource metrics collector (Cgroups v2 first, seamlessly supplemented by
//! `/proc/<pid>` process-tree accounting so partially-delegated or rootless Linux hosts
//! always expose accurate CPU, Memory, and PIDs telemetry).

use chrono::Utc;
use coral_cgroups::CgroupManager;
use coral_runtime::store::{StateStore, is_process_alive};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveContainerMetrics {
    pub pid: i32,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub cpu_usage_usec: u64,
    pub pids_current: u64,
    pub uptime_secs: u64,
    pub source: String,
}

/// Collects live CPU, Memory, and PIDs metrics for a running container.
pub fn collect_container_metrics(
    runtime_root: &Path,
    container_id: &str,
    memory_limit_bytes: i64,
    started_at_nanos: i64,
) -> Option<LiveContainerMetrics> {
    let state = StateStore::new(runtime_root).load(container_id).ok()?;
    let pid = state.pid?;
    if !is_process_alive(pid) {
        return None;
    }

    let now_nanos = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let uptime_secs = if started_at_nanos > 0 && now_nanos > started_at_nanos {
        ((now_nanos - started_at_nanos) / 1_000_000_000) as u64
    } else {
        0
    };
    let mem_limit = memory_limit_bytes.max(0) as u64;

    // 1. Query Cgroups v2 counters (may be partial or unavailable on rootless scopes).
    let cg = CgroupManager::detect();
    let cg_stats = cg.read_stats(container_id).ok();
    let cg_mem = cg_stats
        .as_ref()
        .and_then(|s| s.memory_current_bytes)
        .filter(|&v| v > 0);
    let cg_pids = cg_stats
        .as_ref()
        .and_then(|s| s.pids_current)
        .filter(|&v| v > 0);
    let cg_cpu = cg_stats
        .as_ref()
        .and_then(|s| s.cpu_usage_usec)
        .filter(|&v| v > 0);

    // Full Cgroups v2 accounting active.
    if let (Some(mem), Some(pids), Some(cpu)) = (cg_mem, cg_pids, cg_cpu) {
        return Some(LiveContainerMetrics {
            pid,
            memory_bytes: mem,
            memory_limit_bytes: mem_limit,
            cpu_usage_usec: cpu,
            pids_current: pids.max(1),
            uptime_secs,
            source: "cgroup-v2".into(),
        });
    }

    // 2. Supplement missing fields from `/proc/<pid>` (summing container process tree).
    let (proc_mem, proc_threads, proc_cpu) = read_proc_tree_metrics(pid);
    let source = if cg_mem.is_some() || cg_pids.is_some() || cg_cpu.is_some() {
        "cgroup-v2+procfs"
    } else {
        "procfs"
    };

    Some(LiveContainerMetrics {
        pid,
        memory_bytes: cg_mem.unwrap_or(proc_mem),
        memory_limit_bytes: mem_limit,
        cpu_usage_usec: cg_cpu.unwrap_or(proc_cpu),
        pids_current: cg_pids.unwrap_or(proc_threads).max(1),
        uptime_secs,
        source: source.into(),
    })
}

fn read_proc_tree_metrics(root_pid: i32) -> (u64, u64, u64) {
    let mut visited = HashSet::new();
    let mut stack = vec![root_pid];
    let mut total_mem = 0u64;
    let mut total_threads = 0u64;
    let mut total_cpu = 0u64;

    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) || visited.len() > 256 {
            continue;
        }
        let (rss, threads) = read_proc_status(pid);
        total_mem = total_mem.saturating_add(rss);
        total_threads = total_threads.saturating_add(threads);
        total_cpu = total_cpu.saturating_add(read_proc_cpu_usec(pid));

        if let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) {
            for entry in tasks.flatten() {
                if let Ok(children) = fs::read_to_string(entry.path().join("children")) {
                    for tok in children.split_whitespace() {
                        if let Ok(child_pid) = tok.parse::<i32>() {
                            stack.push(child_pid);
                        }
                    }
                }
            }
        }
    }

    (total_mem, total_threads.max(1), total_cpu)
}

fn read_proc_status(pid: i32) -> (u64, u64) {
    let Ok(raw) = fs::read_to_string(format!("/proc/{pid}/status")) else {
        return (0, 0);
    };
    let mut rss_bytes = 0u64;
    let mut threads = 1u64;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            if let Some(kb) = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok())
            {
                rss_bytes = kb.saturating_mul(1024);
            }
        } else if let Some(rest) = line.strip_prefix("Threads:")
            && let Ok(t) = rest.trim().parse::<u64>()
        {
            threads = t;
        }
    }
    (rss_bytes, threads)
}

fn read_proc_cpu_usec(pid: i32) -> u64 {
    let Ok(raw) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return 0;
    };
    let Some(after_comm) = raw.rsplit_once(')').map(|(_, rest)| rest) else {
        return 0;
    };
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // After `)`, field 0 is state (field 3 in /proc/pid/stat); utime is index 11, stime is index 12.
    let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    let ticks = utime.saturating_add(stime);
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if clk_tck > 0 { clk_tck as u64 } else { 100 };
    ticks.saturating_mul(1_000_000) / hz
}
