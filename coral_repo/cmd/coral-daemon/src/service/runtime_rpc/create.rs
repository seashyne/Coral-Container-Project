//! CRI v1 container creation and status inspection.

#![allow(clippy::result_large_err)]

use crate::service::metrics::collect_container_metrics;
use crate::service::{CoralDaemonService, ManagedContainer};
use chrono::Utc;
use coral_api::{
    ContainerMetadata, ContainerState as CriContainerState, ContainerStatus,
    ContainerStatusRequest, ContainerStatusResponse, CreateContainerRequest,
    CreateContainerResponse,
};
use coral_runtime::overlay::diff_overlay;
use std::collections::HashMap;
use std::fs;
use tonic::{Request, Response, Status};
use tracing::info;

pub(super) async fn create_container(
    svc: &CoralDaemonService,
    request: Request<CreateContainerRequest>,
) -> Result<Response<CreateContainerResponse>, Status> {
    let req = request.into_inner();
    let config = req
        .config
        .ok_or_else(|| Status::invalid_argument("container config is required"))?;

    let mut store = svc.store.write().await;
    let sandbox = store
        .sandboxes
        .get(&req.pod_sandbox_id)
        .ok_or_else(|| {
            Status::not_found(format!("pod sandbox '{}' not found", req.pod_sandbox_id))
        })?
        .clone();

    let image_ref = config
        .image
        .as_ref()
        .map(|i| i.image.clone())
        .unwrap_or_default();
    let rootfs_path = svc.resolve_rootfs_for_image(&store, &image_ref)?;

    let id = svc.next_id("ctr");
    let bundle_dir = svc.cfg.state_dir.join("bundles").join(&id);
    let log_dir = svc.cfg.state_dir.join("logs");
    fs::create_dir_all(&bundle_dir)
        .map_err(|e| Status::internal(format!("create bundle dir: {e}")))?;
    fs::create_dir_all(&log_dir)
        .map_err(|e| Status::internal(format!("create logs dir: {e}")))?;
    let log_path = log_dir.join(format!("{id}.log"));

    let mut config = config;
    svc.configure_container_network(&mut store, &sandbox, &mut config)?;

    if !config.annotations.contains_key("coral.health.cmd") {
        let img_cfg = crate::images::OciImageConfig::load_for_rootfs(&rootfs_path);
        if let Some(hc) = img_cfg.healthcheck_cmd
            && !hc.trim().is_empty()
        {
            config.annotations.insert("coral.health.cmd".into(), hc);
            if img_cfg.healthcheck_interval_ms > 0 {
                config.annotations.insert(
                    "coral.health.interval_ms".into(),
                    img_cfg.healthcheck_interval_ms.to_string(),
                );
            }
            if img_cfg.healthcheck_retries > 0 {
                config.annotations.insert(
                    "coral.health.retries".into(),
                    img_cfg.healthcheck_retries.to_string(),
                );
            }
        }
    }
    let sandbox_cfg = req.sandbox_config.unwrap_or_else(|| sandbox.config.clone());
    let oci_spec = svc.build_oci_spec(&config, &sandbox_cfg, &rootfs_path, &bundle_dir)?;
    oci_spec
        .to_file(bundle_dir.join("config.json"))
        .map_err(|e| Status::invalid_argument(format!("invalid OCI spec: {e}")))?;

    let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let restart_policy = config
        .annotations
        .get("coral.restart")
        .cloned()
        .unwrap_or_else(|| "no".into());
    let has_health = config
        .annotations
        .get("coral.health.cmd")
        .is_some_and(|s| !s.trim().is_empty());
    let health_status = if has_health {
        "starting".to_string()
    } else {
        "none".to_string()
    };
    if has_health {
        config
            .labels
            .insert("coral.health.status".into(), health_status.clone());
    }
    let managed = ManagedContainer {
        id: id.clone(),
        pod_sandbox_id: req.pod_sandbox_id,
        config,
        bundle_dir,
        rootfs_path,
        log_path,
        state: CriContainerState::ContainerCreated,
        created_at: now,
        started_at: 0,
        finished_at: 0,
        exit_code: 0,
        reason: String::new(),
        message: String::new(),
        restart_policy,
        restart_count: 0,
        stopped_by_user: false,
        health_status,
        health_failing_streak: 0,
    };

    store.containers.insert(id.clone(), managed);
    svc.persist_state(&store);
    info!(container_id = %id, "created CRI container bundle");
    Ok(Response::new(CreateContainerResponse { container_id: id }))
}

pub(super) async fn container_status(
    svc: &CoralDaemonService,
    request: Request<ContainerStatusRequest>,
) -> Result<Response<ContainerStatusResponse>, Status> {
    let req = request.into_inner();
    let store = svc.store.read().await;
    let resolved_id = store
        .resolve_container_id(&req.container_id)
        .ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?;
    let ctr = store
        .containers
        .get(&resolved_id)
        .ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?;

    let mut info_map = HashMap::new();
    if req.verbose {
        info_map.insert(
            "bundleDir".into(),
            ctr.bundle_dir.to_string_lossy().into_owned(),
        );
        info_map.insert(
            "logPath".into(),
            ctr.log_path.to_string_lossy().into_owned(),
        );
        if let Ok(log_bytes) = fs::read(&ctr.log_path) {
            info_map.insert(
                "logOutput".into(),
                String::from_utf8_lossy(&log_bytes).into_owned(),
            );
        }
        let upperdir = ctr.bundle_dir.join("overlay/upper");
        if let Ok(changes) = diff_overlay(&upperdir, Some(&ctr.rootfs_path)) {
            let formatted = changes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            info_map.insert("overlayDiff".into(), formatted);
        }
        info_map.insert("restartPolicy".into(), ctr.restart_policy.clone());
        info_map.insert("restartCount".into(), ctr.restart_count.to_string());
        info_map.insert("healthStatus".into(), ctr.health_status.clone());
        info_map.insert(
            "healthFailingStreak".into(),
            ctr.health_failing_streak.to_string(),
        );
        let mut live_metrics = None;
        if ctr.state == CriContainerState::ContainerRunning {
            let mem_limit = ctr
                .config
                .linux
                .as_ref()
                .and_then(|l| l.resources.as_ref())
                .map(|r| r.memory_limit_in_bytes)
                .unwrap_or(0);
            if let Some(m) = collect_container_metrics(
                &svc.runtime_root(),
                &ctr.id,
                mem_limit,
                ctr.started_at,
            ) {
                if let Ok(json) = serde_json::to_string(&m) {
                    info_map.insert("metricsJson".into(), json);
                }
                live_metrics = Some(m);
            }
        }

        let res = ctr.config.linux.as_ref().and_then(|l| l.resources.as_ref());
        let state_label = match ctr.state {
            CriContainerState::ContainerCreated => "created",
            CriContainerState::ContainerRunning => "running",
            CriContainerState::ContainerExited => "exited",
            _ => "unknown",
        };
        let inspect = serde_json::json!({
            "id": ctr.id,
            "name": ctr.config.metadata.as_ref().map(|m| m.name.as_str()).unwrap_or(&ctr.id),
            "podSandboxId": ctr.pod_sandbox_id,
            "image": ctr.config.image.as_ref().map(|i| i.image.as_str()).unwrap_or(""),
            "state": state_label,
            "exitCode": ctr.exit_code,
            "reason": ctr.reason,
            "message": ctr.message,
            "createdAt": ctr.created_at,
            "startedAt": ctr.started_at,
            "finishedAt": ctr.finished_at,
            "command": ctr.config.command,
            "args": ctr.config.args,
            "workingDir": ctr.config.working_dir,
            "envs": ctr.config.envs.iter().map(|kv| format!("{}={}", kv.key, kv.value)).collect::<Vec<_>>(),
            "labels": ctr.config.labels,
            "annotations": ctr.config.annotations,
            "restartPolicy": ctr.restart_policy,
            "restartCount": ctr.restart_count,
            "stoppedByUser": ctr.stopped_by_user,
            "healthStatus": ctr.health_status,
            "healthFailingStreak": ctr.health_failing_streak,
            "bundleDir": ctr.bundle_dir,
            "rootfsPath": ctr.rootfs_path,
            "overlayUpperDir": upperdir,
            "logPath": ctr.log_path,
            "resources": {
                "memoryLimitBytes": res.map(|r| r.memory_limit_in_bytes).unwrap_or(0),
                "cpuQuota": res.map(|r| r.cpu_quota).unwrap_or(0),
                "cpuPeriod": res.map(|r| r.cpu_period).unwrap_or(0),
                "pidsLimit": res.map(|r| r.pids_limit).unwrap_or(0),
            },
            "metrics": live_metrics,
        });
        if let Ok(inspect_str) = serde_json::to_string_pretty(&inspect) {
            info_map.insert("inspectJson".into(), inspect_str);
        }
    }

    let status = ContainerStatus {
        id: ctr.id.clone(),
        metadata: ctr.config.metadata.clone().or_else(|| {
            Some(ContainerMetadata {
                name: ctr.id.clone(),
                attempt: 0,
            })
        }),
        state: ctr.state as i32,
        created_at: ctr.created_at,
        started_at: ctr.started_at,
        finished_at: ctr.finished_at,
        exit_code: ctr.exit_code,
        image: ctr.config.image.clone(),
        image_ref: ctr.rootfs_path.to_string_lossy().into_owned(),
        reason: ctr.reason.clone(),
        message: ctr.message.clone(),
        labels: ctr.config.labels.clone(),
        annotations: ctr.config.annotations.clone(),
    };

    Ok(Response::new(ContainerStatusResponse {
        status: Some(status),
        info: info_map,
    }))
}
