//! `coral compose up` and `coral compose build`.

use super::common::{resolve_compose_file, resolve_project_name, wait_for_service_condition};
use crate::daemon_client::connect_channel;
use crate::daemon_client::images::cmd_build;
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerState, ContainerStatusRequest, CreateContainerRequest, RunPodSandboxRequest,
    StartContainerRequest,
};
use coral_daemon::compose::DependsCondition;
use coral_runtime::rootfs::{default_volumes_dir, ensure_named_volume};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

#[allow(clippy::too_many_arguments)]
pub async fn cmd_compose_up(
    addr: &str,
    state_root: &Path,
    images_dir: &Path,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    profiles: &[String],
    services: &[String],
    detach: bool,
    force_build: bool,
    wait_healthy: bool,
    no_deps: bool,
) -> Result<()> {
    let compose_path = resolve_compose_file(file)?;
    let spec = coral_daemon::compose::ComposeSpec::from_file_with_env(&compose_path, env_file)?;
    let project = resolve_project_name(project_override, Some(&spec), Some(&compose_path));
    let selected = spec.select_services(services, profiles, no_deps)?;

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);

    let vol_root = default_volumes_dir();
    for vol in &spec.volumes {
        ensure_named_volume(&vol_root, vol)
            .with_context(|| format!("provision compose volume '{vol}'"))?;
    }

    println!(
        "Starting Compose stack '{project}' ({} service(s))...",
        selected.len()
    );

    let mut started_map: HashMap<String, String> = HashMap::new();
    let mut started: Vec<(String, String, bool)> = Vec::new();

    for svc in selected {
        for dep in &svc.depends_on_conditions {
            if let Some(dep_cid) = started_map.get(&dep.service)
                && dep.condition != DependsCondition::ServiceStarted
            {
                wait_for_service_condition(&mut rt, &dep.service, dep_cid, dep.condition, 90)
                    .await?;
            }
        }

        let image_name = svc
            .image
            .clone()
            .unwrap_or_else(|| format!("{project}-{}:latest", svc.name));

        if let Some(ctx_str) = &svc.build_context {
            let ctx_path = Path::new(ctx_str);
            let df_path = ctx_path.join(svc.dockerfile.as_deref().unwrap_or("Dockerfile"));
            if force_build || svc.image.is_none() {
                cmd_build(
                    addr,
                    state_root,
                    images_dir,
                    &image_name,
                    &df_path,
                    ctx_path,
                    false,
                    false,
                    svc.build_target.as_deref(),
                    &svc.build_args,
                )
                .await
                .with_context(|| format!("build compose service '{}'", svc.name))?;
            }
        }

        let launch = svc.to_launch_spec(&project, &image_name);
        let pod_id = rt
            .run_pod_sandbox(RunPodSandboxRequest {
                config: Some(launch.pod_config),
                runtime_handler: "coral".into(),
            })
            .await
            .with_context(|| format!("run pod sandbox for service '{}'", svc.name))?
            .into_inner()
            .pod_sandbox_id;

        let ctr_id = rt
            .create_container(CreateContainerRequest {
                pod_sandbox_id: pod_id,
                config: Some(launch.container_config),
                sandbox_config: None,
            })
            .await
            .with_context(|| format!("create container for service '{}'", svc.name))?
            .into_inner()
            .container_id;

        rt.start_container(StartContainerRequest {
            container_id: ctr_id.clone(),
        })
        .await
        .with_context(|| format!("start container for service '{}'", svc.name))?;

        println!(
            "  [+] {:<16} -> {} ({})",
            svc.name,
            crate::daemon_client::short_id(&ctr_id),
            image_name
        );
        started_map.insert(svc.name.clone(), ctr_id.clone());
        started.push((svc.name.clone(), ctr_id, svc.health_cmd.is_some()));
    }

    if wait_healthy {
        for (svc_name, ctr_id, has_health) in &started {
            if *has_health {
                wait_for_service_condition(
                    &mut rt,
                    svc_name,
                    ctr_id,
                    DependsCondition::ServiceHealthy,
                    90,
                )
                .await?;
            }
        }
    }

    if detach {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut printed = vec![false; started.len()];
    loop {
        let mut all_exited = true;
        for (idx, (svc_name, ctr_id, _)) in started.iter().enumerate() {
            if printed[idx] {
                continue;
            }
            let resp = rt
                .container_status(ContainerStatusRequest {
                    container_id: ctr_id.clone(),
                    verbose: true,
                })
                .await?
                .into_inner();
            if let Some(st) = resp.status
                && st.state == ContainerState::ContainerExited as i32
            {
                if let Some(log) = resp.info.get("logOutput") {
                    for line in log.lines() {
                        println!("[{svc_name}] {line}");
                    }
                }
                printed[idx] = true;
            } else {
                all_exited = false;
            }
        }
        if all_exited || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_compose_build(
    addr: &str,
    state_root: &Path,
    images_dir: &Path,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    profiles: &[String],
    services: &[String],
    no_cache: bool,
) -> Result<()> {
    let compose_path = resolve_compose_file(file)?;
    let spec = coral_daemon::compose::ComposeSpec::from_file_with_env(&compose_path, env_file)?;
    let project = resolve_project_name(project_override, Some(&spec), Some(&compose_path));
    let selected = spec.select_services(services, profiles, true)?;

    let mut built_count = 0usize;
    for svc in selected {
        let Some(ctx_str) = &svc.build_context else {
            continue;
        };
        let image_name = svc
            .image
            .clone()
            .unwrap_or_else(|| format!("{project}-{}:latest", svc.name));
        let ctx_path = Path::new(ctx_str);
        let df_path = ctx_path.join(svc.dockerfile.as_deref().unwrap_or("Dockerfile"));
        cmd_build(
            addr,
            state_root,
            images_dir,
            &image_name,
            &df_path,
            ctx_path,
            false,
            no_cache,
            svc.build_target.as_deref(),
            &svc.build_args,
        )
        .await
        .with_context(|| format!("build compose service '{}'", svc.name))?;
        built_count += 1;
    }

    if built_count == 0 {
        println!("No services with a `build:` directive matched in '{project}'.");
    }
    Ok(())
}
