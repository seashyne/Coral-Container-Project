//! CRI v1 container stats and runtime-config handlers.

#![allow(clippy::result_large_err)]

use crate::images;
use crate::service::CoralDaemonService;
use crate::service::ManagedContainer;
use crate::service::metrics::collect_container_metrics;
use chrono::Utc;
use coral_api::{
    ContainerAttributes, ContainerStats,
    ContainerStatsRequest, ContainerStatsResponse, CpuUsage, FilesystemIdentifier,
    FilesystemUsage, ListContainerStatsRequest, ListContainerStatsResponse, MemoryUsage,
    ReopenContainerLogRequest, ReopenContainerLogResponse, UInt64Value,
    UpdateRuntimeConfigRequest, UpdateRuntimeConfigResponse,
};
use std::fs;
use tonic::{Request, Response, Status};
use tracing::info;

pub(super) async fn container_stats(
    svc: &CoralDaemonService,
    request: Request<ContainerStatsRequest>,
) -> Result<Response<ContainerStatsResponse>, Status> {
    let cid = request.into_inner().container_id;
    let store = svc.store.read().await;
    let resolved = store
        .resolve_container_id(&cid)
        .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?;
    let ctr = store
        .containers
        .get(&resolved)
        .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?;
    let stats = svc.build_cri_container_stats(ctr);
    Ok(Response::new(ContainerStatsResponse { stats: Some(stats) }))
}

pub(super) async fn list_container_stats(
    svc: &CoralDaemonService,
    request: Request<ListContainerStatsRequest>,
) -> Result<Response<ListContainerStatsResponse>, Status> {
    let filter = request.into_inner().filter;
    let store = svc.store.read().await;
    let stats = store
        .containers
        .values()
        .filter(|c| {
            if let Some(f) = &filter {
                if !f.id.is_empty() && c.id != f.id {
                    return false;
                }
                if !f.pod_sandbox_id.is_empty() && c.pod_sandbox_id != f.pod_sandbox_id {
                    return false;
                }
                for (k, v) in &f.label_selector {
                    if c.config.labels.get(k) != Some(v) {
                        return false;
                    }
                }
            }
            true
        })
        .map(|c| svc.build_cri_container_stats(c))
        .collect();
    Ok(Response::new(ListContainerStatsResponse { stats }))
}

pub(super) async fn update_runtime_config(
    svc: &CoralDaemonService,
    request: Request<UpdateRuntimeConfigRequest>,
) -> Result<Response<UpdateRuntimeConfigResponse>, Status> {
    let pod_cidr = request
        .into_inner()
        .runtime_config
        .and_then(|rc| rc.network_config)
        .map(|nc| nc.pod_cidr)
        .filter(|s| !s.trim().is_empty());
    if let Some(cidr) = pod_cidr {
        let mut store = svc.store.write().await;
        store.pod_cidr = Some(cidr.clone());
        info!(pod_cidr = %cidr, "updated CRI runtime pod CIDR");
    }
    Ok(Response::new(UpdateRuntimeConfigResponse {}))
}

pub(super) async fn reopen_container_log(
    svc: &CoralDaemonService,
    request: Request<ReopenContainerLogRequest>,
) -> Result<Response<ReopenContainerLogResponse>, Status> {
    let cid = request.into_inner().container_id;
    let log_path = {
        let store = svc.store.read().await;
        let resolved = store
            .resolve_container_id(&cid)
            .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?;
        store
            .containers
            .get(&resolved)
            .map(|c| c.log_path.clone())
            .ok_or_else(|| Status::not_found(format!("container '{cid}' not found")))?
    };
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| Status::internal(format!("reopen container log '{}': {e}", log_path.display())))?;
    Ok(Response::new(ReopenContainerLogResponse {}))
}

impl CoralDaemonService {
    fn build_cri_container_stats(&self, ctr: &ManagedContainer) -> ContainerStats {
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let mem_limit = ctr
            .config
            .linux
            .as_ref()
            .and_then(|l| l.resources.as_ref())
            .map(|r| r.memory_limit_in_bytes)
            .unwrap_or(0);
        let live = collect_container_metrics(
            &self.runtime_root(),
            &ctr.id,
            mem_limit,
            ctr.started_at,
        );
        let cpu_ns = live
            .as_ref()
            .map(|m| m.cpu_usage_usec.saturating_mul(1_000))
            .unwrap_or(0);
        let mem_bytes = live.as_ref().map(|m| m.memory_bytes).unwrap_or(0);
        let upper = ctr.bundle_dir.join("overlay/upper");
        let writable_bytes = if upper.is_dir() {
            images::dir_size(&upper)
        } else {
            0
        };
        ContainerStats {
            attributes: Some(ContainerAttributes {
                id: ctr.id.clone(),
                metadata: ctr.config.metadata.clone(),
                labels: ctr.config.labels.clone(),
                annotations: ctr.config.annotations.clone(),
            }),
            cpu: Some(CpuUsage {
                timestamp: now,
                usage_core_nano_seconds: Some(UInt64Value { value: cpu_ns }),
            }),
            memory: Some(MemoryUsage {
                timestamp: now,
                working_set_bytes: Some(UInt64Value { value: mem_bytes }),
            }),
            writable_layer: Some(FilesystemUsage {
                timestamp: now,
                fs_id: Some(FilesystemIdentifier {
                    mountpoint: ctr.bundle_dir.display().to_string(),
                }),
                used_bytes: Some(UInt64Value {
                    value: writable_bytes,
                }),
                inodes_used: Some(UInt64Value { value: 0 }),
            }),
        }
    }
}
