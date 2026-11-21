//! Background healthcheck probe loop for supervised containers.

use crate::service::persist::save_persisted_store;
use crate::service::DaemonStore;
use coral_api::ContainerState as CriContainerState;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_health_monitor(
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
