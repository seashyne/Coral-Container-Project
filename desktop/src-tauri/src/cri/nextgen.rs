//! Tauri CRI v2.0 Next-Gen commands (`vmm_probe`, `wasm_run`, `sandbox_run`).

use super::{AppState, err};
use coral_api::{PodSandboxConfig, PodSandboxMetadata, RunPodSandboxRequest};
use std::collections::HashMap;
use tauri::State;

async fn invoke_nextgen(
    state: &State<'_, AppState>,
    annotations: HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    let mut client = state.runtime().await?;
    let resp = client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-desktop-nextgen".into(),
                    uid: "coral-desktop-nextgen".into(),
                    namespace: "coral-system".into(),
                    attempt: 0,
                }),
                hostname: "coral-nextgen".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations,
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .map_err(err)?
        .into_inner();
    serde_json::from_str(&resp.pod_sandbox_id).map_err(err)
}

#[tauri::command]
pub async fn vmm_probe(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    invoke_nextgen(
        &state,
        HashMap::from([("coral.system.action".into(), "vmm_probe".into())]),
    )
    .await
}

#[tauri::command]
pub async fn wasm_run(
    state: State<'_, AppState>,
    source: Option<String>,
    image: Option<String>,
    fuel_limit: Option<u64>,
    memory_limit_mb: Option<usize>,
) -> Result<serde_json::Value, String> {
    let mut ann = HashMap::from([("coral.system.action".into(), "wasm_run".into())]);
    if let Some(s) = source
        && !s.trim().is_empty()
    {
        ann.insert("coral.wasm.source".into(), s);
    }
    if let Some(i) = image
        && !i.trim().is_empty()
    {
        ann.insert("coral.wasm.image".into(), i);
    }
    if let Some(f) = fuel_limit {
        ann.insert("coral.wasm.fuel".into(), f.to_string());
    }
    if let Some(m) = memory_limit_mb {
        ann.insert("coral.wasm.memory_mb".into(), m.to_string());
    }
    invoke_nextgen(&state, ann).await
}

#[tauri::command]
pub async fn sandbox_run(
    state: State<'_, AppState>,
    image: String,
    cmd: String,
    timeout_secs: Option<u64>,
    memory_mb: Option<i64>,
    allow_network: Option<bool>,
) -> Result<serde_json::Value, String> {
    let mut ann = HashMap::from([
        ("coral.system.action".into(), "sandbox_run".into()),
        ("coral.sandbox.image".into(), image),
        ("coral.sandbox.cmd".into(), cmd),
    ]);
    if let Some(t) = timeout_secs {
        ann.insert("coral.sandbox.timeout".into(), t.to_string());
    }
    if let Some(m) = memory_mb {
        ann.insert("coral.sandbox.memory_mb".into(), m.to_string());
    }
    if allow_network.unwrap_or(false) {
        ann.insert("coral.sandbox.allow_network".into(), "true".into());
    }
    invoke_nextgen(&state, ann).await
}

#[tauri::command]
pub async fn image_sign(
    state: State<'_, AppState>,
    image: String,
) -> Result<serde_json::Value, String> {
    invoke_nextgen(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "image_sign".into()),
            ("coral.image.ref".into(), image),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn image_verify(
    state: State<'_, AppState>,
    image: String,
) -> Result<serde_json::Value, String> {
    invoke_nextgen(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "image_verify".into()),
            ("coral.image.ref".into(), image),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn image_sbom(
    state: State<'_, AppState>,
    image: String,
) -> Result<serde_json::Value, String> {
    invoke_nextgen(
        &state,
        HashMap::from([
            ("coral.system.action".into(), "image_sbom".into()),
            ("coral.image.ref".into(), image),
        ]),
    )
    .await
}

#[tauri::command]
pub async fn image_scan(
    state: State<'_, AppState>,
    image: String,
    offline: Option<bool>,
) -> Result<serde_json::Value, String> {
    let mut ann = HashMap::from([
        ("coral.system.action".into(), "image_scan".into()),
        ("coral.image.ref".into(), image),
    ]);
    if offline.unwrap_or(false) {
        ann.insert("coral.scan.offline".into(), "true".into());
    }
    invoke_nextgen(&state, ann).await
}

#[tauri::command]
pub async fn vm_setup_kernel(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    invoke_nextgen(
        &state,
        HashMap::from([("coral.system.action".into(), "vm_setup_kernel".into())]),
    )
    .await
}

#[tauri::command]
pub async fn vm_boot(
    state: State<'_, AppState>,
    image: String,
    cmd: String,
    memory_mb: Option<i64>,
    timeout_secs: Option<u64>,
    disk: Option<String>,
    allow_net: Option<bool>,
) -> Result<serde_json::Value, String> {
    let mut ann = HashMap::from([
        ("coral.system.action".into(), "vm_boot".into()),
        ("coral.vm.image".into(), image),
        ("coral.vm.cmd".into(), cmd),
    ]);
    if let Some(m) = memory_mb {
        ann.insert("coral.vm.memory_mb".into(), m.to_string());
    }
    if let Some(t) = timeout_secs {
        ann.insert("coral.vm.timeout".into(), t.to_string());
    }
    if let Some(d) = disk
        && !d.trim().is_empty()
    {
        ann.insert("coral.vm.disk".into(), d.trim().to_string());
    }
    if allow_net.unwrap_or(false) {
        ann.insert("coral.vm.net".into(), "true".into());
    }
    invoke_nextgen(&state, ann).await
}
