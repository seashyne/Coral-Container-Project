//! `coral compose` container lifecycle commands (`start`, `stop`, `restart`, `exec`, `down`).

use super::common::{list_project_containers, load_compose_spec_optional, resolve_project_name};
use crate::daemon_client::connect_channel;
use crate::daemon_client::containers::cmd_exec;
use crate::daemon_client::short_id;
use anyhow::{Context, Result, bail};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerState, ListPodSandboxRequest, PodSandboxFilter, RemoveContainerRequest,
    RemovePodSandboxRequest, StartContainerRequest, StopContainerRequest, StopPodSandboxRequest,
};
use coral_cgroups::CgroupManager;
use coral_runtime::rootfs::{default_volumes_dir, remove_named_volume};
use coral_runtime::{ContainerManager, ExecOptions, StateStore};
use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

pub async fn cmd_compose_start(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    services: &[String],
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let rows = list_project_containers(&mut rt, &project, services).await?;
    if rows.is_empty() {
        bail!(
            "no containers found for Compose project '{project}' (run `coral compose up -d` first)"
        );
    }
    for c in rows {
        if c.state != ContainerState::ContainerRunning as i32 {
            rt.start_container(StartContainerRequest {
                container_id: c.id.clone(),
            })
            .await
            .with_context(|| format!("start compose container '{}'", c.id))?;
            let svc = c
                .labels
                .get("coral.compose.service")
                .map(String::as_str)
                .unwrap_or(&c.id);
            println!("Started {svc} ({})", short_id(&c.id));
        }
    }
    Ok(())
}

pub async fn cmd_compose_stop(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    services: &[String],
    timeout_secs: i64,
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let rows = list_project_containers(&mut rt, &project, services).await?;
    for c in rows {
        if c.state == ContainerState::ContainerRunning as i32 {
            rt.stop_container(StopContainerRequest {
                container_id: c.id.clone(),
                timeout: timeout_secs,
            })
            .await
            .with_context(|| format!("stop compose container '{}'", c.id))?;
            let svc = c
                .labels
                .get("coral.compose.service")
                .map(String::as_str)
                .unwrap_or(&c.id);
            println!("Stopped {svc} ({})", short_id(&c.id));
        }
    }
    Ok(())
}

pub async fn cmd_compose_restart(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    services: &[String],
    timeout_secs: i64,
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let rows = list_project_containers(&mut rt, &project, services).await?;
    if rows.is_empty() {
        bail!("no containers found for Compose project '{project}'");
    }
    for c in rows {
        if c.state == ContainerState::ContainerRunning as i32 {
            rt.stop_container(StopContainerRequest {
                container_id: c.id.clone(),
                timeout: timeout_secs,
            })
            .await
            .with_context(|| format!("stop compose container '{}'", c.id))?;
        }
        rt.start_container(StartContainerRequest {
            container_id: c.id.clone(),
        })
        .await
        .with_context(|| format!("restart compose container '{}'", c.id))?;
        let svc = c
            .labels
            .get("coral.compose.service")
            .map(String::as_str)
            .unwrap_or(&c.id);
        println!("Restarted {svc} ({})", short_id(&c.id));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_compose_exec(
    addr: &str,
    state_root: &Path,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    service: &str,
    interactive: bool,
    tty: bool,
    envs: Vec<String>,
    workdir: Option<String>,
    timeout: i64,
    cmd: Vec<String>,
) -> Result<ExitCode> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let rows = list_project_containers(&mut rt, &project, &[service.to_string()]).await?;
    let target = rows
        .into_iter()
        .find(|c| c.state == ContainerState::ContainerRunning as i32)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no running container found for compose service '{service}' in project '{project}'"
            )
        })?;

    if interactive || tty || !envs.is_empty() || workdir.is_some() {
        let daemon_rt = state_root.join("daemon/runtime-state");
        let rt_dir = if StateStore::new(&daemon_rt).load(&target.id).is_ok() {
            daemon_rt
        } else {
            state_root.to_path_buf()
        };
        let mgr = ContainerManager::new(rt_dir, CgroupManager::detect());
        let code = mgr.exec(
            &target.id,
            &ExecOptions {
                cmd,
                env: envs,
                cwd: workdir,
                tty,
                interactive,
            },
        )?;
        let byte = u8::try_from(code.clamp(0, 255)).unwrap_or(1);
        return Ok(ExitCode::from(byte));
    }

    cmd_exec(addr, &target.id, timeout, cmd).await
}

pub async fn cmd_compose_down(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    remove_volumes: bool,
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);

    let containers = list_project_containers(&mut rt, &project, &[]).await?;
    for c in &containers {
        let _ = rt
            .stop_container(StopContainerRequest {
                container_id: c.id.clone(),
                timeout: 10,
            })
            .await;
        rt.remove_container(RemoveContainerRequest {
            container_id: c.id.clone(),
        })
        .await
        .with_context(|| format!("remove compose container '{}'", c.id))?;
    }

    let pods = rt
        .list_pod_sandbox(ListPodSandboxRequest {
            filter: Some(PodSandboxFilter {
                id: String::new(),
                label_selector: HashMap::from([("coral.compose.project".into(), project.clone())]),
            }),
        })
        .await
        .context("list compose pods for down")?
        .into_inner()
        .items;

    for p in &pods {
        let _ = rt
            .stop_pod_sandbox(StopPodSandboxRequest {
                pod_sandbox_id: p.id.clone(),
            })
            .await;
        let _ = rt
            .remove_pod_sandbox(RemovePodSandboxRequest {
                pod_sandbox_id: p.id.clone(),
            })
            .await;
    }

    if remove_volumes && let Some((_, spec)) = &spec_opt {
        let vol_root = default_volumes_dir();
        for vol in &spec.volumes {
            let _ = remove_named_volume(&vol_root, vol);
        }
    }

    println!(
        "Stopped & removed Compose project '{project}' ({} container(s), {} pod(s)).",
        containers.len(),
        pods.len()
    );
    Ok(())
}
