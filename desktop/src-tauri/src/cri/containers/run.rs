//! Tauri CRI command that creates a pod sandbox + container in one call (`run_container`).

use super::super::{AppState, err};
use coral_api::{
    ContainerConfig, ContainerMetadata, CreateContainerRequest, ImageSpec, KeyValue,
    LinuxContainerConfig, LinuxContainerResources, PodSandboxConfig, PodSandboxMetadata,
    RunPodSandboxRequest, StartContainerRequest,
};
use serde::Deserialize;
use std::collections::HashMap;
use tauri::State;

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

    let mut labels = HashMap::from([("managed-by".into(), "coral-desktop".into())]);
    if !params.ports.trim().is_empty() {
        labels.insert("coral.ports".into(), params.ports.trim().into());
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
                working_dir: String::new(),
                envs,
                labels,
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
