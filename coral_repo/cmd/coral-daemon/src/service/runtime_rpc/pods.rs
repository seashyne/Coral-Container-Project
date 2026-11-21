//! CRI v1 pod-sandbox RPC handlers.

#![allow(clippy::result_large_err)]

use super::cni::build_cni_exec_request;
use crate::service::{CoralDaemonService, SandboxRecord};
use chrono::Utc;
use coral_api::{
    ListPodSandboxRequest, ListPodSandboxResponse, PodSandbox, PodSandboxNetworkStatus,
    PodSandboxState, PodSandboxStatus, PodSandboxStatusRequest, PodSandboxStatusResponse,
    RemovePodSandboxRequest, RemovePodSandboxResponse, RunPodSandboxRequest,
    RunPodSandboxResponse, RuntimeCondition, RuntimeStatus, StatusRequest, StatusResponse,
    StopPodSandboxRequest, StopPodSandboxResponse, VersionRequest, VersionResponse,
};
use std::collections::HashMap;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

pub(super) async fn version(
    _svc: &CoralDaemonService,
    _request: Request<VersionRequest>,
) -> Result<Response<VersionResponse>, Status> {
    Ok(Response::new(VersionResponse {
        version: "0.1.0".into(),
        runtime_name: "coral".into(),
        runtime_version: env!("CARGO_PKG_VERSION").into(),
        runtime_api_version: "v1".into(),
    }))
}

pub(super) async fn status(
    svc: &CoralDaemonService,
    request: Request<StatusRequest>,
) -> Result<Response<StatusResponse>, Status> {
    let verbose = request.into_inner().verbose;
    let conditions = vec![
        RuntimeCondition {
            r#type: "RuntimeReady".into(),
            status: true,
            reason: String::new(),
            message: String::new(),
        },
        RuntimeCondition {
            r#type: "NetworkReady".into(),
            status: true,
            reason: String::new(),
            message: String::new(),
        },
    ];
    let mut info = HashMap::new();
    if verbose {
        let kvm = coral_vmm::probe_kvm();
        let summary = serde_json::json!({
            "runtime": "coral",
            "version": env!("CARGO_PKG_VERSION"),
            "stateDir": svc.cfg.state_dir.display().to_string(),
            "imagesDir": svc.images_dir.display().to_string(),
            "wasmRuntime": "wasmi-2.0-wasi-p1+http",
            "kvmAvailable": kvm.available,
            "kvmApiVersion": kvm.api_version,
            "arch": std::env::consts::ARCH,
        });
        info.insert("config".into(), summary.to_string());
    }
    Ok(Response::new(StatusResponse {
        status: Some(RuntimeStatus { conditions }),
        info,
    }))
}

pub(super) async fn pod_sandbox_status(
    svc: &CoralDaemonService,
    request: Request<PodSandboxStatusRequest>,
) -> Result<Response<PodSandboxStatusResponse>, Status> {
    let req = request.into_inner();
    let store = svc.store.read().await;
    let sb = store
        .sandboxes
        .get(&req.pod_sandbox_id)
        .ok_or_else(|| Status::not_found(format!("pod sandbox '{}' not found", req.pod_sandbox_id)))?;
    let mut info = HashMap::new();
    if req.verbose {
        info.insert("hostname".into(), sb.config.hostname.clone());
    }
    Ok(Response::new(PodSandboxStatusResponse {
        status: Some(PodSandboxStatus {
            id: sb.id.clone(),
            metadata: sb.config.metadata.clone(),
            state: sb.state as i32,
            created_at: sb.created_at,
            network: Some(PodSandboxNetworkStatus {
                ip: sb.pod_ip.clone().unwrap_or_else(|| "127.0.0.1".into()),
            }),
            labels: sb.config.labels.clone(),
            annotations: sb.config.annotations.clone(),
            runtime_handler: "coral".into(),
        }),
        info,
    }))
}

pub(super) async fn run_pod_sandbox(
    svc: &CoralDaemonService,
    request: Request<RunPodSandboxRequest>,
) -> Result<Response<RunPodSandboxResponse>, Status> {
    let req = request.into_inner();
    let config = req
        .config
        .ok_or_else(|| Status::invalid_argument("sandbox config is required"))?;

    // Special Compose stack launch hook (`coral.compose.yaml` annotation).
    if let Some(yaml) = config.annotations.get("coral.compose.yaml") {
        let project_override = config
            .annotations
            .get("coral.compose.project")
            .map(String::as_str);
        let base_dir = config
            .annotations
            .get("coral.compose.base_dir")
            .map(String::as_str);
        let result = svc
            .run_compose_stack_yaml(yaml, project_override, base_dir)
            .await?;
        return Ok(Response::new(RunPodSandboxResponse {
            pod_sandbox_id: result,
        }));
    }

    // Special System & Volume management hook (`coral.system.action` annotation).
    if config.annotations.contains_key("coral.system.action") {
        let result = svc.handle_system_action(&config.annotations).await?;
        return Ok(Response::new(RunPodSandboxResponse {
            pod_sandbox_id: result,
        }));
    }

    let id = svc.next_id("pod");
    let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);

    // Execute Kubernetes CNI v1.0.0 plugin chain (`ADD`) when CNI configs are present in conf_dir.
    let cni_req = build_cni_exec_request(&id, &config);
    let cni = svc.cni.clone();
    let cni_outcome = tokio::task::spawn_blocking(move || cni.add_pod_network(&cni_req))
        .await
        .map_err(|e| Status::internal(format!("CNI ADD join error: {e}")))?
        .map_err(|e| Status::internal(format!("CNI ADD failed: {e:#}")))?;

    let (pod_ip, cni_raw_result) = match cni_outcome {
        Some(out) => {
            info!(
                pod_sandbox_id = %id,
                cni_network = %out.network_name,
                pod_ip = ?out.pod_ip,
                "CNI plugin chain attached pod network"
            );
            (out.pod_ip, Some(out.raw_result))
        }
        None => (None, None),
    };

    let record = SandboxRecord {
        id: id.clone(),
        config,
        state: PodSandboxState::SandboxReady,
        created_at: now,
        pod_ip,
        cni_raw_result,
    };

    let mut store = svc.store.write().await;
    store.sandboxes.insert(id.clone(), record);
    svc.persist_state(&store);
    drop(store);
    info!(pod_sandbox_id = %id, "pod sandbox ready");
    Ok(Response::new(RunPodSandboxResponse { pod_sandbox_id: id }))
}

pub(super) async fn stop_pod_sandbox(
    svc: &CoralDaemonService,
    request: Request<StopPodSandboxRequest>,
) -> Result<Response<StopPodSandboxResponse>, Status> {
    let id = request.into_inner().pod_sandbox_id;
    let (id, cni_teardown) = {
        let mut store = svc.store.write().await;
        let id = store
            .resolve_sandbox_id(&id)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{id}' not found")))?;
        let sb = store
            .sandboxes
            .get_mut(&id)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{id}' not found")))?;
        sb.state = PodSandboxState::SandboxNotready;
        let prev = sb.cni_raw_result.take();
        let req = prev.as_ref().map(|_| build_cni_exec_request(&id, &sb.config));
        svc.persist_state(&store);
        (id, req.zip(prev))
    };
    if let Some((cni_req, prev_res)) = cni_teardown {
        let cni = svc.cni.clone();
        if let Ok(Err(e)) =
            tokio::task::spawn_blocking(move || cni.del_pod_network(&cni_req, Some(&prev_res)))
                .await
        {
            warn!(pod_sandbox_id = %id, err = %e, "CNI DEL reported warning during stop_pod_sandbox");
        }
    }
    Ok(Response::new(StopPodSandboxResponse {}))
}

pub(super) async fn remove_pod_sandbox(
    svc: &CoralDaemonService,
    request: Request<RemovePodSandboxRequest>,
) -> Result<Response<RemovePodSandboxResponse>, Status> {
    let query = request.into_inner().pod_sandbox_id;
    let (id, removed) = {
        let mut store = svc.store.write().await;
        let id = store
            .resolve_sandbox_id(&query)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{query}' not found")))?;
        let has_containers = store
            .containers
            .values()
            .any(|c| c.pod_sandbox_id == id);
        if has_containers {
            return Err(Status::failed_precondition(format!(
                "cannot remove pod sandbox '{query}' while containers still exist"
            )));
        }
        let sb = store
            .sandboxes
            .remove(&id)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{query}' not found")))?;
        svc.persist_state(&store);
        (id, sb)
    };
    if let Some(prev_res) = removed.cni_raw_result {
        let cni = svc.cni.clone();
        let cni_req = build_cni_exec_request(&id, &removed.config);
        let _ = tokio::task::spawn_blocking(move || {
            cni.del_pod_network(&cni_req, Some(&prev_res))
        })
        .await;
    }
    Ok(Response::new(RemovePodSandboxResponse {}))
}

pub(super) async fn list_pod_sandbox(
    svc: &CoralDaemonService,
    request: Request<ListPodSandboxRequest>,
) -> Result<Response<ListPodSandboxResponse>, Status> {
    let filter = request.into_inner().filter;
    let store = svc.store.read().await;
    let items = store
        .sandboxes
        .values()
        .filter(|sb| {
            if let Some(f) = &filter {
                if !f.id.is_empty() && sb.id != f.id {
                    return false;
                }
                for (k, v) in &f.label_selector {
                    if sb.config.labels.get(k) != Some(v) {
                        return false;
                    }
                }
            }
            true
        })
        .map(|sb| PodSandbox {
            id: sb.id.clone(),
            metadata: sb.config.metadata.clone(),
            state: sb.state as i32,
            created_at: sb.created_at,
            labels: sb.config.labels.clone(),
            annotations: sb.config.annotations.clone(),
        })
        .collect();

    Ok(Response::new(ListPodSandboxResponse { items }))
}
