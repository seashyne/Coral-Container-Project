//! Pod sandbox commands (`pods ls`, `pods stop`, `pods rm`) over CRI v1 `RuntimeService` gRPC.

use super::{connect_channel, format_nanos, short_id};
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{
    ListPodSandboxRequest, PodSandboxState, RemovePodSandboxRequest, StopPodSandboxRequest,
};

pub async fn cmd_pods_list(addr: &str) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let mut items = rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .context("list_pod_sandbox")?
        .into_inner()
        .items;
    items.sort_by_key(|p| std::cmp::Reverse(p.created_at));

    if items.is_empty() {
        println!("No pod sandboxes in coral-daemon ({addr}).");
        return Ok(());
    }

    println!("{:<24} {:<24} {:<10} CREATED", "POD ID", "NAME", "STATE");
    for p in items {
        let name = p
            .metadata
            .as_ref()
            .map(|m| m.name.as_str())
            .unwrap_or(&p.id);
        let state = match PodSandboxState::try_from(p.state) {
            Ok(PodSandboxState::SandboxReady) => "ready",
            _ => "notready",
        };
        println!(
            "{:<24} {:<24} {:<10} {}",
            short_id(&p.id),
            name,
            state,
            format_nanos(p.created_at)
        );
    }
    Ok(())
}

pub async fn cmd_pods_stop(addr: &str, id: &str) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    rt.stop_pod_sandbox(StopPodSandboxRequest {
        pod_sandbox_id: id.to_string(),
    })
    .await
    .with_context(|| format!("stop pod sandbox '{id}'"))?;
    println!("Stopped pod '{id}'");
    Ok(())
}

pub async fn cmd_pods_rm(addr: &str, id: &str) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    rt.remove_pod_sandbox(RemovePodSandboxRequest {
        pod_sandbox_id: id.to_string(),
    })
    .await
    .with_context(|| format!("remove pod sandbox '{id}'"))?;
    println!("Removed pod '{id}'");
    Ok(())
}
