//! Tauri CRI system housekeeping & Named Volume commands (`system_df`, `system_prune`,
//! `list_volumes`, `create_volume`, `remove_volume`).

use super::{AppState, err};
use coral_api::{PodSandboxConfig, PodSandboxMetadata, RunPodSandboxRequest};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemDiskUsage {
    pub images_count: usize,
    pub images_bytes: u64,
    pub cache_entries: usize,
    pub cache_bytes: u64,
    pub containers_count: usize,
    pub containers_running: usize,
    pub containers_exited: usize,
    pub containers_bytes: u64,
    pub volumes_count: usize,
    pub volumes_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PruneReport {
    pub containers_removed: usize,
    pub pods_removed: usize,
    pub cache_entries_removed: usize,
    pub volumes_removed: usize,
    pub images_removed: usize,
    pub bytes_reclaimed: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VolumeRow {
    pub name: String,
    pub mountpoint: String,
    pub size: u64,
    pub used_by: Vec<String>,
}

async fn call_system_action(
    state: &State<'_, AppState>,
    annotations: HashMap<String, String>,
) -> Result<String, String> {
    let mut client = state.runtime().await?;
    let resp = client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-desktop-system".into(),
                    uid: "coral-desktop-system".into(),
                    namespace: "coral-system".into(),
                    attempt: 0,
                }),
                hostname: "coral-system".into(),
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
pub async fn system_df(state: State<'_, AppState>) -> Result<SystemDiskUsage, String> {
    let json = call_system_action(
        &state,
        HashMap::from([("coral.system.action".into(), "df".into())]),
    )
    .await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn system_prune(
    state: State<'_, AppState>,
    all: Option<bool>,
    volumes: Option<bool>,
) -> Result<PruneReport, String> {
    let mut ann = HashMap::from([("coral.system.action".into(), "prune".into())]);
    if all.unwrap_or(false) {
        ann.insert("coral.system.prune_all".into(), "true".into());
    }
    if volumes.unwrap_or(false) {
        ann.insert("coral.system.prune_volumes".into(), "true".into());
    }
    let json = call_system_action(&state, ann).await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn list_volumes(state: State<'_, AppState>) -> Result<Vec<VolumeRow>, String> {
    let json = call_system_action(
        &state,
        HashMap::from([("coral.system.action".into(), "volume_ls".into())]),
    )
    .await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn create_volume(
    state: State<'_, AppState>,
    name: String,
) -> Result<VolumeRow, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("volume name must not be empty".into());
    }
    let json = call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "volume_create".into()),
            ("coral.volume.name".into(), trimmed.to_string()),
        ]),
    )
    .await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn remove_volume(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("volume name must not be empty".into());
    }
    call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "volume_rm".into()),
            ("coral.volume.name".into(), trimmed.to_string()),
        ]),
    )
    .await?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PushResultRow {
    pub image: String,
    pub registry: String,
    pub repository: String,
    pub reference: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,
}

#[tauri::command]
pub async fn registry_login(
    state: State<'_, AppState>,
    registry: String,
    username: String,
    password: String,
) -> Result<String, String> {
    call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "login".into()),
            ("coral.auth.registry".into(), registry),
            ("coral.auth.username".into(), username),
            ("coral.auth.password".into(), password),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn registry_logout(
    state: State<'_, AppState>,
    registry: String,
) -> Result<String, String> {
    call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "logout".into()),
            ("coral.auth.registry".into(), registry),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn push_image(
    state: State<'_, AppState>,
    image: String,
) -> Result<PushResultRow, String> {
    let trimmed = image.trim();
    if trimmed.is_empty() {
        return Err("image reference must not be empty".into());
    }
    let json = call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "push".into()),
            ("coral.image.ref".into(), trimmed.to_string()),
        ]),
    )
    .await?;
    serde_json::from_str(&json).map_err(err)
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRow {
    pub name: String,
    pub driver: String,
    pub subnet: String,
    pub gateway: String,
    pub builtin: bool,
    pub created_at: i64,
}

#[tauri::command]
pub async fn list_networks(state: State<'_, AppState>) -> Result<Vec<NetworkRow>, String> {
    let json = call_system_action(
        &state,
        HashMap::from([("coral.system.action".into(), "network_ls".into())]),
    )
    .await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn create_network(
    state: State<'_, AppState>,
    name: String,
    subnet: Option<String>,
) -> Result<NetworkRow, String> {
    let mut ann = HashMap::from([
        ("coral.system.action".into(), "network_create".into()),
        ("coral.network.name".into(), name),
    ]);
    if let Some(s) = subnet
        && !s.trim().is_empty()
    {
        ann.insert("coral.network.subnet".into(), s.trim().to_string());
    }
    let json = call_system_action(&state, ann).await?;
    serde_json::from_str(&json).map_err(err)
}

#[tauri::command]
pub async fn remove_network(state: State<'_, AppState>, name: String) -> Result<(), String> {
    call_system_action(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "network_rm".into()),
            ("coral.network.name".into(), name),
        ]),
    )
    .await?;
    Ok(())
}

#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
        return Err("only http:// and https:// URLs can be opened".into());
    }
    if trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("invalid whitespace or control characters in URL".into());
    }
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(trimmed);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(trimmed);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", trimmed]);
        c
    };
    cmd.spawn().map_err(|e| format!("open browser: {e}"))?;
    Ok(())
}
