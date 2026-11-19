//! Tauri CRI pod sandbox commands (`list_pods`, `stop_pod`, `remove_pod`).

use super::{AppState, err};
use coral_api::{
    ListPodSandboxRequest, PodSandboxState, RemovePodSandboxRequest, StopPodSandboxRequest,
};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PodRow {
    pub id: String,
    pub name: String,
    pub state: String,
    pub created_at: i64,
    pub labels: HashMap<String, String>,
}

#[tauri::command]
pub async fn list_pods(state: State<'_, AppState>) -> Result<Vec<PodRow>, String> {
    let resp = state
        .runtime()
        .await?
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .map_err(err)?
        .into_inner();
    let mut rows: Vec<PodRow> = resp
        .items
        .into_iter()
        .map(|p| PodRow {
            name: p
                .metadata
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| p.id.clone()),
            state: match PodSandboxState::try_from(p.state) {
                Ok(PodSandboxState::SandboxReady) => "ready".into(),
                _ => "notready".into(),
            },
            created_at: p.created_at,
            labels: p.labels,
            id: p.id,
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.created_at));
    Ok(rows)
}

#[tauri::command]
pub async fn stop_pod(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .runtime()
        .await?
        .stop_pod_sandbox(StopPodSandboxRequest { pod_sandbox_id: id })
        .await
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn remove_pod(state: State<'_, AppState>, id: String) -> Result<(), String> {
    state
        .runtime()
        .await?
        .remove_pod_sandbox(RemovePodSandboxRequest { pod_sandbox_id: id })
        .await
        .map_err(err)?;
    Ok(())
}
