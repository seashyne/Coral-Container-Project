//! Supervisor spawn loop: streams container logs and enforces the restart policy.

use super::health::spawn_health_monitor;
use super::log_writer::LogRotationPolicy;
use super::policy::RestartPolicy;
use super::run::run_container_streaming_logs;
use crate::service::persist::save_persisted_store;
use crate::service::DaemonStore;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub(crate) struct SupervisorLaunch {
    pub cid: String,
    pub bundle_dir: PathBuf,
    pub log_path: PathBuf,
    pub runtime_root: PathBuf,
    pub state_dir: PathBuf,
    pub coral_bin: PathBuf,
    pub store: Arc<RwLock<DaemonStore>>,
    pub log_policy: LogRotationPolicy,
}

/// Spawns the asynchronous supervisor loop for a container: streams stdout/stderr live
/// into `log_path`, runs optional healthchecks, and enforces the container's restart policy.
pub(crate) fn spawn_container_supervisor(launch: SupervisorLaunch) -> JoinHandle<()> {
    tokio::spawn(async move {
        let SupervisorLaunch {
            cid,
            bundle_dir,
            log_path,
            runtime_root,
            state_dir,
            coral_bin,
            store,
            log_policy,
        } = launch;

        loop {
            // Prepare healthcheck monitor, rootless bridge egress & log policy overrides.
            let (health_cfg, backoff_base_ms, want_egress, effective_log_policy) = {
                let guard = store.read().await;
                let Some(ctr) = guard.containers.get(&cid) else {
                    break;
                };
                let hc = ctr
                    .config
                    .annotations
                    .get("coral.health.cmd")
                    .map(|cmd| {
                        let interval_ms = ctr
                            .config
                            .annotations
                            .get("coral.health.interval_ms")
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(5_000)
                            .max(100);
                        let retries = ctr
                            .config
                            .annotations
                            .get("coral.health.retries")
                            .and_then(|v| v.parse::<u32>().ok())
                            .unwrap_or(3)
                            .max(1);
                        (cmd.clone(), interval_ms, retries)
                    });
                let backoff = ctr
                    .config
                    .annotations
                    .get("coral.restart.backoff_ms")
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(200)
                    .clamp(25, 10_000);
                let net_mode = ctr
                    .config
                    .annotations
                    .get("coral.network")
                    .map(String::as_str)
                    .unwrap_or("host");
                let is_bridge = !net_mode.eq_ignore_ascii_case("host")
                    && !net_mode.eq_ignore_ascii_case("none");
                let mut ctr_log_policy = log_policy;
                if let Some(v) = ctr
                    .config
                    .annotations
                    .get("coral.log.max_size_bytes")
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    ctr_log_policy.max_size_bytes = v;
                }
                if let Some(v) = ctr
                    .config
                    .annotations
                    .get("coral.log.max_files")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    ctr_log_policy.max_files = v;
                }
                (hc, backoff, is_bridge, ctr_log_policy)
            };

            let egress_handle = if want_egress && coral_net::detect_egress_backend().is_some() {
                let rt_root = runtime_root.clone();
                let cid_clone = cid.clone();
                Some(tokio::spawn(async move {
                    let st_store = coral_runtime::store::StateStore::new(&rt_root);
                    for _ in 0..40 {
                        if let Ok(st) = st_store.load(&cid_clone)
                            && let Some(pid) = st.pid
                            && let Some(guard) =
                                coral_net::RootlessEgressGuard::attach_best_effort(pid)
                        {
                            info!(
                                container_id = %cid_clone,
                                pid,
                                backend = guard.backend.binary_name(),
                                "attached rootless bridge internet egress"
                            );
                            std::future::pending::<()>().await;
                            drop(guard);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }))
            } else {
                None
            };

            let health_handle = health_cfg.map(|(cmd, interval_ms, retries)| {
                spawn_health_monitor(
                    cid.clone(),
                    cmd,
                    interval_ms,
                    retries,
                    runtime_root.clone(),
                    state_dir.clone(),
                    coral_bin.clone(),
                    store.clone(),
                )
            });

            let run_result = run_container_streaming_logs(
                &coral_bin,
                &runtime_root,
                &bundle_dir,
                &cid,
                &log_path,
                effective_log_policy,
            )
            .await;

            if let Some(h) = egress_handle {
                h.abort();
            }
            if let Some(h) = health_handle {
                h.abort();
            }

            let finished_at = Utc::now().timestamp_nanos_opt().unwrap_or(0);
            let mut should_restart_after_ms: Option<u64> = None;

            {
                let mut guard = store.write().await;
                if let Some(ctr) = guard.containers.get_mut(&cid) {
                    let (exit_code, stderr_tail, start_err) = match run_result {
                        Ok((code, err_str)) => (code, err_str, None),
                        Err(e) => (127, String::new(), Some(e.to_string())),
                    };

                    ctr.exit_code = exit_code;
                    ctr.finished_at = finished_at;
                    if let Some(se) = start_err {
                        warn!(container_id = %cid, err = %se, "failed to launch coral binary");
                        ctr.reason = "StartError".into();
                        ctr.message = se;
                    } else {
                        ctr.reason = if exit_code == 0 {
                            "Completed".into()
                        } else {
                            "Error".into()
                        };
                        if exit_code != 0 {
                            ctr.message = stderr_tail;
                        }
                    }

                    let policy = RestartPolicy::parse(&ctr.restart_policy);
                    if policy.should_restart(exit_code, ctr.restart_count, ctr.stopped_by_user) {
                        ctr.restart_count += 1;
                        ctr.state = CriContainerState::ContainerRunning;
                        ctr.started_at = Utc::now().timestamp_nanos_opt().unwrap_or(0);
                        ctr.reason = format!("Restarting ({})", ctr.restart_count);
                        let shift = (ctr.restart_count.saturating_sub(1)).min(4);
                        let delay = (backoff_base_ms.saturating_mul(1u64 << shift)).min(5_000);
                        should_restart_after_ms = Some(delay);
                        info!(
                            container_id = %cid,
                            restart_count = ctr.restart_count,
                            policy = %ctr.restart_policy,
                            delay_ms = delay,
                            "auto-restarting container"
                        );
                    } else {
                        ctr.state = CriContainerState::ContainerExited;
                        if let Some(handles) = guard.port_forwarders.remove(&cid) {
                            for h in handles {
                                h.abort();
                            }
                        }
                    }
                    save_persisted_store(&state_dir, &guard);
                } else {
                    if let Some(handles) = guard.port_forwarders.remove(&cid) {
                        for h in handles {
                            h.abort();
                        }
                    }
                    break;
                }
            }

            if let Some(delay_ms) = should_restart_after_ms {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                // Re-check if user stopped or removed the container during backoff.
                let still_wanted = {
                    let guard = store.read().await;
                    guard
                        .containers
                        .get(&cid)
                        .is_some_and(|c| !c.stopped_by_user && c.state == CriContainerState::ContainerRunning)
                };
                if !still_wanted {
                    break;
                }
                continue;
            }
            break;
        }
    })
}
