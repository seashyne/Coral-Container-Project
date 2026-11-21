//! Boot Auto-Resume for `--restart always` / `--restart unless-stopped` containers.

use crate::service::port_forward::spawn_port_forwarders;
use crate::service::supervisor::{RestartPolicy, SupervisorLaunch, spawn_container_supervisor};
use crate::service::CoralDaemonService;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use tracing::{info, warn};

impl CoralDaemonService {
    /// Resumes containers configured with `--restart always` or `--restart unless-stopped`
    /// that were previously running (not stopped by user) before a daemon restart or reboot.
    pub(crate) fn spawn_boot_auto_resume(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let svc = self.clone();
        handle.spawn(async move {
            let candidates: Vec<(String, std::path::PathBuf, std::path::PathBuf, Option<String>, bool)> = {
                let mut store = svc.store.write().await;
                let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
                let mut list = Vec::new();
                for ctr in store.containers.values_mut() {
                    if ctr.stopped_by_user || ctr.started_at <= 0 {
                        continue;
                    }
                    if ctr.state == CriContainerState::ContainerRunning {
                        continue;
                    }
                    let policy = RestartPolicy::parse(&ctr.restart_policy);
                    if !matches!(policy, RestartPolicy::Always | RestartPolicy::UnlessStopped) {
                        continue;
                    }
                    ctr.state = CriContainerState::ContainerRunning;
                    ctr.started_at = now;
                    ctr.restart_count = ctr.restart_count.saturating_add(1);
                    ctr.reason = "BootAutoResume".into();
                    let net_mode = ctr
                        .config
                        .annotations
                        .get("coral.network")
                        .map(String::as_str)
                        .unwrap_or("host");
                    let is_bridge = !net_mode.eq_ignore_ascii_case("host")
                        && !net_mode.eq_ignore_ascii_case("none");
                    list.push((
                        ctr.id.clone(),
                        ctr.bundle_dir.clone(),
                        ctr.log_path.clone(),
                        ctr.config.annotations.get("coral.ports").cloned(),
                        is_bridge,
                    ));
                }
                if !list.is_empty() {
                    svc.persist_state(&store);
                }
                list
            };

            let runtime_root = svc.runtime_root();
            for (cid, bundle_dir, log_path, ports_spec, is_bridge) in candidates {
                if let Some(raw_ports) = ports_spec {
                    match spawn_port_forwarders(&cid, &raw_ports, is_bridge, &runtime_root).await {
                        Ok(handles) if !handles.is_empty() => {
                            svc.store
                                .write()
                                .await
                                .port_forwarders
                                .insert(cid.clone(), handles);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(container_id = %cid, err = %e, "boot auto-resume port forwarder skipped");
                        }
                    }
                }
                info!(container_id = %cid, "auto-resuming container after daemon boot");
                let sup_handle = spawn_container_supervisor(SupervisorLaunch {
                    cid: cid.clone(),
                    bundle_dir,
                    log_path,
                    runtime_root: runtime_root.clone(),
                    state_dir: svc.cfg.state_dir.clone(),
                    coral_bin: svc.cfg.coral_bin.clone(),
                    store: svc.store.clone(),
                    log_policy: svc.current_log_policy(),
                });
                if let Some(prev) = svc.store.write().await.supervisors.insert(cid, sup_handle) {
                    prev.abort();
                }
            }
        });
    }
}
