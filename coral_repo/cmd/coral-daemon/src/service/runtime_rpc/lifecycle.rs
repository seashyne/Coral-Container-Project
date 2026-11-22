//! CRI v1 container start/stop/remove/list handlers.

#![allow(clippy::result_large_err)]

use crate::images;
use crate::service::CoralDaemonService;
use crate::service::port_forward::spawn_port_forwarders;
use crate::service::supervisor::{SupervisorLaunch, spawn_container_supervisor};
use chrono::Utc;
use coral_api::{
    Container, ContainerState as CriContainerState, ListContainersRequest,
    ListContainersResponse, RemoveContainerRequest, RemoveContainerResponse,
    StartContainerRequest, StartContainerResponse, StopContainerRequest, StopContainerResponse,
};
use std::fs;
use tokio::process::Command;
use tonic::{Request, Response, Status};

pub(super) async fn start_container(
    svc: &CoralDaemonService,
    request: Request<StartContainerRequest>,
) -> Result<Response<StartContainerResponse>, Status> {
    let query = request.into_inner().container_id;
    let (
        id,
        bundle_dir,
        rootfs_path,
        log_path,
        ports_spec,
        is_bridge_net,
        vswitch_attach,
        is_wasm,
        wasm_args,
        wasm_envs,
        wasm_fuel,
        wasm_mem_mb,
    ) = {
        let mut store = svc.store.write().await;
        let id = store
            .resolve_container_id(&query)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        let ctr = store
            .containers
            .get_mut(&id)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        if ctr.state == CriContainerState::ContainerRunning {
            return Err(Status::failed_precondition(format!(
                "container '{id}' is already RUNNING"
            )));
        }
        ctr.state = CriContainerState::ContainerRunning;
        ctr.stopped_by_user = false;
        ctr.started_at = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let net_mode = ctr
            .config
            .annotations
            .get("coral.network")
            .map(String::as_str)
            .unwrap_or("host");
        let is_bridge =
            !net_mode.eq_ignore_ascii_case("host") && !net_mode.eq_ignore_ascii_case("none");
        let is_wasm = CoralDaemonService::is_wasm_container(&ctr.config.annotations, &ctr.rootfs_path);
        let mut wasm_args = Vec::new();
        wasm_args.extend(ctr.config.command.iter().cloned());
        wasm_args.extend(ctr.config.args.iter().cloned());
        let wasm_envs: Vec<String> = ctr
            .config
            .envs
            .iter()
            .map(|kv| format!("{}={}", kv.key, kv.value))
            .collect();
        let wasm_fuel = ctr
            .config
            .annotations
            .get("coral.wasm.fuel")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(50_000_000);
        let wasm_mem_mb = ctr
            .config
            .linux
            .as_ref()
            .and_then(|l| l.resources.as_ref())
            .map(|r| r.memory_limit_in_bytes)
            .filter(|&b| b > 0)
            .map(|b| (b / (1024 * 1024)).max(1) as usize)
            .unwrap_or(64);
        let vswitch_attach = match (
            ctr.config.labels.get("coral.network.name").cloned(),
            ctr.config.labels.get("coral.network.ip").cloned(),
        ) {
            (Some(net), Some(ip)) => Some((net, ip)),
            _ => None,
        };
        let ret = (
            id,
            ctr.bundle_dir.clone(),
            ctr.rootfs_path.clone(),
            ctr.log_path.clone(),
            ctr.config.annotations.get("coral.ports").cloned(),
            is_bridge,
            vswitch_attach,
            is_wasm,
            wasm_args,
            wasm_envs,
            wasm_fuel,
            wasm_mem_mb,
        );
        svc.persist_state(&store);
        ret
    };

    if is_wasm {
        svc.spawn_wasm_container(
            id,
            rootfs_path,
            log_path,
            wasm_args,
            wasm_envs,
            wasm_fuel,
            wasm_mem_mb,
        );
        return Ok(Response::new(StartContainerResponse {}));
    }

    let runtime_root = svc.runtime_root();

    if let Some(raw_ports) = ports_spec {
        let handles =
            spawn_port_forwarders(&id, &raw_ports, is_bridge_net, &runtime_root).await?;
        if !handles.is_empty() {
            svc.store
                .write()
                .await
                .port_forwarders
                .insert(id.clone(), handles);
        }
    }

    let sup_handle = spawn_container_supervisor(SupervisorLaunch {
        cid: id.clone(),
        bundle_dir,
        log_path,
        runtime_root,
        state_dir: svc.cfg.state_dir.clone(),
        coral_bin: svc.cfg.coral_bin.clone(),
        store: svc.store.clone(),
        log_policy: svc.current_log_policy(),
    });
    if let Some((net_name, ip_str)) = vswitch_attach {
        svc.spawn_vswitch_attach(id.clone(), net_name, ip_str);
    }
    if let Some(prev) = svc.store.write().await.supervisors.insert(id, sup_handle) {
        prev.abort();
    }

    Ok(Response::new(StartContainerResponse {}))
}

pub(super) async fn stop_container(
    svc: &CoralDaemonService,
    request: Request<StopContainerRequest>,
) -> Result<Response<StopContainerResponse>, Status> {
    let query = request.into_inner().container_id;
    let id = {
        let mut store = svc.store.write().await;
        let id = store
            .resolve_container_id(&query)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        let ctr = store
            .containers
            .get_mut(&id)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        ctr.stopped_by_user = true;
        svc.persist_state(&store);
        id
    };

    let _ = Command::new(&svc.cfg.coral_bin)
        .arg("--root")
        .arg(svc.runtime_root())
        .arg("kill")
        .arg(&id)
        .arg("SIGTERM")
        .output()
        .await;

    let mut store = svc.store.write().await;
    let net_name = store
        .containers
        .get(&id)
        .and_then(|c| c.config.labels.get("coral.network.name").cloned());
    CoralDaemonService::detach_vswitch_port(&store, &id, net_name.as_deref());
    if let Some(handles) = store.port_forwarders.remove(&id) {
        for h in handles {
            h.abort();
        }
    }
    if let Some(ctr) = store.containers.get_mut(&id)
        && ctr.state != CriContainerState::ContainerExited
    {
        ctr.state = CriContainerState::ContainerExited;
        ctr.finished_at = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        ctr.reason = "Stopped".into();
    }
    svc.persist_state(&store);
    Ok(Response::new(StopContainerResponse {}))
}

pub(super) async fn remove_container(
    svc: &CoralDaemonService,
    request: Request<RemoveContainerRequest>,
) -> Result<Response<RemoveContainerResponse>, Status> {
    let query = request.into_inner().container_id;
    let (id, removed) = {
        let mut store = svc.store.write().await;
        let id = store
            .resolve_container_id(&query)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        if let Some(sup) = store.supervisors.remove(&id) {
            sup.abort();
        }
        if let Some(handles) = store.port_forwarders.remove(&id) {
            for h in handles {
                h.abort();
            }
        }
        let removed = store
            .containers
            .remove(&id)
            .ok_or_else(|| Status::not_found(format!("container '{query}' not found")))?;
        let net_name = removed.config.labels.get("coral.network.name").map(String::as_str);
        CoralDaemonService::detach_vswitch_port(&store, &id, net_name);
        svc.persist_state(&store);
        (id, removed)
    };

    let _ = Command::new(&svc.cfg.coral_bin)
        .arg("--root")
        .arg(svc.runtime_root())
        .arg("delete")
        .arg("--force")
        .arg(&id)
        .output()
        .await;
    let _ = images::remove_dir_force(&removed.bundle_dir);
    let _ = fs::remove_file(&removed.log_path);
    Ok(Response::new(RemoveContainerResponse {}))
}

pub(super) async fn list_containers(
    svc: &CoralDaemonService,
    request: Request<ListContainersRequest>,
) -> Result<Response<ListContainersResponse>, Status> {
    let filter = request.into_inner().filter;
    let store = svc.store.read().await;
    let containers = store
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
        .map(|c| Container {
            id: c.id.clone(),
            pod_sandbox_id: c.pod_sandbox_id.clone(),
            metadata: c.config.metadata.clone(),
            image: c.config.image.clone(),
            image_ref: c.rootfs_path.to_string_lossy().into_owned(),
            state: c.state as i32,
            created_at: c.created_at,
            labels: c.config.labels.clone(),
            annotations: c.config.annotations.clone(),
        })
        .collect();

    Ok(Response::new(ListContainersResponse { containers }))
}
