//! Persistent JSON state store (`<state_dir>/daemon-state.json`) for `coral-daemon`
//! sandboxes, containers, restart policies, and health status across daemon restarts.

use super::{DaemonStore, ManagedContainer, SandboxRecord};
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState as CriContainerState, ImageSpec, KeyValue,
    LinuxContainerConfig, LinuxContainerResources, PodSandboxConfig, PodSandboxMetadata,
    PodSandboxState,
};
use coral_runtime::store::{StateStore, is_process_alive};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

const STATE_FILE_NAME: &str = "daemon-state.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistedDaemonState {
    sandboxes: Vec<PersistedSandbox>,
    containers: Vec<PersistedContainer>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedSandbox {
    id: String,
    name: String,
    uid: String,
    namespace: String,
    hostname: String,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    state: i32,
    created_at: i64,
    #[serde(default)]
    pod_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedContainer {
    id: String,
    pod_sandbox_id: String,
    name: String,
    image: String,
    command: Vec<String>,
    args: Vec<String>,
    working_dir: String,
    envs: Vec<(String, String)>,
    labels: HashMap<String, String>,
    annotations: HashMap<String, String>,
    memory_limit_in_bytes: i64,
    cpu_quota: i64,
    cpu_period: i64,
    cpu_shares: i64,
    pids_limit: i64,
    bundle_dir: PathBuf,
    rootfs_path: PathBuf,
    log_path: PathBuf,
    state: i32,
    created_at: i64,
    started_at: i64,
    finished_at: i64,
    exit_code: i32,
    reason: String,
    message: String,
    #[serde(default)]
    restart_policy: String,
    #[serde(default)]
    restart_count: u32,
    #[serde(default)]
    stopped_by_user: bool,
    #[serde(default)]
    health_status: String,
    #[serde(default)]
    health_failing_streak: u32,
}

pub(crate) fn load_persisted_store(state_dir: &Path, runtime_root: &Path, store: &mut DaemonStore) {
    let path = state_dir.join(STATE_FILE_NAME);
    let Ok(raw) = fs::read_to_string(&path) else {
        return;
    };
    let Ok(persisted) = serde_json::from_str::<PersistedDaemonState>(&raw) else {
        warn!(path = %path.display(), "failed to parse persisted daemon-state.json; starting clean");
        return;
    };

    for sb in persisted.sandboxes {
        let state = PodSandboxState::try_from(sb.state).unwrap_or(PodSandboxState::SandboxNotready);
        store.sandboxes.insert(
            sb.id.clone(),
            SandboxRecord {
                id: sb.id,
                config: PodSandboxConfig {
                    metadata: Some(PodSandboxMetadata {
                        name: sb.name,
                        uid: sb.uid,
                        namespace: sb.namespace,
                        attempt: 0,
                    }),
                    hostname: sb.hostname,
                    log_directory: String::new(),
                    labels: sb.labels,
                    annotations: sb.annotations,
                },
                state,
                created_at: sb.created_at,
                pod_ip: sb.pod_ip,
                cni_raw_result: None,
            },
        );
    }

    let rt_store = StateStore::new(runtime_root);
    for ctr in persisted.containers {
        if !ctr.bundle_dir.is_dir() {
            continue;
        }
        let mut state =
            CriContainerState::try_from(ctr.state).unwrap_or(CriContainerState::ContainerExited);
        let mut reason = ctr.reason;
        if state == CriContainerState::ContainerRunning {
            let alive = rt_store
                .load(&ctr.id)
                .ok()
                .and_then(|st| st.pid)
                .is_some_and(is_process_alive);
            if !alive {
                state = CriContainerState::ContainerExited;
                if reason.is_empty() {
                    reason = "DaemonRestarted".into();
                }
            }
        }

        let linux = if ctr.memory_limit_in_bytes > 0
            || ctr.cpu_quota > 0
            || ctr.cpu_period > 0
            || ctr.cpu_shares > 0
            || ctr.pids_limit > 0
        {
            Some(LinuxContainerConfig {
                resources: Some(LinuxContainerResources {
                    cpu_period: ctr.cpu_period,
                    cpu_quota: ctr.cpu_quota,
                    cpu_shares: ctr.cpu_shares,
                    memory_limit_in_bytes: ctr.memory_limit_in_bytes,
                    pids_limit: ctr.pids_limit,
                }),
            })
        } else {
            None
        };

        let config = ContainerConfig {
            metadata: Some(ContainerMetadata {
                name: ctr.name,
                attempt: 0,
            }),
            image: Some(ImageSpec {
                image: ctr.image,
                annotations: HashMap::new(),
            }),
            command: ctr.command,
            args: ctr.args,
            working_dir: ctr.working_dir,
            envs: ctr
                .envs
                .into_iter()
                .map(|(key, value)| KeyValue { key, value })
                .collect(),
            labels: ctr.labels,
            annotations: ctr.annotations,
            log_path: String::new(),
            linux,
        };

        store.containers.insert(
            ctr.id.clone(),
            ManagedContainer {
                id: ctr.id,
                pod_sandbox_id: ctr.pod_sandbox_id,
                config,
                bundle_dir: ctr.bundle_dir,
                rootfs_path: ctr.rootfs_path,
                log_path: ctr.log_path,
                state,
                created_at: ctr.created_at,
                started_at: ctr.started_at,
                finished_at: ctr.finished_at,
                exit_code: ctr.exit_code,
                reason,
                message: ctr.message,
                restart_policy: if ctr.restart_policy.is_empty() {
                    "no".into()
                } else {
                    ctr.restart_policy
                },
                restart_count: ctr.restart_count,
                stopped_by_user: ctr.stopped_by_user,
                health_status: if ctr.health_status.is_empty() {
                    "none".into()
                } else {
                    ctr.health_status
                },
                health_failing_streak: ctr.health_failing_streak,
            },
        );
    }
}

pub(crate) fn save_persisted_store(state_dir: &Path, store: &DaemonStore) {
    if let Err(e) = fs::create_dir_all(state_dir) {
        warn!(err = %e, dir = %state_dir.display(), "create state_dir for daemon-state.json");
        return;
    }

    let sandboxes = store
        .sandboxes
        .values()
        .map(|sb| {
            let meta = sb.config.metadata.as_ref();
            PersistedSandbox {
                id: sb.id.clone(),
                name: meta.map(|m| m.name.clone()).unwrap_or_default(),
                uid: meta.map(|m| m.uid.clone()).unwrap_or_default(),
                namespace: meta.map(|m| m.namespace.clone()).unwrap_or_default(),
                hostname: sb.config.hostname.clone(),
                labels: sb.config.labels.clone(),
                annotations: sb.config.annotations.clone(),
                state: sb.state as i32,
                created_at: sb.created_at,
                pod_ip: sb.pod_ip.clone(),
            }
        })
        .collect();

    let containers = store
        .containers
        .values()
        .map(|c| {
            let res = c.config.linux.as_ref().and_then(|l| l.resources.as_ref());
            PersistedContainer {
                id: c.id.clone(),
                pod_sandbox_id: c.pod_sandbox_id.clone(),
                name: c
                    .config
                    .metadata
                    .as_ref()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| c.id.clone()),
                image: c
                    .config
                    .image
                    .as_ref()
                    .map(|i| i.image.clone())
                    .unwrap_or_default(),
                command: c.config.command.clone(),
                args: c.config.args.clone(),
                working_dir: c.config.working_dir.clone(),
                envs: c
                    .config
                    .envs
                    .iter()
                    .map(|kv| (kv.key.clone(), kv.value.clone()))
                    .collect(),
                labels: c.config.labels.clone(),
                annotations: c.config.annotations.clone(),
                memory_limit_in_bytes: res.map(|r| r.memory_limit_in_bytes).unwrap_or(0),
                cpu_quota: res.map(|r| r.cpu_quota).unwrap_or(0),
                cpu_period: res.map(|r| r.cpu_period).unwrap_or(0),
                cpu_shares: res.map(|r| r.cpu_shares).unwrap_or(0),
                pids_limit: res.map(|r| r.pids_limit).unwrap_or(0),
                bundle_dir: c.bundle_dir.clone(),
                rootfs_path: c.rootfs_path.clone(),
                log_path: c.log_path.clone(),
                state: c.state as i32,
                created_at: c.created_at,
                started_at: c.started_at,
                finished_at: c.finished_at,
                exit_code: c.exit_code,
                reason: c.reason.clone(),
                message: c.message.clone(),
                restart_policy: c.restart_policy.clone(),
                restart_count: c.restart_count,
                stopped_by_user: c.stopped_by_user,
                health_status: c.health_status.clone(),
                health_failing_streak: c.health_failing_streak,
            }
        })
        .collect();

    let payload = PersistedDaemonState {
        sandboxes,
        containers,
    };
    let tmp = state_dir.join(format!("{STATE_FILE_NAME}.tmp"));
    let target = state_dir.join(STATE_FILE_NAME);
    if let Ok(json) = serde_json::to_string_pretty(&payload)
        && fs::write(&tmp, format!("{json}\n")).is_ok()
    {
        let _ = fs::rename(&tmp, &target);
    }
}
