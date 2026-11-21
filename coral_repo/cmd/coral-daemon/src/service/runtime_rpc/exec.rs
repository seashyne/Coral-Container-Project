//! CRI v1 exec / attach / port-forward handlers.

#![allow(clippy::result_large_err)]

use crate::images;
use crate::service::CoralDaemonService;
use crate::service::pty_rpc::ExecStreamOutput;
use crate::service::streaming::StreamKind;
use coral_api::{
    AttachRequest, AttachResponse, ContainerState as CriContainerState, ExecRequest, ExecResponse,
    ExecStreamRequest,
    ExecSyncRequest, ExecSyncResponse, PortForwardRequest, PortForwardResponse,
};
use coral_oci::OciSpec;
use std::fs;
use tokio::process::Command;
use tonic::{Request, Response, Status, Streaming};

pub(super) async fn exec_sync(
    svc: &CoralDaemonService,
    request: Request<ExecSyncRequest>,
) -> Result<Response<ExecSyncResponse>, Status> {
    let req = request.into_inner();
    if req.cmd.is_empty() {
        return Err(Status::invalid_argument("exec command must not be empty"));
    }

    let (canonical_cid, bundle_dir, is_running) = {
        let store = svc.store.read().await;
        let canonical_cid = store.resolve_container_id(&req.container_id).ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?;
        let ctr = store.containers.get(&canonical_cid).ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?;
        (
            canonical_cid,
            ctr.bundle_dir.clone(),
            ctr.state == CriContainerState::ContainerRunning,
        )
    };

    // 1. Live container: enter its real running namespaces via `setns()`!
    if is_running {
        let mut cmd = Command::new(&svc.cfg.coral_bin);
        cmd.arg("--root")
            .arg(svc.runtime_root())
            .arg("exec")
            .arg("--local")
            .arg(&canonical_cid)
            .arg("--");
        for arg in &req.cmd {
            cmd.arg(arg);
        }
        if let Ok(out) = cmd.output().await {
            let stderr_txt = String::from_utf8_lossy(&out.stderr);
            let container_gone = !out.status.success()
                && (stderr_txt.contains("must be 'running'")
                    || stderr_txt.contains("no longer alive")
                    || stderr_txt.contains("not found"));
            if !container_gone {
                return Ok(Response::new(ExecSyncResponse {
                    stdout: out.stdout,
                    stderr: out.stderr,
                    exit_code: out.status.code().unwrap_or(1),
                }));
            }
        }
    }

    // 2. Exited container (or race right as container finished): inspect via ephemeral exec bundle,
    // preserving the container's OverlayFS Copy-on-Write upper layer (`bundle_dir/overlay`).
    let mut spec = OciSpec::from_file(bundle_dir.join("config.json"))
        .map_err(|e| Status::internal(format!("load container config.json: {e}")))?;
    if let Some(proc_cfg) = spec.process.as_mut() {
        proc_cfg.args = req.cmd;
    }
    if bundle_dir.join("overlay/upper").is_dir() {
        spec.annotations.insert(
            "coral.overlay.bundle".into(),
            bundle_dir.to_string_lossy().into_owned(),
        );
    }

    let exec_id = svc.next_id("exec");
    let exec_bundle = svc.cfg.state_dir.join("exec-bundles").join(&exec_id);
    fs::create_dir_all(&exec_bundle)
        .map_err(|e| Status::internal(format!("create exec bundle dir: {e}")))?;
    spec.to_file(exec_bundle.join("config.json"))
        .map_err(|e| Status::internal(format!("write exec config.json: {e}")))?;

    let out = Command::new(&svc.cfg.coral_bin)
        .arg("--root")
        .arg(svc.runtime_root())
        .arg("run")
        .arg("--bundle")
        .arg(&exec_bundle)
        .arg(&exec_id)
        .output()
        .await
        .map_err(|e| Status::internal(format!("spawn coral exec: {e}")))?;

    let _ = images::remove_dir_force(&exec_bundle);

    Ok(Response::new(ExecSyncResponse {
        stdout: out.stdout,
        stderr: out.stderr,
        exit_code: out.status.code().unwrap_or(1),
    }))
}

pub(super) async fn exec(
    svc: &CoralDaemonService,
    request: Request<ExecRequest>,
) -> Result<Response<ExecResponse>, Status> {
    let req = request.into_inner();
    if req.cmd.is_empty() {
        return Err(Status::invalid_argument("exec command must not be empty"));
    }
    let canonical_cid = {
        let store = svc.store.read().await;
        store.resolve_container_id(&req.container_id).ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?
    };
    let url = svc
        .stream_server
        .issue_url(
            svc.clone(),
            "exec",
            StreamKind::Exec {
                container_id: canonical_cid,
                cmd: req.cmd,
                stdout: req.stdout || (!req.stdout && !req.stderr),
                stderr: req.stderr,
            },
        )
        .await
        .map_err(Status::internal)?;
    Ok(Response::new(ExecResponse { url }))
}

pub(super) async fn attach(
    svc: &CoralDaemonService,
    request: Request<AttachRequest>,
) -> Result<Response<AttachResponse>, Status> {
    let req = request.into_inner();
    let canonical_cid = {
        let store = svc.store.read().await;
        store.resolve_container_id(&req.container_id).ok_or_else(|| {
            Status::not_found(format!("container '{}' not found", req.container_id))
        })?
    };
    let url = svc
        .stream_server
        .issue_url(
            svc.clone(),
            "attach",
            StreamKind::Attach {
                container_id: canonical_cid,
            },
        )
        .await
        .map_err(Status::internal)?;
    Ok(Response::new(AttachResponse { url }))
}

pub(super) async fn port_forward(
    svc: &CoralDaemonService,
    request: Request<PortForwardRequest>,
) -> Result<Response<PortForwardResponse>, Status> {
    let req = request.into_inner();
    let canonical_pod_id = {
        let store = svc.store.read().await;
        store.resolve_sandbox_id(&req.pod_sandbox_id).ok_or_else(|| {
            Status::not_found(format!("pod sandbox '{}' not found", req.pod_sandbox_id))
        })?
    };
    let url = svc
        .stream_server
        .issue_url(
            svc.clone(),
            "portforward",
            StreamKind::PortForward {
                pod_sandbox_id: canonical_pod_id,
                ports: req.port,
            },
        )
        .await
        .map_err(Status::internal)?;
    Ok(Response::new(PortForwardResponse { url }))
}

pub(super) async fn exec_stream(
    svc: &CoralDaemonService,
    request: Request<Streaming<ExecStreamRequest>>,
) -> Result<Response<ExecStreamOutput>, Status> {
    let stream = svc.handle_exec_stream(request.into_inner()).await?;
    Ok(Response::new(stream))
}
