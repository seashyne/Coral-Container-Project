//! Container lifecycle helpers (`ps`, `exec`, `stop`, `rm`, `diff`).

use crate::daemon_client::{
    connect_channel, container_state_label, format_nanos, short_id, try_connect_channel,
};
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ContainerStatusRequest, ExecSyncRequest, ListContainersRequest, RemoveContainerRequest,
    StopContainerRequest,
};
use std::io::Write;
use std::process::ExitCode;

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
