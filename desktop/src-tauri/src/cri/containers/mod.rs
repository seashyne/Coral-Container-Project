//! Tauri CRI container commands (`list_containers`, `run_container`, `stop_container`,
//! `remove_container`, `container_detail`, `exec_container`).

mod run;

pub use run::*;

use super::{AppState, container_state_name, err};
use coral_api::{
    ContainerStatusRequest, ExecSyncRequest, ListContainersRequest, PodSandboxConfig,
    RemoveContainerRequest, RemovePodSandboxRequest, RunPodSandboxRequest, StartContainerRequest,
    StopContainerRequest, StopPodSandboxRequest,
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
    pub paused: bool,
    pub ip: String,
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
            let paused = c
                .labels
                .get("coral.paused")
                .is_some_and(|v| v == "true" || v == "1");
            let ip = c
                .labels
                .get("coral.network.ip")
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
                paused,
                ip,
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

#[tauri::command]
pub async fn start_container(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .runtime()
        .await?
        .start_container(StartContainerRequest { container_id: id })
        .await
        .map_err(err)?;
    Ok(())
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
    let mut client = state.runtime().await?;
    let existing = client
        .list_containers(ListContainersRequest { filter: None })
        .await
        .ok()
        .map(|r| r.into_inner().containers)
        .unwrap_or_default();

    let target_pod = existing
        .iter()
        .find(|c| c.id == id)
        .filter(|c| {
            c.labels
                .get("managed-by")
                .is_some_and(|v| v == "coral-desktop")
        })
        .map(|c| c.pod_sandbox_id.clone());

    client
        .remove_container(RemoveContainerRequest {
            container_id: id.clone(),
        })
        .await
        .map_err(err)?;

    if let Some(pod_id) = target_pod {
        let still_used = existing
            .iter()
            .any(|c| c.id != id && c.pod_sandbox_id == pod_id);
        if !still_used {
            let _ = client
                .stop_pod_sandbox(StopPodSandboxRequest {
                    pod_sandbox_id: pod_id.clone(),
                })
                .await;
            let _ = client
                .remove_pod_sandbox(RemovePodSandboxRequest {
                    pod_sandbox_id: pod_id,
                })
                .await;
        }
    }
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

async fn run_container_system_action(
    state: &State<'_, AppState>,
    action: &str,
    mut annotations: HashMap<String, String>,
) -> Result<String, String> {
    annotations.insert("coral.system.action".into(), action.to_string());
    let resp = state
        .runtime()
        .await?
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations,
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.pod_sandbox_id)
}

#[tauri::command]
pub async fn pause_container(state: State<'_, AppState>, id: String) -> Result<String, String> {
    run_container_system_action(
        &state,
        "container_pause",
        HashMap::from([("coral.container.id".into(), id)]),
    )
    .await
}

#[tauri::command]
pub async fn unpause_container(state: State<'_, AppState>, id: String) -> Result<String, String> {
    run_container_system_action(
        &state,
        "container_unpause",
        HashMap::from([("coral.container.id".into(), id)]),
    )
    .await
}

#[tauri::command]
pub async fn rename_container(
    state: State<'_, AppState>,
    id: String,
    new_name: String,
) -> Result<String, String> {
    run_container_system_action(
        &state,
        "container_rename",
        HashMap::from([
            ("coral.container.id".into(), id),
            ("coral.container.new_name".into(), new_name),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn update_container(
    state: State<'_, AppState>,
    id: String,
    memory_mb: Option<i64>,
    cpu_percent: Option<i64>,
    pids_limit: Option<i64>,
) -> Result<String, String> {
    let mut map = HashMap::from([("coral.container.id".into(), id)]);
    if let Some(mb) = memory_mb
        && mb > 0
    {
        map.insert(
            "coral.update.memory_bytes".into(),
            (mb * 1024 * 1024).to_string(),
        );
    }
    if let Some(cpu) = cpu_percent
        && cpu > 0
    {
        map.insert("coral.update.cpu_quota".into(), (cpu * 1_000).to_string());
        map.insert("coral.update.cpu_period".into(), "100000".into());
    }
    if let Some(pids) = pids_limit
        && pids > 0
    {
        map.insert("coral.update.pids_limit".into(), pids.to_string());
    }
    run_container_system_action(&state, "container_update", map).await
}

#[tauri::command]
pub async fn container_top(
    state: State<'_, AppState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let raw = run_container_system_action(
        &state,
        "container_top",
        HashMap::from([("coral.container.id".into(), id)]),
    )
    .await?;
    serde_json::from_str(&raw).map_err(err)
}
