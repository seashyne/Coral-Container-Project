//! `coral run --daemon` — managed container launch through `coral-daemon`.

use crate::daemon_client::connect_channel;
use crate::units::PullPolicy;
use anyhow::{Context, Result};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ImageSpec, ImageStatusRequest, KeyValue, LinuxContainerConfig,
    LinuxContainerResources, PodSandboxConfig, PodSandboxMetadata, PullImageRequest,
    RemoveContainerRequest, RemovePodSandboxRequest, RunPodSandboxRequest, StartContainerRequest,
};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::{Duration, Instant};

pub struct DaemonRunOptions {
    pub name: String,
    pub image: String,
    pub command: Vec<String>,
    pub envs: Vec<String>,
    pub volumes: Vec<String>,
    pub ports: Vec<String>,
    pub network: String,
    pub dns: Vec<String>,
    pub extra_hosts: Vec<String>,
    pub network_aliases: Vec<String>,
    pub devices: Vec<String>,
    pub gpus: Option<String>,
    pub log_driver: String,
    pub privileged: bool,
    pub no_overlay: bool,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub entrypoint: Option<String>,
    pub hostname: Option<String>,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub restart: String,
    pub health_cmd: Option<String>,
    pub health_interval_secs: u64,
    pub health_retries: u32,
    pub detach: bool,
    pub rm: bool,
    pub pull_policy: PullPolicy,
}

pub async fn cmd_run_via_daemon(addr: &str, opts: DaemonRunOptions) -> Result<ExitCode> {
    let ch = connect_channel(addr, 5).await?;
    let mut img_client = ImageServiceClient::new(ch.clone());
    let mut rt = RuntimeServiceClient::new(ch);

    if opts.pull_policy != PullPolicy::Never && !std::path::Path::new(&opts.image).is_dir() {
        let should_pull = if opts.pull_policy == PullPolicy::Always {
            true
        } else {
            img_client
                .image_status(ImageStatusRequest {
                    image: Some(ImageSpec {
                        image: opts.image.clone(),
                        annotations: HashMap::new(),
                    }),
                })
                .await
                .map(|r| r.into_inner().image.is_none())
                .unwrap_or(true)
        };
        if should_pull {
            eprintln!("Unable to find image '{}' locally", opts.image);
            eprintln!("Pulling '{}' via coral-daemon...", opts.image);
            img_client
                .pull_image(PullImageRequest {
                    image: Some(ImageSpec {
                        image: opts.image.clone(),
                        annotations: HashMap::new(),
                    }),
                })
                .await
                .with_context(|| format!("pull image '{}'", opts.image))?;
        }
    }

    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: format!("{}-pod", opts.name),
                    uid: opts.name.clone(),
                    namespace: "coral-cli".into(),
                    attempt: 0,
                }),
                hostname: opts.hostname.unwrap_or_else(|| opts.name.clone()),
                log_directory: String::new(),
                labels: HashMap::from([("managed-by".into(), "coral-cli".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .context("run_pod_sandbox")?
        .into_inner()
        .pod_sandbox_id;

    let envs = opts
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

    let (command, args) = if opts.command.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (vec![opts.command[0].clone()], opts.command[1..].to_vec())
    };

    let linux = if opts.memory_mb > 0 || opts.cpu_percent > 0 || opts.pids_limit > 0 {
        Some(LinuxContainerConfig {
            resources: Some(LinuxContainerResources {
                cpu_period: if opts.cpu_percent > 0 { 100_000 } else { 0 },
                cpu_quota: opts.cpu_percent * 1_000,
                cpu_shares: if opts.cpu_percent > 0 { 1024 } else { 0 },
                memory_limit_in_bytes: opts.memory_mb * 1024 * 1024,
                pids_limit: opts.pids_limit,
            }),
        })
    } else {
        None
    };

    let mut annotations = HashMap::new();
    if !opts.volumes.is_empty() {
        annotations.insert("coral.mounts".into(), opts.volumes.join("\n"));
    }
    if !opts.ports.is_empty() {
        annotations.insert("coral.ports".into(), opts.ports.join(","));
    }
    if !opts.network.trim().is_empty() {
        annotations.insert("coral.network".into(), opts.network.trim().to_string());
    }
    if !opts.dns.is_empty() {
        annotations.insert("coral.dns".into(), opts.dns.join(","));
    }
    if !opts.extra_hosts.is_empty() {
        annotations.insert("coral.extra_hosts".into(), opts.extra_hosts.join("\n"));
    }
    if !opts.network_aliases.is_empty() {
        annotations.insert(
            "coral.network.aliases".into(),
            opts.network_aliases.join(","),
        );
    }
    if !opts.devices.is_empty() {
        annotations.insert("coral.devices".into(), opts.devices.join("\n"));
    }
    if let Some(g) = &opts.gpus
        && !g.trim().is_empty()
    {
        annotations.insert("coral.gpus".into(), g.trim().to_string());
    }
    if !opts.log_driver.trim().is_empty() && !opts.log_driver.eq_ignore_ascii_case("raw") {
        annotations.insert("coral.log.driver".into(), opts.log_driver.trim().to_string());
    }
    if opts.privileged {
        annotations.insert("coral.privileged".into(), "true".into());
    }
    if opts.no_overlay {
        annotations.insert("coral.overlay".into(), "false".into());
    }
    if let Some(u) = &opts.user
        && !u.trim().is_empty()
    {
        annotations.insert("coral.user".into(), u.trim().to_string());
    }
    if let Some(ep) = &opts.entrypoint {
        annotations.insert("coral.entrypoint".into(), ep.clone());
    }
    if !opts.restart.trim().is_empty() && !opts.restart.eq_ignore_ascii_case("no") {
        annotations.insert("coral.restart".into(), opts.restart.trim().to_string());
    }
    if let Some(hc) = &opts.health_cmd
        && !hc.trim().is_empty()
    {
        annotations.insert("coral.health.cmd".into(), hc.trim().to_string());
        annotations.insert(
            "coral.health.interval_ms".into(),
            opts.health_interval_secs
                .saturating_mul(1_000)
                .max(500)
                .to_string(),
        );
        annotations.insert(
            "coral.health.retries".into(),
            opts.health_retries.max(1).to_string(),
        );
    }

    let mut ctr_labels = HashMap::from([("managed-by".into(), "coral-cli".into())]);
    if !opts.ports.is_empty() {
        ctr_labels.insert("coral.ports".into(), opts.ports.join(","));
    }

    let ctr_id = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: opts.name,
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: opts.image,
                    annotations: HashMap::new(),
                }),
                command,
                args,
                working_dir: opts.workdir.unwrap_or_default(),
                envs,
                labels: ctr_labels,
                annotations,
                log_path: String::new(),
                linux,
            }),
            sandbox_config: None,
        })
        .await
        .context("create_container")?
        .into_inner()
        .container_id;

    rt.start_container(StartContainerRequest {
        container_id: ctr_id.clone(),
    })
    .await
    .context("start_container")?;

    if opts.detach {
        println!("{ctr_id}");
        return Ok(ExitCode::SUCCESS);
    }

    let deadline = Instant::now() + Duration::from_secs(300);
    let mut poll_ms = 5u64;
    loop {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: ctr_id.clone(),
                verbose: true,
            })
            .await
            .context("container_status")?
            .into_inner();
        if let Some(status) = resp.status
            && status.state == ContainerState::ContainerExited as i32
        {
            if let Some(log) = resp.info.get("logOutput")
                && !log.is_empty()
            {
                print!("{log}");
            }
            if status.exit_code != 0 && !status.message.is_empty() {
                eprintln!("{}", status.message);
            }
            if opts.rm {
                let _ = rt
                    .remove_container(RemoveContainerRequest {
                        container_id: ctr_id.clone(),
                    })
                    .await;
                let _ = rt
                    .remove_pod_sandbox(RemovePodSandboxRequest {
                        pod_sandbox_id: pod_id.clone(),
                    })
                    .await;
            }
            let code = u8::try_from(status.exit_code.clamp(0, 255)).unwrap_or(1);
            return Ok(ExitCode::from(code));
        }
        if Instant::now() >= deadline {
            eprintln!("Timed out waiting for container '{ctr_id}' (still running in daemon)");
            return Ok(ExitCode::SUCCESS);
        }
        tokio::time::sleep(Duration::from_millis(poll_ms)).await;
        poll_ms = (poll_ms * 2).min(50);
    }
}
