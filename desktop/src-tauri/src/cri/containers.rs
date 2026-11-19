//! Tauri CRI container commands (`list_containers`, `run_container`, `stop_container`,
//! `remove_container`, `container_detail`, `exec_container`).

use super::{AppState, container_state_name, err};
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerStatusRequest, CreateContainerRequest,
    ExecSyncRequest, ImageSpec, KeyValue, LinuxContainerConfig, LinuxContainerResources,
    ListContainersRequest, PodSandboxConfig, PodSandboxMetadata, RemoveContainerRequest,
    RunPodSandboxRequest, StartContainerRequest, StopContainerRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_ref: String,
    pub state: String,
    pub health: String,
    pub created_at: i64,
    pub pod_id: String,
    pub labels: HashMap<String, String>,
}

#[tauri::command]
pub async fn list_containers(state: State<'_, AppState>) -> Result<Vec<ContainerRow>, String> {
    let mut client = state.runtime().await?;
    let resp = client
        .list_containers(ListContainersRequest { filter: None })
        .await
        .map_err(err)?
        .into_inner();
    let mut rows: Vec<ContainerRow> = resp
        .containers
        .into_iter()
        .map(|c| {
            let health = c
                .labels
                .get("coral.health.status")
                .cloned()
                .unwrap_or_default();
            ContainerRow {
                name: c
                    .metadata
                    .as_ref()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| c.id.clone()),
                image: c
                    .image
                    .as_ref()
                    .map(|i| i.image.clone())
                    .unwrap_or_default(),
                image_ref: c.image_ref,
                state: container_state_name(c.state).into(),
                health,
                created_at: c.created_at,
                pod_id: c.pod_sandbox_id,
                labels: c.labels,
                id: c.id,
            }
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Ok(rows)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunParams {
    pub name: String,
    pub image: String,
    pub command: String,
    pub args: String,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub env: String,
    #[serde(default)]
    pub volumes: String,
    #[serde(default)]
    pub ports: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub restart_policy: String,
    #[serde(default)]
    pub health_cmd: String,
    #[serde(default)]
    pub health_interval_secs: u64,
}

#[tauri::command]
pub async fn run_container(
    state: State<'_, AppState>,
    params: RunParams,
) -> Result<String, String> {
    if params.name.trim().is_empty() {
        return Err("container name must not be empty".into());
    }
    let mut client = state.runtime().await?;

    let pod_id = client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: format!("{}-pod", params.name),
                    uid: params.name.clone(),
                    namespace: "coral-desktop".into(),
                    attempt: 0,
                }),
                hostname: params.name.clone(),
                log_directory: String::new(),
                labels: HashMap::from([("managed-by".into(), "coral-desktop".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .map_err(err)?
        .into_inner()
        .pod_sandbox_id;

    let command = shell_words::split(&params.command).map_err(err)?;
    let args = shell_words::split(&params.args).map_err(err)?;
    let envs = params
        .env
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            line.split_once('=').map(|(k, v)| KeyValue {
                key: k.trim().to_string(),
                value: v.trim().to_string(),
            })
        })
        .filter(|kv| !kv.key.is_empty())
        .collect();

    let mut annotations = HashMap::new();
    if !params.volumes.trim().is_empty() {
        annotations.insert("coral.mounts".into(), params.volumes.trim().into());
    }
    if !params.ports.trim().is_empty() {
        annotations.insert("coral.ports".into(), params.ports.trim().into());
    }
    if !params.network.trim().is_empty() {
        annotations.insert("coral.network".into(), params.network.trim().into());
    }
    if !params.restart_policy.trim().is_empty()
        && !params.restart_policy.trim().eq_ignore_ascii_case("no")
    {
        annotations.insert("coral.restart".into(), params.restart_policy.trim().into());
    }
    if !params.health_cmd.trim().is_empty() {
        annotations.insert("coral.health.cmd".into(), params.health_cmd.trim().into());
        let interval_ms = params
            .health_interval_secs
            .max(1)
            .saturating_mul(1_000);
        annotations.insert("coral.health.interval_ms".into(), interval_ms.to_string());
        annotations.insert("coral.health.retries".into(), "3".into());
    }

    let container_id = client
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id,
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: params.name.clone(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: params.image.clone(),
                    annotations: HashMap::new(),
                }),
                command,
                args,
                working_dir: "/".into(),
                envs,
                labels: HashMap::from([("managed-by".into(), "coral-desktop".into())]),
                annotations,
                log_path: String::new(),
                linux: Some(LinuxContainerConfig {
                    resources: Some(LinuxContainerResources {
                        cpu_period: 100_000,
                        cpu_quota: params.cpu_percent * 1_000,
                        cpu_shares: 1024,
                        memory_limit_in_bytes: params.memory_mb * 1024 * 1024,
                        pids_limit: params.pids_limit,
                    }),
                }),
            }),
            sandbox_config: None,
        })
        .await
        .map_err(err)?
        .into_inner()
        .container_id;

    client
        .start_container(StartContainerRequest {
            container_id: container_id.clone(),
        })
        .await
        .map_err(err)?;
    Ok(container_id)
}

#[tauri::command]
pub async fn stop_container(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .runtime()
        .await?
        .stop_container(StopContainerRequest {
            container_id: id,
            timeout: 10,
        })
        .await
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn remove_container(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .runtime()
        .await?
        .remove_container(RemoveContainerRequest { container_id: id })
        .await
        .map_err(err)?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMetrics {
    pub pid: i32,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub cpu_usage_usec: u64,
    pub pids_current: u64,
    pub uptime_secs: u64,
    pub source: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDetail {
    pub state: String,
    pub exit_code: i32,
    pub reason: String,
    pub message: String,
    pub log: String,
    pub overlay_diff: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub image: String,
    pub restart_policy: String,
    pub restart_count: u32,
    pub health_status: String,
    pub metrics: Option<ContainerMetrics>,
}

#[tauri::command]
pub async fn container_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<ContainerDetail, String> {
    let resp = state
        .runtime()
        .await?
        .container_status(ContainerStatusRequest {
            container_id: id,
            verbose: true,
        })
        .await
        .map_err(err)?
        .into_inner();
    let status = resp
        .status
        .ok_or_else(|| "daemon returned no status".to_string())?;
    let metrics = resp
        .info
        .get("metricsJson")
        .and_then(|j| serde_json::from_str::<ContainerMetrics>(j).ok());
    Ok(ContainerDetail {
        state: container_state_name(status.state).into(),
        exit_code: status.exit_code,
        reason: status.reason,
        message: status.message,
        log: resp.info.get("logOutput").cloned().unwrap_or_default(),
        overlay_diff: resp.info.get("overlayDiff").cloned().unwrap_or_default(),
        started_at: status.started_at,
        finished_at: status.finished_at,
        image: status.image.map(|i| i.image).unwrap_or_default(),
        restart_policy: resp
            .info
            .get("restartPolicy")
            .cloned()
            .unwrap_or_else(|| "no".into()),
        restart_count: resp
            .info
            .get("restartCount")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0),
        health_status: resp
            .info
            .get("healthStatus")
            .cloned()
            .unwrap_or_else(|| "none".into()),
        metrics,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[tauri::command]
pub async fn exec_container(
    state: State<'_, AppState>,
    id: String,
    cmd: Option<String>,
    argv: Option<Vec<String>>,
) -> Result<ExecResult, String> {
    let cmd_vec = match argv {
        Some(v) if !v.is_empty() => v,
        _ => {
            let raw = cmd.unwrap_or_default();
            shell_words::split(&raw).map_err(err)?
        }
    };
    if cmd_vec.is_empty() {
        return Err("exec command must not be empty".into());
    }
    let resp = state
        .runtime()
        .await?
        .exec_sync(ExecSyncRequest {
            container_id: id,
            cmd: cmd_vec,
            timeout: 30,
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&resp.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&resp.stderr).into_owned(),
        exit_code: resp.exit_code,
    })
}
