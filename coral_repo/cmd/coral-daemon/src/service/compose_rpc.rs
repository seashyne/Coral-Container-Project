//! Server-side `coral compose` execution inside `CoralDaemonService` so both Coral Desktop
//! (Local, WSL2, or Remote) and gRPC clients can launch multi-container stacks atomically.

use super::CoralDaemonService;
use crate::compose::{ComposeSpec, DependsCondition};
use crate::images;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    ContainerState, ContainerStatusRequest, CreateContainerRequest, RunPodSandboxRequest,
    StartContainerRequest,
};
use coral_runtime::rootfs::{default_volumes_dir, ensure_named_volume};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
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
        let spec = ComposeSpec::from_yaml_with_dotenv(yaml, base_path)
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

        let selected = spec
            .select_services(&[], &[], false)
            .map_err(|e| Status::invalid_argument(format!("select compose services: {e:#}")))?;

        let mut started_map: HashMap<String, String> = HashMap::new();
        let mut started_ids = Vec::with_capacity(selected.len());
        for svc in selected {
            for dep in &svc.depends_on_conditions {
                if let Some(dep_cid) = started_map.get(&dep.service)
                    && dep.condition != DependsCondition::ServiceStarted
                {
                    self.wait_for_compose_dependency(&dep.service, dep_cid, dep.condition)
                        .await?;
                }
            }

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
                    target: svc.build_target.as_deref(),
                    build_args: &svc.build_args,
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

            let launch = svc.to_launch_spec(&project, &image_name);
            let pod_id = self
                .run_pod_sandbox(Request::new(RunPodSandboxRequest {
                    config: Some(launch.pod_config),
                    runtime_handler: "coral".into(),
                }))
                .await?
                .into_inner()
                .pod_sandbox_id;

            let ctr_id = self
                .create_container(Request::new(CreateContainerRequest {
                    pod_sandbox_id: pod_id,
                    config: Some(launch.container_config),
                    sandbox_config: None,
                }))
                .await?
                .into_inner()
                .container_id;

            self.start_container(Request::new(StartContainerRequest {
                container_id: ctr_id.clone(),
            }))
            .await?;

            started_map.insert(svc.name.clone(), ctr_id.clone());
            started_ids.push(ctr_id);
        }

        Ok(format!("{project}:{}", started_ids.join(",")))
    }

    #[allow(clippy::result_large_err)]
    async fn wait_for_compose_dependency(
        &self,
        dep_service: &str,
        dep_cid: &str,
        condition: DependsCondition,
    ) -> Result<(), Status> {
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let resp = self
                .container_status(Request::new(ContainerStatusRequest {
                    container_id: dep_cid.to_string(),
                    verbose: true,
                }))
                .await?
                .into_inner();
            let state = resp.status.as_ref().map(|s| s.state).unwrap_or(0);
            let exit_code = resp.status.as_ref().map(|s| s.exit_code).unwrap_or(-1);
            let health = resp
                .info
                .get("healthStatus")
                .map(String::as_str)
                .unwrap_or("none");

            match condition {
                DependsCondition::ServiceStarted => return Ok(()),
                DependsCondition::ServiceHealthy => {
                    if health == "healthy" {
                        return Ok(());
                    }
                    if state == ContainerState::ContainerExited as i32 {
                        return Err(Status::failed_precondition(format!(
                            "dependency service '{dep_service}' exited with code {exit_code} before becoming healthy"
                        )));
                    }
                    if health == "unhealthy" {
                        return Err(Status::failed_precondition(format!(
                            "dependency service '{dep_service}' reported unhealthy"
                        )));
                    }
                }
                DependsCondition::ServiceCompletedSuccessfully => {
                    if state == ContainerState::ContainerExited as i32 {
                        if exit_code == 0 {
                            return Ok(());
                        }
                        return Err(Status::failed_precondition(format!(
                            "dependency service '{dep_service}' exited with code {exit_code}"
                        )));
                    }
                }
            }

            if Instant::now() >= deadline {
                return Err(Status::deadline_exceeded(format!(
                    "timed out waiting for dependency service '{dep_service}'"
                )));
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
    }
}
