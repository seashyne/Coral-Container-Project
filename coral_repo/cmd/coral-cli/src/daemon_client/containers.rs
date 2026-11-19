//! Container lifecycle commands (`run --daemon`, `ps`, `logs`, `exec`, `stop`, `rm`, `diff`)
//! over CRI v1 `RuntimeService` gRPC.

use super::{
    connect_channel, container_state_label, format_nanos, short_id, try_connect_channel,
};
use crate::catalog;
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ExecSyncRequest, ImageSpec, KeyValue, LinuxContainerConfig,
    LinuxContainerResources, ListContainersRequest, PodSandboxConfig, PodSandboxMetadata,
    RemoveContainerRequest, RunPodSandboxRequest, StartContainerRequest, StopContainerRequest,
};
use coral_daemon::service::metrics::LiveContainerMetrics;
use std::collections::HashMap;
use std::io::Write;
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
    pub privileged: bool,
    pub no_overlay: bool,
    pub workdir: String,
    pub hostname: Option<String>,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub restart: String,
    pub health_cmd: Option<String>,
    pub health_interval_secs: u64,
    pub health_retries: u32,
    pub detach: bool,
}

pub async fn cmd_run_via_daemon(addr: &str, opts: DaemonRunOptions) -> Result<ExitCode> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);

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
        (vec!["/bin/sh".into()], Vec::new())
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
    if opts.privileged {
        annotations.insert("coral.privileged".into(), "true".into());
    }
    if opts.no_overlay {
        annotations.insert("coral.overlay".into(), "false".into());
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
            opts.health_interval_secs.saturating_mul(1_000).max(500).to_string(),
        );
        annotations.insert(
            "coral.health.retries".into(),
            opts.health_retries.max(1).to_string(),
        );
    }

    let ctr_id = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id,
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
                working_dir: opts.workdir,
                envs,
                labels: HashMap::from([("managed-by".into(), "coral-cli".into())]),
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

pub async fn cmd_ps(addr: &str) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let mut rows = rt
        .list_containers(ListContainersRequest { filter: None })
        .await
        .context("list_containers")?
        .into_inner()
        .containers;
    rows.sort_by_key(|c| std::cmp::Reverse(c.created_at));

    if rows.is_empty() {
        println!("No containers in coral-daemon ({addr}).");
        return Ok(());
    }

    println!(
        "{:<24} {:<18} {:<24} {:<18} CREATED",
        "CONTAINER ID", "NAME", "IMAGE", "STATE"
    );
    for c in rows {
        let name = c
            .metadata
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or(&c.id);
        let img = c.image.as_ref().map(|i| i.image.as_str()).unwrap_or("-");
        let base_state = container_state_label(c.state);
        let state_col = match c.labels.get("coral.health.status").map(String::as_str) {
            Some(h) if !h.is_empty() && h != "none" => format!("{base_state} ({h})"),
            _ => base_state.to_string(),
        };
        println!(
            "{:<24} {:<18} {:<24} {:<18} {}",
            short_id(&c.id),
            name,
            img,
            state_col,
            format_nanos(c.created_at)
        );
    }
    Ok(())
}

fn slice_tail_lines(log: &str, tail: Option<usize>) -> &str {
    let Some(n) = tail else {
        return log;
    };
    if n == 0 {
        return "";
    }
    let trimmed_end = log.strip_suffix('\n').unwrap_or(log);
    let mut newline_positions: Vec<usize> = trimmed_end.match_indices('\n').map(|(i, _)| i).collect();
    if newline_positions.len() < n {
        return log;
    }
    let start = newline_positions.swap_remove(newline_positions.len() - n) + 1;
    &log[start..]
}

pub async fn cmd_logs(
    addr: &str,
    id: &str,
    show_status: bool,
    follow: bool,
    tail: Option<usize>,
) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let resp = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await
        .with_context(|| format!("fetch logs for container '{id}'"))?
        .into_inner();

    if show_status && let Some(st) = resp.status.as_ref() {
        let restart = resp
            .info
            .get("restartPolicy")
            .map(String::as_str)
            .unwrap_or("no");
        let restarts = resp
            .info
            .get("restartCount")
            .map(String::as_str)
            .unwrap_or("0");
        let health = resp
            .info
            .get("healthStatus")
            .map(String::as_str)
            .unwrap_or("none");
        println!(
            "state={} exit_code={} reason={} restart={}({}) health={} started={} finished={}",
            container_state_label(st.state),
            st.exit_code,
            if st.reason.is_empty() { "-" } else { &st.reason },
            restart,
            restarts,
            health,
            format_nanos(st.started_at),
            format_nanos(st.finished_at)
        );
    }

    let initial = resp.info.get("logOutput").map(String::as_str).unwrap_or("");
    let to_print = slice_tail_lines(initial, tail);
    if !to_print.is_empty() {
        print!("{to_print}");
        let _ = std::io::stdout().flush();
    } else if !show_status && !follow {
        eprintln!("(no log output yet for '{id}')");
    }

    if !follow {
        return Ok(());
    }

    let mut printed_bytes = initial.len();
    if resp
        .status
        .as_ref()
        .is_some_and(|st| st.state == ContainerState::ContainerExited as i32)
    {
        return Ok(());
    }

    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let poll = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
            .with_context(|| format!("stream logs for container '{id}'"))?
            .into_inner();
        let current = poll.info.get("logOutput").map(String::as_str).unwrap_or("");
        if current.len() > printed_bytes && current.is_char_boundary(printed_bytes) {
            let chunk = &current[printed_bytes..];
            print!("{chunk}");
            let _ = std::io::stdout().flush();
            printed_bytes = current.len();
        } else if current.len() < printed_bytes {
            printed_bytes = current.len();
        }
        if poll
            .status
            .as_ref()
            .is_some_and(|st| st.state == ContainerState::ContainerExited as i32)
        {
            break;
        }
    }
    Ok(())
}

pub async fn cmd_stats(addr: &str, id: &str, watch: bool, interval_ms: u64) -> Result<bool> {
    let Some(ch) = try_connect_channel(addr).await else {
        return Ok(false);
    };
    let mut rt = RuntimeServiceClient::new(ch);
    let first = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await;
    let Ok(resp) = first else {
        return Ok(false);
    };
    let mut current = resp.into_inner();
    let step = Duration::from_millis(interval_ms.clamp(100, 60_000));

    println!(
        "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} SOURCE",
        "CONTAINER", "STATE", "HEALTH", "PID", "MEMORY", "CPU TIME", "PIDS", "RESTARTS"
    );

    loop {
        let st = current.status.as_ref();
        let state_str = st.map(|s| container_state_label(s.state)).unwrap_or("unknown");
        let health = current
            .info
            .get("healthStatus")
            .map(String::as_str)
            .unwrap_or("none");
        let restarts = current
            .info
            .get("restartCount")
            .map(String::as_str)
            .unwrap_or("0");
        let metrics = current
            .info
            .get("metricsJson")
            .and_then(|j| serde_json::from_str::<LiveContainerMetrics>(j).ok());

        if let Some(m) = metrics {
            let mem_str = if m.memory_limit_bytes > 0 {
                format!(
                    "{} / {}",
                    catalog::format_bytes(m.memory_bytes),
                    catalog::format_bytes(m.memory_limit_bytes)
                )
            } else {
                catalog::format_bytes(m.memory_bytes)
            };
            let cpu_ms = m.cpu_usage_usec as f64 / 1000.0;
            println!(
                "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} {}",
                short_id(id),
                state_str,
                health,
                m.pid,
                mem_str,
                format!("{cpu_ms:.1}ms"),
                m.pids_current,
                restarts,
                m.source
            );
        } else {
            println!(
                "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} -",
                short_id(id),
                state_str,
                health,
                "-",
                "-",
                "-",
                "-",
                restarts
            );
        }
        let _ = std::io::stdout().flush();

        if !watch || st.is_some_and(|s| s.state == ContainerState::ContainerExited as i32) {
            break;
        }
        tokio::time::sleep(step).await;
        current = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
            .with_context(|| format!("poll metrics for container '{id}'"))?
            .into_inner();
    }

    Ok(true)
}

pub async fn cmd_exec(addr: &str, id: &str, timeout: i64, cmd: Vec<String>) -> Result<ExitCode> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let resp = rt
        .exec_sync(ExecSyncRequest {
            container_id: id.to_string(),
            cmd,
            timeout,
        })
        .await
        .with_context(|| format!("exec inside container '{id}'"))?
        .into_inner();

    std::io::stdout().write_all(&resp.stdout)?;
    std::io::stderr().write_all(&resp.stderr)?;
    let code = u8::try_from(resp.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

pub async fn cmd_stop(addr: &str, id: &str, timeout: i64) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    rt.stop_container(StopContainerRequest {
        container_id: id.to_string(),
        timeout,
    })
    .await
    .with_context(|| format!("stop container '{id}'"))?;
    println!("Stopped container '{id}'");
    Ok(())
}

pub async fn cmd_rm(addr: &str, id: &str) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    rt.remove_container(RemoveContainerRequest {
        container_id: id.to_string(),
    })
    .await
    .with_context(|| format!("remove container '{id}'"))?;
    println!("Removed container '{id}'");
    Ok(())
}

pub async fn cmd_diff_via_daemon(addr: &str, id: &str) -> Result<Option<String>> {
    let Some(ch) = try_connect_channel(addr).await else {
        return Ok(None);
    };
    let mut rt = RuntimeServiceClient::new(ch);
    let Ok(resp) = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await
    else {
        return Ok(None);
    };
    Ok(Some(
        resp.into_inner()
            .info
            .get("overlayDiff")
            .cloned()
            .unwrap_or_default(),
    ))
}
