//! Tauri CRI Compose stack commands (`compose_up`, `compose_down`) delegating YAML stack
//! orchestration to `coral-daemon` so Local, WSL2, and Remote hosts work uniformly.

use super::{AppState, err};
use coral_api::{
    ContainerFilter, ListContainersRequest, ListPodSandboxRequest, PodSandboxConfig,
    PodSandboxFilter, PodSandboxMetadata, RemoveContainerRequest, RemovePodSandboxRequest,
    RunPodSandboxRequest, StopContainerRequest, StopPodSandboxRequest,
};
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub async fn compose_up(
    state: State<'_, AppState>,
    yaml: String,
    project_name: Option<String>,
    base_dir: Option<String>,
) -> Result<String, String> {
    if yaml.trim().is_empty() {
        return Err("Compose YAML must not be empty".into());
    }
    let mut annotations = HashMap::new();
    annotations.insert("coral.compose.yaml".into(), yaml);
    if let Some(proj) = project_name
        && !proj.trim().is_empty()
    {
        annotations.insert("coral.compose.project".into(), proj.trim().into());
    }
    if let Some(dir) = base_dir
        && !dir.trim().is_empty()
    {
        annotations.insert("coral.compose.base_dir".into(), dir.trim().into());
    }

    let resp = state
        .runtime()
        .await?
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-compose-launcher".into(),
                    uid: "coral-compose".into(),
                    namespace: "coral-desktop".into(),
                    attempt: 0,
                }),
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
pub async fn compose_down(
    state: State<'_, AppState>,
    project_name: String,
) -> Result<usize, String> {
    let project = project_name.trim().to_string();
    if project.is_empty() {
        return Err("Compose project name must not be empty".into());
    }
    let mut rt = state.runtime().await?;
    let containers = rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                id: String::new(),
                pod_sandbox_id: String::new(),
                label_selector: HashMap::from([("coral.compose.project".into(), project.clone())]),
            }),
        })
        .await
        .map_err(err)?
        .into_inner()
        .containers;

    let count = containers.len();
    for c in containers {
        let _ = rt
            .stop_container(StopContainerRequest {
                container_id: c.id.clone(),
                timeout: 10,
            })
            .await;
        rt.remove_container(RemoveContainerRequest { container_id: c.id })
            .await
            .map_err(err)?;
    }

    let pods = rt
        .list_pod_sandbox(ListPodSandboxRequest {
            filter: Some(PodSandboxFilter {
                id: String::new(),
                label_selector: HashMap::from([("coral.compose.project".into(), project)]),
            }),
        })
        .await
        .map_err(err)?
        .into_inner()
        .items;

    for p in pods {
        let _ = rt
            .stop_pod_sandbox(StopPodSandboxRequest {
                pod_sandbox_id: p.id.clone(),
            })
            .await;
        let _ = rt
            .remove_pod_sandbox(RemovePodSandboxRequest {
                pod_sandbox_id: p.id,
            })
            .await;
    }

    Ok(count)
}

#[tauri::command]
pub async fn read_compose_file(path: String) -> Result<String, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("compose file path must not be empty".into());
    }
    let meta = tokio::fs::metadata(trimmed)
        .await
        .map_err(|e| format!("stat '{trimmed}': {e}"))?;
    if meta.len() > 1024 * 1024 {
        return Err("compose file exceeds 1 MiB limit".into());
    }
    tokio::fs::read_to_string(trimmed)
        .await
        .map_err(|e| format!("read '{trimmed}': {e}"))
}
