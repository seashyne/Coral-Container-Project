//! `coral compose ps` and `coral compose config`.

use super::common::{list_project_containers, load_compose_spec_optional, resolve_project_name};
use crate::daemon_client::{connect_channel, short_id};
use anyhow::Result;
use coral_api::ContainerState;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_daemon::compose::ComposeSpec;
use std::path::Path;

pub async fn cmd_compose_ps(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    include_all: bool,
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let mut rows = list_project_containers(&mut rt, &project, &[]).await?;
    if !include_all {
        let active: Vec<_> = rows
            .iter()
            .filter(|c| c.state == ContainerState::ContainerRunning as i32)
            .cloned()
            .collect();
        if !active.is_empty() {
            rows = active;
        }
    }

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
        let base_state = crate::daemon_client::container_state_label(c.state);
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

pub fn cmd_compose_config(file: Option<&Path>, env_file: Option<&Path>) -> Result<()> {
    let compose_path = super::common::resolve_compose_file(file)?;
    let spec = ComposeSpec::from_file_with_env(&compose_path, env_file)?;
    println!("{}", serde_json::to_string_pretty(&spec)?);
    Ok(())
}
