//! Container execution supervisor: live log streaming to disk while running,
//! Auto-Restart Policies (`always`, `unless-stopped`, `on-failure[:N]`), and
//! background Healthcheck probes (`coral.health.cmd`).

use super::persist::save_persisted_store;
use super::DaemonStore;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Parsed Docker-compatible restart policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestartPolicy {
    No,
    Always,
    UnlessStopped,
    OnFailure(Option<u32>),
}

impl RestartPolicy {
    pub fn parse(raw: &str) -> Self {
        let lower = raw.trim().to_ascii_lowercase();
        if lower == "always" {
            Self::Always
        } else if lower == "unless-stopped" {
            Self::UnlessStopped
        } else if let Some(rest) = lower.strip_prefix("on-failure") {
            let max = rest
                .strip_prefix(':')
                .and_then(|n| n.trim().parse::<u32>().ok())
                .filter(|&n| n > 0);
            Self::OnFailure(max)
        } else {
            Self::No
        }
    }

    pub fn should_restart(&self, exit_code: i32, restart_count: u32, stopped_by_user: bool) -> bool {
        if stopped_by_user {
            return false;
        }
        match self {
            Self::No => false,
            Self::Always | Self::UnlessStopped => true,
            Self::OnFailure(None) => exit_code != 0,
            Self::OnFailure(Some(max)) => exit_code != 0 && restart_count < *max,
        }
    }
}

pub(crate) struct SupervisorLaunch {
    pub cid: String,
    pub bundle_dir: PathBuf,
    pub log_path: PathBuf,
    pub runtime_root: PathBuf,
    pub state_dir: PathBuf,
    pub coral_bin: PathBuf,
    pub store: Arc<RwLock<DaemonStore>>,
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
        } = launch;

        loop {
            // Prepare healthcheck monitor & rootless bridge egress if configured.
            let (health_cfg, backoff_base_ms, want_egress) = {
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
                (hc, backoff, is_bridge)
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

            let run_result =
                run_container_streaming_logs(&coral_bin, &runtime_root, &bundle_dir, &cid, &log_path)
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

async fn run_container_streaming_logs(
    coral_bin: &PathBuf,
    runtime_root: &PathBuf,
    bundle_dir: &PathBuf,
    cid: &str,
    log_path: &PathBuf,
) -> std::io::Result<(i32, String)> {
    // Clean up any stale OCI state left behind after an unclean host reboot or SIGKILL crash.
    if runtime_root.join(cid).exists() {
        let _ = Command::new(coral_bin)
            .arg("--root")
            .arg(runtime_root)
            .arg("delete")
            .arg("--force")
            .arg(cid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let log_writer = Arc::new(StdMutex::new(log_file));

    let mut child = Command::new(coral_bin)
        .arg("--root")
        .arg(runtime_root)
        .arg("run")
        .arg("--bundle")
        .arg(bundle_dir)
        .arg(cid)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let w_out = log_writer.clone();
    let out_task = tokio::spawn(async move {
        if let Some(mut reader) = stdout {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut f) = w_out.lock() {
                    let _ = f.write_all(&buf[..n]);
                    let _ = f.flush();
                }
            }
        }
    });

    let w_err = log_writer;
    let err_task = tokio::spawn(async move {
        let mut tail = Vec::new();
        if let Some(mut reader) = stderr {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut f) = w_err.lock() {
                    let _ = f.write_all(&buf[..n]);
                    let _ = f.flush();
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 2048 {
                    let drop_n = tail.len() - 2048;
                    tail.drain(..drop_n);
                }
            }
        }
        String::from_utf8_lossy(&tail).trim().to_string()
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let stderr_tail = err_task.await.unwrap_or_default();
    Ok((status.code().unwrap_or(1), stderr_tail))
}

#[allow(clippy::too_many_arguments)]
fn spawn_health_monitor(
    cid: String,
    health_cmd: String,
    interval_ms: u64,
    retries: u32,
    runtime_root: PathBuf,
    state_dir: PathBuf,
    coral_bin: PathBuf,
    store: Arc<RwLock<DaemonStore>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        {
            let mut guard = store.write().await;
            if let Some(ctr) = guard.containers.get_mut(&cid) {
                ctr.health_status = "starting".into();
                ctr.health_failing_streak = 0;
                ctr.config
                    .labels
                    .insert("coral.health.status".into(), "starting".into());
                save_persisted_store(&state_dir, &guard);
            }
        }

        // Wait briefly for container init to enter running state before first probe.
        tokio::time::sleep(Duration::from_millis((interval_ms / 2).clamp(100, 1000))).await;

        loop {
            let probe = Command::new(&coral_bin)
                .arg("--root")
                .arg(&runtime_root)
                .arg("exec")
                .arg("--local")
                .arg(&cid)
                .arg("--")
                .arg("/bin/sh")
                .arg("-c")
                .arg(&health_cmd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .await;

            {
                let mut guard = store.write().await;
                let Some(ctr) = guard.containers.get_mut(&cid) else {
                    break;
                };
                if ctr.state != CriContainerState::ContainerRunning {
                    break;
                }
                if probe.is_ok_and(|s| s.success()) {
                    ctr.health_failing_streak = 0;
                    ctr.health_status = "healthy".into();
                } else {
                    ctr.health_failing_streak = ctr.health_failing_streak.saturating_add(1);
                    if ctr.health_failing_streak >= retries {
                        ctr.health_status = "unhealthy".into();
                    }
                }
                ctr.config
                    .labels
                    .insert("coral.health.status".into(), ctr.health_status.clone());
                save_persisted_store(&state_dir, &guard);
            }

            tokio::time::sleep(Duration::from_millis(interval_ms)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_restart_policies_correctly() {
        assert!(!RestartPolicy::parse("no").should_restart(1, 0, false));
        assert!(RestartPolicy::parse("always").should_restart(0, 5, false));
        assert!(!RestartPolicy::parse("always").should_restart(0, 0, true));
        assert!(RestartPolicy::parse("unless-stopped").should_restart(0, 1, false));
        assert!(!RestartPolicy::parse("unless-stopped").should_restart(0, 1, true));
        assert!(!RestartPolicy::parse("on-failure:3").should_restart(0, 0, false));
        assert!(RestartPolicy::parse("on-failure:3").should_restart(1, 2, false));
        assert!(!RestartPolicy::parse("on-failure:3").should_restart(1, 3, false));
    }
}
