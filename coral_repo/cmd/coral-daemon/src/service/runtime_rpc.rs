//! Kubernetes CRI v1 `RuntimeService` gRPC implementation for `CoralDaemonService`.

use super::metrics::collect_container_metrics;
use super::port_forward::spawn_port_forwarders;
use super::pty_rpc::ExecStreamOutput;
use super::supervisor::{SupervisorLaunch, spawn_container_supervisor};
use super::{CoralDaemonService, ManagedContainer, SandboxRecord};
use crate::images;
use chrono::Utc;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    Container, ContainerMetadata, ContainerState as CriContainerState, ContainerStatus,
    ContainerStatusRequest, ContainerStatusResponse, CreateContainerRequest,
    CreateContainerResponse, ExecStreamRequest, ExecSyncRequest, ExecSyncResponse,
    ListContainersRequest, ListContainersResponse, ListPodSandboxRequest, ListPodSandboxResponse,
    PodSandbox, PodSandboxNetworkStatus, PodSandboxState, PodSandboxStatus,
    PodSandboxStatusRequest, PodSandboxStatusResponse, RemoveContainerRequest,
    RemoveContainerResponse, RemovePodSandboxRequest, RemovePodSandboxResponse,
    RunPodSandboxRequest, RunPodSandboxResponse, RuntimeCondition, RuntimeStatus,
    StartContainerRequest, StartContainerResponse, StatusRequest, StatusResponse,
    StopContainerRequest, StopContainerResponse, StopPodSandboxRequest, StopPodSandboxResponse,
    VersionRequest, VersionResponse,
};
use coral_oci::OciSpec;
use coral_runtime::overlay::diff_overlay;
use std::collections::HashMap;
use std::fs;
use tokio::process::Command;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;

#[tonic::async_trait]
impl RuntimeService for CoralDaemonService {
    type ExecStreamStream = ExecStreamOutput;
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        Ok(Response::new(VersionResponse {
            version: "0.1.0".into(),
            runtime_name: "coral".into(),
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            runtime_api_version: "v1".into(),
        }))
    }

    async fn status(
        &self,
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
                "stateDir": self.cfg.state_dir.display().to_string(),
                "imagesDir": self.images_dir.display().to_string(),
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

    async fn pod_sandbox_status(
        &self,
        request: Request<PodSandboxStatusRequest>,
    ) -> Result<Response<PodSandboxStatusResponse>, Status> {
        let req = request.into_inner();
        let store = self.store.read().await;
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
                    ip: "127.0.0.1".into(),
                }),
                labels: sb.config.labels.clone(),
                annotations: sb.config.annotations.clone(),
                runtime_handler: "coral".into(),
            }),
            info,
        }))
    }

    async fn run_pod_sandbox(
        &self,
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
            let result = self
                .run_compose_stack_yaml(yaml, project_override, base_dir)
                .await?;
            return Ok(Response::new(RunPodSandboxResponse {
                pod_sandbox_id: result,
            }));
        }

        // Special System & Volume management hook (`coral.system.action` annotation).
        if config.annotations.contains_key("coral.system.action") {
            let result = self.handle_system_action(&config.annotations).await?;
            return Ok(Response::new(RunPodSandboxResponse {
                pod_sandbox_id: result,
            }));
        }

        let id = self.next_id("pod");
        let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);

        let record = SandboxRecord {
            id: id.clone(),
            config,
            state: PodSandboxState::SandboxReady,
            created_at: now,
        };

        let mut store = self.store.write().await;
        store.sandboxes.insert(id.clone(), record);
        self.persist_state(&store);
        drop(store);
        info!(pod_sandbox_id = %id, "pod sandbox ready");
        Ok(Response::new(RunPodSandboxResponse { pod_sandbox_id: id }))
    }

    async fn stop_pod_sandbox(
        &self,
        request: Request<StopPodSandboxRequest>,
    ) -> Result<Response<StopPodSandboxResponse>, Status> {
        let id = request.into_inner().pod_sandbox_id;
        let mut store = self.store.write().await;
        let sb = store
            .sandboxes
            .get_mut(&id)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{id}' not found")))?;
        sb.state = PodSandboxState::SandboxNotready;
        self.persist_state(&store);
        Ok(Response::new(StopPodSandboxResponse {}))
    }

    async fn remove_pod_sandbox(
        &self,
        request: Request<RemovePodSandboxRequest>,
    ) -> Result<Response<RemovePodSandboxResponse>, Status> {
        let id = request.into_inner().pod_sandbox_id;
        let mut store = self.store.write().await;
        let has_containers = store
            .containers
            .values()
            .any(|c| c.pod_sandbox_id == id);
        if has_containers {
            return Err(Status::failed_precondition(format!(
                "cannot remove pod sandbox '{id}' while containers still exist"
            )));
        }
        store
            .sandboxes
            .remove(&id)
            .ok_or_else(|| Status::not_found(format!("pod sandbox '{id}' not found")))?;
        self.persist_state(&store);
        Ok(Response::new(RemovePodSandboxResponse {}))
    }

    async fn list_pod_sandbox(
        &self,
        request: Request<ListPodSandboxRequest>,
    ) -> Result<Response<ListPodSandboxResponse>, Status> {
        let filter = request.into_inner().filter;
        let store = self.store.read().await;
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

    async fn create_container(
        &self,
        request: Request<CreateContainerRequest>,
    ) -> Result<Response<CreateContainerResponse>, Status> {
        let req = request.into_inner();
        let config = req
            .config
            .ok_or_else(|| Status::invalid_argument("container config is required"))?;

        let mut store = self.store.write().await;
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
        let rootfs_path = self.resolve_rootfs_for_image(&store, &image_ref)?;

        let id = self.next_id("ctr");
        let bundle_dir = self.cfg.state_dir.join("bundles").join(&id);
        let log_dir = self.cfg.state_dir.join("logs");
        fs::create_dir_all(&bundle_dir)
            .map_err(|e| Status::internal(format!("create bundle dir: {e}")))?;
        fs::create_dir_all(&log_dir)
            .map_err(|e| Status::internal(format!("create logs dir: {e}")))?;
        let log_path = log_dir.join(format!("{id}.log"));

        let mut config = config;
        let net_name = config
            .annotations
            .get("coral.network")
            .cloned()
            .unwrap_or_else(|| "host".into());
        if !net_name.eq_ignore_ascii_case("host")
            && !net_name.eq_ignore_ascii_case("none")
            && !net_name.eq_ignore_ascii_case("bridge")
            && !net_name.eq_ignore_ascii_case("isolated")
        {
            if coral_net::find_network(&net_name).is_none() {
                return Err(Status::not_found(format!(
                    "network '{net_name}' not found — create it first with `coral network create {net_name}`"
                )));
            }
            config
                .labels
                .insert("coral.network.name".into(), net_name.clone());
            // Inject sibling container names on the same user-defined network into `/etc/hosts`.
            let mut sibling_hosts: Vec<String> = config
                .annotations
                .get("coral.extra_hosts")
                .map(|raw| {
                    raw.split(['\n', ','])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            for existing in store.containers.values() {
                if existing.config.labels.get("coral.network.name").map(String::as_str)
                    == Some(net_name.as_str())
                    && let Some(meta) = &existing.config.metadata
                    && !meta.name.is_empty()
                {
                    let entry = format!("{}:127.0.0.1", meta.name);
                    if !sibling_hosts.contains(&entry) {
                        sibling_hosts.push(entry);
                    }
                }
            }
            if !sibling_hosts.is_empty() {
                config
                    .annotations
                    .insert("coral.extra_hosts".into(), sibling_hosts.join("\n"));
            }
        }

        let sandbox_cfg = req.sandbox_config.unwrap_or_else(|| sandbox.config.clone());
        let oci_spec = self.build_oci_spec(&config, &sandbox_cfg, &rootfs_path, &bundle_dir)?;
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
        self.persist_state(&store);
        info!(container_id = %id, "created CRI container bundle");
        Ok(Response::new(CreateContainerResponse { container_id: id }))
    }

    async fn start_container(
        &self,
        request: Request<StartContainerRequest>,
    ) -> Result<Response<StartContainerResponse>, Status> {
        let id = request.into_inner().container_id;
        let (
            bundle_dir,
            rootfs_path,
            log_path,
            ports_spec,
            is_bridge_net,
            is_wasm,
            wasm_args,
            wasm_envs,
            wasm_fuel,
            wasm_mem_mb,
        ) = {
            let mut store = self.store.write().await;
            let ctr = store
                .containers
                .get_mut(&id)
                .ok_or_else(|| Status::not_found(format!("container '{id}' not found")))?;
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
            let is_wasm = Self::is_wasm_container(&ctr.config.annotations, &ctr.rootfs_path);
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
            let ret = (
                ctr.bundle_dir.clone(),
                ctr.rootfs_path.clone(),
                ctr.log_path.clone(),
                ctr.config.annotations.get("coral.ports").cloned(),
                is_bridge,
                is_wasm,
                wasm_args,
                wasm_envs,
                wasm_fuel,
                wasm_mem_mb,
            );
            self.persist_state(&store);
            ret
        };

        if is_wasm {
            self.spawn_wasm_container(
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

        let runtime_root = self.runtime_root();

        if let Some(raw_ports) = ports_spec {
            let handles =
                spawn_port_forwarders(&id, &raw_ports, is_bridge_net, &runtime_root).await?;
            if !handles.is_empty() {
                self.store
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
            state_dir: self.cfg.state_dir.clone(),
            coral_bin: self.cfg.coral_bin.clone(),
            store: self.store.clone(),
        });
        if let Some(prev) = self.store.write().await.supervisors.insert(id, sup_handle) {
            prev.abort();
        }

        Ok(Response::new(StartContainerResponse {}))
    }

    async fn stop_container(
        &self,
        request: Request<StopContainerRequest>,
    ) -> Result<Response<StopContainerResponse>, Status> {
        let id = request.into_inner().container_id;
        {
            let mut store = self.store.write().await;
            let ctr = store
                .containers
                .get_mut(&id)
                .ok_or_else(|| Status::not_found(format!("container '{id}' not found")))?;
            ctr.stopped_by_user = true;
            self.persist_state(&store);
        }

        let _ = Command::new(&self.cfg.coral_bin)
            .arg("--root")
            .arg(self.runtime_root())
            .arg("kill")
            .arg(&id)
            .arg("SIGTERM")
            .output()
            .await;

        let mut store = self.store.write().await;
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
        self.persist_state(&store);
        Ok(Response::new(StopContainerResponse {}))
    }

    async fn remove_container(
        &self,
        request: Request<RemoveContainerRequest>,
    ) -> Result<Response<RemoveContainerResponse>, Status> {
        let id = request.into_inner().container_id;
        let removed = {
            let mut store = self.store.write().await;
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
                .ok_or_else(|| Status::not_found(format!("container '{id}' not found")))?;
            self.persist_state(&store);
            removed
        };

        let _ = Command::new(&self.cfg.coral_bin)
            .arg("--root")
            .arg(self.runtime_root())
            .arg("delete")
            .arg("--force")
            .arg(&id)
            .output()
            .await;
        let _ = images::remove_dir_force(&removed.bundle_dir);
        let _ = fs::remove_file(&removed.log_path);
        Ok(Response::new(RemoveContainerResponse {}))
    }

    async fn list_containers(
        &self,
        request: Request<ListContainersRequest>,
    ) -> Result<Response<ListContainersResponse>, Status> {
        let filter = request.into_inner().filter;
        let store = self.store.read().await;
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

    async fn container_status(
        &self,
        request: Request<ContainerStatusRequest>,
    ) -> Result<Response<ContainerStatusResponse>, Status> {
        let req = request.into_inner();
        let store = self.store.read().await;
        let ctr = store
            .containers
            .get(&req.container_id)
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
                    &self.runtime_root(),
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

    async fn exec_sync(
        &self,
        request: Request<ExecSyncRequest>,
    ) -> Result<Response<ExecSyncResponse>, Status> {
        let req = request.into_inner();
        if req.cmd.is_empty() {
            return Err(Status::invalid_argument("exec command must not be empty"));
        }

        let (bundle_dir, is_running) = {
            let store = self.store.read().await;
            let ctr = store.containers.get(&req.container_id).ok_or_else(|| {
                Status::not_found(format!("container '{}' not found", req.container_id))
            })?;
            (
                ctr.bundle_dir.clone(),
                ctr.state == CriContainerState::ContainerRunning,
            )
        };

        // 1. Live container: enter its real running namespaces via `setns()`!
        if is_running {
            let mut cmd = Command::new(&self.cfg.coral_bin);
            cmd.arg("--root")
                .arg(self.runtime_root())
                .arg("exec")
                .arg("--local")
                .arg(&req.container_id)
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

        let exec_id = self.next_id("exec");
        let exec_bundle = self.cfg.state_dir.join("exec-bundles").join(&exec_id);
        fs::create_dir_all(&exec_bundle)
            .map_err(|e| Status::internal(format!("create exec bundle dir: {e}")))?;
        spec.to_file(exec_bundle.join("config.json"))
            .map_err(|e| Status::internal(format!("write exec config.json: {e}")))?;

        let out = Command::new(&self.cfg.coral_bin)
            .arg("--root")
            .arg(self.runtime_root())
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

    async fn exec_stream(
        &self,
        request: Request<Streaming<ExecStreamRequest>>,
    ) -> Result<Response<Self::ExecStreamStream>, Status> {
        let stream = self.handle_exec_stream(request.into_inner()).await?;
        Ok(Response::new(stream))
    }
}
