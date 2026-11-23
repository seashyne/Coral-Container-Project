//! Live container lifecycle controls: `pause` (Cgroups v2 `cgroup.freeze` / `SIGSTOP`),
//! `unpause`, `rename`, and live resource `update` (`memory`, `cpus`, `pids`).

#![allow(clippy::result_large_err)]

use super::CoralDaemonService;
use coral_api::{ContainerMetadata, ContainerState as CriContainerState, LinuxContainerConfig, LinuxContainerResources};
use coral_cgroups::{CgroupConfig, CgroupManager};
use coral_runtime::store::StateStore;
use tonic::Status;

impl CoralDaemonService {
    /// Freezes all processes inside a running container (`cgroup.freeze = 1`, or `SIGSTOP` fallback).
    pub(crate) async fn pause_container(&self, query: &str) -> Result<String, Status> {
        let id = {
            let store = self.store.read().await;
            let id = store
                .resolve_container_id(query)
                .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
            let ctr = store
                .containers
                .get(&id)
                .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
            if ctr.state != CriContainerState::ContainerRunning {
                return Err(Status::failed_precondition(format!(
                    "container '{id}' is not running"
                )));
            }
            id
        };

        let cg = CgroupManager::detect();
        let frozen_via_cg = cg.freeze(&id).unwrap_or(false);
        if !frozen_via_cg
            && let Ok(st) = StateStore::new(self.runtime_root()).load(&id)
            && let Some(pid) = st.pid
        {
            unsafe {
                libc::kill(pid, libc::SIGSTOP);
            }
        }

        let mut store = self.store.write().await;
        if let Some(ctr) = store.containers.get_mut(&id) {
            ctr.config.labels.insert("coral.paused".into(), "true".into());
            ctr.reason = "Paused".into();
        }
        self.persist_state(&store);
        Ok(id)
    }

    /// Unfreezes a paused container (`cgroup.freeze = 0`, or `SIGCONT` fallback).
    pub(crate) async fn unpause_container(&self, query: &str) -> Result<String, Status> {
        let id = {
            let store = self.store.read().await;
            store
                .resolve_container_id(query)
                .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?
        };

        let cg = CgroupManager::detect();
        let unfrozen_via_cg = cg.unfreeze(&id).unwrap_or(false);
        if !unfrozen_via_cg
            && let Ok(st) = StateStore::new(self.runtime_root()).load(&id)
            && let Some(pid) = st.pid
        {
            unsafe {
                libc::kill(pid, libc::SIGCONT);
            }
        }

        let mut store = self.store.write().await;
        if let Some(ctr) = store.containers.get_mut(&id) {
            ctr.config.labels.remove("coral.paused");
            if ctr.reason == "Paused" {
                ctr.reason.clear();
            }
        }
        self.persist_state(&store);
        Ok(id)
    }

    /// Renames an existing container and refreshes its embedded DNS entry.
    pub(crate) async fn rename_container(
        &self,
        query: &str,
        new_name: &str,
    ) -> Result<String, Status> {
        let trimmed = new_name.trim().trim_start_matches('/');
        if trimmed.is_empty()
            || !trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(Status::invalid_argument(format!(
                "invalid container name '{new_name}'"
            )));
        }

        let mut store = self.store.write().await;
        let id = store
            .resolve_container_id(query)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        let conflict = store.containers.values().any(|c| {
            c.id != id
                && c.config
                    .metadata
                    .as_ref()
                    .is_some_and(|m| m.name == trimmed)
        });
        if conflict {
            return Err(Status::already_exists(format!(
                "container name '{trimmed}' is already in use"
            )));
        }

        let (old_name, target_ip) = {
            let ctr = store
                .containers
                .get_mut(&id)
                .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
            let old = ctr
                .config
                .metadata
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_default();
            ctr.config.metadata = Some(ContainerMetadata {
                name: trimmed.to_string(),
                attempt: ctr.config.metadata.as_ref().map(|m| m.attempt).unwrap_or(0),
            });
            let ip = ctr
                .config
                .labels
                .get("coral.network.ip")
                .and_then(|s| s.parse::<std::net::Ipv4Addr>().ok())
                .unwrap_or(std::net::Ipv4Addr::LOCALHOST);
            (old, ip)
        };

        if let Some(dns) = &self.dns_server {
            if !old_name.is_empty() {
                dns.unregister_host(&old_name);
            }
            dns.register_host(trimmed, target_ip);
        }
        self.persist_state(&store);
        Ok(id)
    }

    /// Updates memory, CPU quota/period, and PIDs limits live on a container and its Cgroup v2.
    pub(crate) async fn update_container_resources(
        &self,
        query: &str,
        memory_bytes: Option<i64>,
        cpu_quota: Option<i64>,
        cpu_period: Option<i64>,
        pids_limit: Option<i64>,
    ) -> Result<String, Status> {
        let mut store = self.store.write().await;
        let id = store
            .resolve_container_id(query)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        let ctr = store
            .containers
            .get_mut(&id)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;

        let linux = ctr
            .config
            .linux
            .get_or_insert_with(LinuxContainerConfig::default);
        let res = linux
            .resources
            .get_or_insert_with(LinuxContainerResources::default);
        if let Some(m) = memory_bytes
            && m > 0
        {
            res.memory_limit_in_bytes = m;
        }
        if let Some(q) = cpu_quota
            && q > 0
        {
            res.cpu_quota = q;
        }
        if let Some(p) = cpu_period
            && p > 0
        {
            res.cpu_period = p;
        }
        if let Some(pids) = pids_limit
            && pids > 0
        {
            res.pids_limit = pids;
        }

        let cg_cfg = CgroupConfig {
            memory_max: memory_bytes.filter(|&v| v > 0),
            memory_high: None,
            cpu_quota: cpu_quota.filter(|&v| v > 0),
            cpu_period: cpu_period.filter(|&v| v > 0).map(|v| v as u64),
            cpu_weight: None,
            pids_max: pids_limit.filter(|&v| v > 0),
        };
        let _ = CgroupManager::detect().update_limits(&id, &cg_cfg);
        self.persist_state(&store);
        Ok(id)
    }

    /// Lists live processes running inside a container's PID namespace (`coral top` / `/containers/{id}/top`).
    pub(crate) async fn container_top(&self, query: &str) -> Result<String, Status> {
        let id = {
            let store = self.store.read().await;
            store
                .resolve_container_id(query)
                .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?
        };
        let state = StateStore::new(self.runtime_root())
            .load(&id)
            .map_err(|_| Status::failed_precondition(format!("container '{id}' is not running")))?;
        let Some(root_pid) = state.pid else {
            return Err(Status::failed_precondition(format!(
                "container '{id}' has no active PID"
            )));
        };
        let procs = collect_pidns_processes(root_pid);
        serde_json::to_string(&procs)
            .map_err(|e| Status::internal(format!("serialize container top: {e}")))
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TopProcEntry {
    pid: i32,
    ppid: i32,
    state: String,
    rss_kb: u64,
    cmd: String,
}

fn collect_pidns_processes(root_pid: i32) -> Vec<TopProcEntry> {
    let target_ns = std::fs::read_link(format!("/proc/{root_pid}/ns/pid")).ok();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let matches = if let Some(ref ns) = target_ns {
            std::fs::read_link(format!("/proc/{pid}/ns/pid")).ok().as_ref() == Some(ns)
        } else {
            pid == root_pid
        };
        if !matches {
            continue;
        }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let mut ppid = 0i32;
        let mut proc_state = String::from("?");
        let mut rss_kb = 0u64;
        for line in status.lines() {
            if let Some(v) = line.strip_prefix("PPid:") {
                ppid = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("State:") {
                proc_state = v.trim().to_string();
            } else if let Some(v) = line.strip_prefix("VmRSS:") {
                rss_kb = v
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
            }
        }
        let cmdline_raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let cmd = if !cmdline_raw.is_empty() {
            cmdline_raw
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| "<unknown>".into())
        };
        out.push(TopProcEntry {
            pid,
            ppid,
            state: proc_state,
            rss_kb,
            cmd,
        });
    }
    out.sort_by_key(|p| p.pid);
    out
}
