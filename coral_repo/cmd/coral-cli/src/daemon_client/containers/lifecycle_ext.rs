//! Client RPC helpers for `coral pause`, `coral unpause`, `coral rename`, and `coral update`.

use crate::daemon_client::connect_channel;
use crate::units::parse_memory_mb;
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{PodSandboxConfig, RunPodSandboxRequest};
use std::collections::HashMap;

async fn invoke_container_action(
    addr: &str,
    action: &str,
    extra: HashMap<String, String>,
) -> Result<String> {
    let ch = connect_channel(addr, 5).await?;
    let mut client = RuntimeServiceClient::new(ch);
    let mut annotations = extra;
    annotations.insert("coral.system.action".into(), action.to_string());
    let resp = client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations,
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .with_context(|| format!("daemon action '{action}' failed"))?
        .into_inner();
    Ok(resp.pod_sandbox_id)
}

pub async fn cmd_pause(addr: &str, id: &str) -> Result<()> {
    let cid = invoke_container_action(
        addr,
        "container_pause",
        HashMap::from([("coral.container.id".into(), id.to_string())]),
    )
    .await?;
    println!("{cid}");
    Ok(())
}

pub async fn cmd_unpause(addr: &str, id: &str) -> Result<()> {
    let cid = invoke_container_action(
        addr,
        "container_unpause",
        HashMap::from([("coral.container.id".into(), id.to_string())]),
    )
    .await?;
    println!("{cid}");
    Ok(())
}

pub async fn cmd_rename(addr: &str, id: &str, new_name: &str) -> Result<()> {
    let cid = invoke_container_action(
        addr,
        "container_rename",
        HashMap::from([
            ("coral.container.id".into(), id.to_string()),
            ("coral.container.new_name".into(), new_name.to_string()),
        ]),
    )
    .await?;
    println!("Renamed {cid} -> {new_name}");
    Ok(())
}

pub async fn cmd_update(
    addr: &str,
    id: &str,
    memory: Option<&str>,
    cpus: Option<f64>,
    pids_limit: Option<i64>,
) -> Result<()> {
    let mut map = HashMap::from([("coral.container.id".into(), id.to_string())]);
    if let Some(mem_str) = memory {
        let mb = parse_memory_mb(mem_str).map_err(anyhow::Error::msg)?;
        let bytes = mb * 1024 * 1024;
        map.insert("coral.update.memory_bytes".into(), bytes.to_string());
    }
    if let Some(c) = cpus
        && c > 0.0
    {
        let period = 100_000i64;
        let quota = ((c * period as f64).round() as i64).max(1_000);
        map.insert("coral.update.cpu_quota".into(), quota.to_string());
        map.insert("coral.update.cpu_period".into(), period.to_string());
    }
    if let Some(p) = pids_limit
        && p > 0
    {
        map.insert("coral.update.pids_limit".into(), p.to_string());
    }
    let cid = invoke_container_action(addr, "container_update", map).await?;
    println!("{cid}");
    Ok(())
}

pub async fn cmd_top(addr: &str, id: &str, json_out: bool) -> Result<()> {
    let raw = invoke_container_action(
        addr,
        "container_top",
        HashMap::from([("coral.container.id".into(), id.to_string())]),
    )
    .await?;
    if json_out {
        println!("{raw}");
        return Ok(());
    }
    let procs: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap_or_default();
    println!("{:<8} {:<8} {:<12} {:<10} CMD", "PID", "PPID", "STATE", "RSS");
    for p in procs {
        println!(
            "{:<8} {:<8} {:<12} {:<10} {}",
            p["pid"],
            p["ppid"],
            p["state"].as_str().unwrap_or("?"),
            format!("{} KiB", p["rssKb"].as_u64().unwrap_or(0)),
            p["cmd"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

pub async fn cmd_wait(addr: &str, id: &str) -> Result<i32> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    loop {
        let resp = rt
            .container_status(coral_api::ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: false,
            })
            .await
            .with_context(|| format!("wait for container '{id}'"))?
            .into_inner();
        let Some(st) = resp.status else {
            anyhow::bail!("container '{id}' status missing");
        };
        if st.state == coral_api::ContainerState::ContainerExited as i32 {
            println!("{}", st.exit_code);
            return Ok(st.exit_code);
        }
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    }
}
