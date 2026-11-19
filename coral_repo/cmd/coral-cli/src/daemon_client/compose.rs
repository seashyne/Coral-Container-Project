//! `coral compose` CLI commands (`up`, `down`, `ps`, `logs`) managing multi-container stacks
//! defined in `coral-compose.yml` / `docker-compose.yml` via `coral-daemon` CRI v1.

use super::{connect_channel, container_state_label, short_id};
use crate::daemon_client::images::cmd_build;
use anyhow::{Context, Result, bail};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerConfig, ContainerFilter, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ImageSpec, KeyValue, LinuxContainerConfig, LinuxContainerResources,
    ListContainersRequest, ListPodSandboxRequest, PodSandboxConfig, PodSandboxFilter,
    PodSandboxMetadata, RemoveContainerRequest, RemovePodSandboxRequest, RunPodSandboxRequest,
    StartContainerRequest, StopContainerRequest, StopPodSandboxRequest,
};
use coral_daemon::compose::ComposeSpec;
use coral_runtime::rootfs::{default_volumes_dir, ensure_named_volume, remove_named_volume};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const DEFAULT_COMPOSE_FILES: &[&str] = &[
    "coral-compose.yml",
    "coral-compose.yaml",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

fn resolve_compose_file(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.is_file() {
            bail!("compose file '{}' not found", p.display());
        }
        return Ok(p.to_path_buf());
    }
    for name in DEFAULT_COMPOSE_FILES {
        let candidate = Path::new(name);
        if candidate.is_file() {
            return Ok(candidate.to_path_buf());
        }
    }
    bail!(
        "no compose file found in current directory (expected one of: {})",
        DEFAULT_COMPOSE_FILES.join(", ")
    )
}

fn resolve_project_name(
    cli_project: Option<&str>,
    spec: Option<&ComposeSpec>,
    compose_file: Option<&Path>,
) -> String {
    if let Some(p) = cli_project.map(str::trim).filter(|s| !s.is_empty()) {
        return p.to_string();
    }
    if let Some(p) = spec.and_then(|s| s.project_name.as_deref())
        && !p.trim().is_empty()
    {
        return p.trim().to_string();
    }
    let dir_name = compose_file
        .and_then(|f| std::fs::canonicalize(f).ok())
        .and_then(|abs| abs.parent().map(Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "coral-stack".into());
    let slug: String = dir_name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "coral-stack".into()
    } else {
        trimmed.to_string()
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn cmd_compose_up(
    addr: &str,
    state_root: &Path,
    images_dir: &Path,
    file: Option<&Path>,
    project_override: Option<&str>,
    detach: bool,
    force_build: bool,
) -> Result<()> {
    let compose_path = resolve_compose_file(file)?;
    let spec = ComposeSpec::from_file(&compose_path)?;
    let project = resolve_project_name(project_override, Some(&spec), Some(&compose_path));

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);

    // 1. Provision named volumes declared or referenced by the stack.
    let vol_root = default_volumes_dir();
    for vol in &spec.volumes {
        ensure_named_volume(&vol_root, vol)
            .with_context(|| format!("provision compose volume '{vol}'"))?;
    }

    println!(
        "Starting Compose stack '{project}' ({} service(s))...",
        spec.services.len()
    );

    let mut started: Vec<(String, String)> = Vec::new();
    for svc in &spec.services {
        let image_name = svc
            .image
            .clone()
            .unwrap_or_else(|| format!("{project}-{}:latest", svc.name));

        // 2. Build service image if `build:` is configured.
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
                )
                .await
                .with_context(|| format!("build compose service '{}'", svc.name))?;
            }
        }

        let labels = HashMap::from([
            ("managed-by".into(), "coral-compose".into()),
            ("coral.compose.project".into(), project.clone()),
            ("coral.compose.service".into(), svc.name.clone()),
        ]);

        let pod_id = rt
            .run_pod_sandbox(RunPodSandboxRequest {
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
            })
            .await
            .with_context(|| format!("run pod sandbox for service '{}'", svc.name))?
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

        let ctr_id = rt
            .create_container(CreateContainerRequest {
                pod_sandbox_id: pod_id,
                config: Some(ContainerConfig {
                    metadata: Some(ContainerMetadata {
                        name: format!("{project}_{}", svc.name),
                        attempt: 0,
                    }),
                    image: Some(ImageSpec {
                        image: image_name.clone(),
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
            short_id(&ctr_id),
            image_name
        );
        started.push((svc.name.clone(), ctr_id));
    }

    if detach {
        return Ok(());
    }

    // Foreground mode: wait until all stack containers finish (or timeout) and stream logs.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut printed = vec![false; started.len()];
    loop {
        let mut all_exited = true;
        for (idx, (svc_name, ctr_id)) in started.iter().enumerate() {
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

pub async fn cmd_compose_ps(
    addr: &str,
    file: Option<&Path>,
    project_override: Option<&str>,
) -> Result<()> {
    let spec_opt = resolve_compose_file(file)
        .ok()
        .and_then(|p| ComposeSpec::from_file(&p).ok().map(|s| (p, s)));
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let rows = rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                id: String::new(),
                pod_sandbox_id: String::new(),
                label_selector: HashMap::from([("coral.compose.project".into(), project.clone())]),
            }),
        })
        .await
        .context("list compose containers")?
        .into_inner()
        .containers;

    if rows.is_empty() {
        println!("No containers found for Compose project '{project}'.");
        return Ok(());
    }

    println!(
        "{:<16} {:<24} {:<24} {:<18} PORTS",
        "SERVICE", "CONTAINER ID", "IMAGE", "STATE"
    );
    for c in rows {
        let svc = c
            .labels
            .get("coral.compose.service")
            .map(String::as_str)
            .unwrap_or("-");
        let img = c.image.as_ref().map(|i| i.image.as_str()).unwrap_or("-");
        let ports = c
            .annotations
            .get("coral.ports")
            .map(String::as_str)
            .unwrap_or("-");
        let base_state = container_state_label(c.state);
        let state_col = match c.labels.get("coral.health.status").map(String::as_str) {
            Some(h) if !h.is_empty() && h != "none" => format!("{base_state} ({h})"),
            _ => base_state.to_string(),
        };
        println!(
            "{:<16} {:<24} {:<24} {:<18} {}",
            svc,
            short_id(&c.id),
            img,
            state_col,
            ports
        );
    }
    Ok(())
}

pub async fn cmd_compose_logs(
    addr: &str,
    file: Option<&Path>,
    project_override: Option<&str>,
    service_filter: Option<&str>,
) -> Result<()> {
    let spec_opt = resolve_compose_file(file)
        .ok()
        .and_then(|p| ComposeSpec::from_file(&p).ok().map(|s| (p, s)));
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let mut selector = HashMap::from([("coral.compose.project".into(), project.clone())]);
    if let Some(svc) = service_filter {
        selector.insert("coral.compose.service".into(), svc.to_string());
    }
    let rows = rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                id: String::new(),
                pod_sandbox_id: String::new(),
                label_selector: selector,
            }),
        })
        .await
        .context("list compose containers")?
        .into_inner()
        .containers;

    if rows.is_empty() {
        println!("No containers found for Compose project '{project}'.");
        return Ok(());
    }

    for c in rows {
        let svc = c
            .labels
            .get("coral.compose.service")
            .cloned()
            .unwrap_or_else(|| c.id.clone());
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: c.id,
                verbose: true,
            })
            .await?
            .into_inner();
        if let Some(log) = resp.info.get("logOutput") {
            for line in log.lines() {
                println!("[{svc}] {line}");
            }
        }
    }
    Ok(())
}

pub async fn cmd_compose_down(
    addr: &str,
    file: Option<&Path>,
    project_override: Option<&str>,
    remove_volumes: bool,
) -> Result<()> {
    let spec_opt = resolve_compose_file(file)
        .ok()
        .and_then(|p| ComposeSpec::from_file(&p).ok().map(|s| (p, s)));
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);

    let containers = rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                id: String::new(),
                pod_sandbox_id: String::new(),
                label_selector: HashMap::from([("coral.compose.project".into(), project.clone())]),
            }),
        })
        .await
        .context("list compose containers for down")?
        .into_inner()
        .containers;

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

    if remove_volumes
        && let Some((_, spec)) = &spec_opt
    {
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
