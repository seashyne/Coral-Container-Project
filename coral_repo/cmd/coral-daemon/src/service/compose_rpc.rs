//! Server-side `coral compose` execution inside `CoralDaemonService` so both Coral Desktop
//! (Local, WSL2, or Remote) and gRPC clients can launch multi-container stacks atomically.

use super::CoralDaemonService;
use crate::compose::ComposeSpec;
use crate::images;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    ContainerConfig, ContainerMetadata, CreateContainerRequest, ImageSpec, KeyValue,
    LinuxContainerConfig, LinuxContainerResources, PodSandboxConfig, PodSandboxMetadata,
    RunPodSandboxRequest, StartContainerRequest,
};
use coral_runtime::rootfs::{default_volumes_dir, ensure_named_volume};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tonic::{Request, Status};

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn run_compose_stack_yaml(
        &self,
        yaml: &str,
        project_override: Option<&str>,
        base_dir: Option<&str>,
    ) -> Result<String, Status> {
        let base_path = base_dir
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(Path::new);
        let spec = ComposeSpec::from_yaml_str(yaml, base_path)
            .map_err(|e| Status::invalid_argument(format!("parse compose YAML: {e:#}")))?;

        let project = project_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| spec.project_name.clone())
            .unwrap_or_else(|| "coral-stack".to_string());

        let vol_root = default_volumes_dir();
        for vol in &spec.volumes {
            ensure_named_volume(&vol_root, vol).map_err(|e| {
                Status::internal(format!("provision compose volume '{vol}': {e}"))
            })?;
        }

        let mut started_ids = Vec::with_capacity(spec.services.len());
        for svc in &spec.services {
            let image_name = svc
                .image
                .clone()
                .unwrap_or_else(|| format!("{project}-{}:latest", svc.name));

            if let Some(ctx_str) = &svc.build_context {
                let ctx_path = Path::new(ctx_str);
                let df_name = svc.dockerfile.as_deref().unwrap_or("Dockerfile");
                let df_path = ctx_path.join(df_name);
                let df_content = fs::read_to_string(&df_path).map_err(|e| {
                    Status::invalid_argument(format!(
                        "read Dockerfile '{}' for service '{}': {e}",
                        df_path.display(),
                        svc.name
                    ))
                })?;

                {
                    let store = self.store.read().await;
                    self.persist_images(&store.images);
                }

                let built = crate::builder::build_image(crate::builder::BuildOptions {
                    tag: &image_name,
                    dockerfile_content: &df_content,
                    context_dir: Some(ctx_path),
                    images_dir: &self.images_dir,
                    state_dir: &self.cfg.state_dir,
                    coral_bin: &self.cfg.coral_bin,
                    no_cache: false,
                })
                .await
                .map_err(|e| {
                    Status::internal(format!("build compose service '{}': {e:#}", svc.name))
                })?;

                let mut store = self.store.write().await;
                store.images = images::load_index(&self.images_dir);
                let orphaned = images::upsert_image(&mut store.images, built);
                for old in orphaned {
                    self.cleanup_unused_managed_dir(&store.images, &old);
                }
                self.persist_images(&store.images);
            }

            let labels = HashMap::from([
                ("managed-by".into(), "coral-compose".into()),
                ("coral.compose.project".into(), project.clone()),
                ("coral.compose.service".into(), svc.name.clone()),
            ]);

            let pod_id = self
                .run_pod_sandbox(Request::new(RunPodSandboxRequest {
                    config: Some(PodSandboxConfig {
                        metadata: Some(PodSandboxMetadata {
                            name: format!("{project}-{}-pod", svc.name),
                            uid: format!("{project}-{}", svc.name),
                            namespace: project.clone(),
                            attempt: 0,
                        }),
                        hostname: svc.hostname.clone().unwrap_or_else(|| svc.name.clone()),
                        log_directory: String::new(),
                        labels: labels.clone(),
                        annotations: HashMap::new(),
                    }),
                    runtime_handler: "coral".into(),
                }))
                .await?
                .into_inner()
                .pod_sandbox_id;

            let envs = svc
                .envs
                .iter()
                .filter_map(|e| {
                    e.split_once('=').map(|(k, v)| KeyValue {
                        key: k.trim().to_string(),
                        value: v.to_string(),
                    })
                })
                .filter(|kv| !kv.key.is_empty())
                .collect();

            let (command, args) = if svc.command.is_empty() {
                (vec!["/bin/sh".into()], Vec::new())
            } else {
                (vec![svc.command[0].clone()], svc.command[1..].to_vec())
            };

            let mut annotations = HashMap::new();
            if !svc.volumes.is_empty() {
                annotations.insert("coral.mounts".into(), svc.volumes.join("\n"));
            }
            if !svc.ports.is_empty() {
                annotations.insert("coral.ports".into(), svc.ports.join(","));
            }
            if !svc.network.trim().is_empty() {
                annotations.insert("coral.network".into(), svc.network.trim().to_string());
            }
            if !svc.dns.is_empty() {
                annotations.insert("coral.dns".into(), svc.dns.join(","));
            }
            if !svc.extra_hosts.is_empty() {
                annotations.insert("coral.extra_hosts".into(), svc.extra_hosts.join("\n"));
            }
            if !svc.restart.trim().is_empty() && !svc.restart.eq_ignore_ascii_case("no") {
                annotations.insert("coral.restart".into(), svc.restart.trim().to_string());
            }
            if let Some(hc) = &svc.health_cmd
                && !hc.trim().is_empty()
            {
                annotations.insert("coral.health.cmd".into(), hc.trim().to_string());
                annotations.insert(
                    "coral.health.interval_ms".into(),
                    svc.health_interval_ms.to_string(),
                );
                annotations.insert(
                    "coral.health.retries".into(),
                    svc.health_retries.to_string(),
                );
            }

            let linux = if svc.memory_mb > 0 || svc.cpu_percent > 0 || svc.pids_limit > 0 {
                Some(LinuxContainerConfig {
                    resources: Some(LinuxContainerResources {
                        cpu_period: if svc.cpu_percent > 0 { 100_000 } else { 0 },
                        cpu_quota: svc.cpu_percent * 1_000,
                        cpu_shares: if svc.cpu_percent > 0 { 1024 } else { 0 },
                        memory_limit_in_bytes: svc.memory_mb * 1024 * 1024,
                        pids_limit: svc.pids_limit,
                    }),
                })
            } else {
                None
            };

            let ctr_id = self
                .create_container(Request::new(CreateContainerRequest {
                    pod_sandbox_id: pod_id,
                    config: Some(ContainerConfig {
                        metadata: Some(ContainerMetadata {
                            name: format!("{project}_{}", svc.name),
                            attempt: 0,
                        }),
                        image: Some(ImageSpec {
                            image: image_name,
                            annotations: HashMap::new(),
                        }),
                        command,
                        args,
                        working_dir: svc.working_dir.clone(),
                        envs,
                        labels,
                        annotations,
                        log_path: String::new(),
                        linux,
                    }),
                    sandbox_config: None,
                }))
                .await?
                .into_inner()
                .container_id;

            self.start_container(Request::new(StartContainerRequest {
                container_id: ctr_id.clone(),
            }))
            .await?;

            started_ids.push(ctr_id);
        }

        Ok(format!("{project}:{}", started_ids.join(",")))
    }
}
