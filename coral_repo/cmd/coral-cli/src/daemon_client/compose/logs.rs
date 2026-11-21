//! `coral compose logs`.

use super::common::{list_project_containers, load_compose_spec_optional, resolve_project_name};
use crate::daemon_client::connect_channel;
use anyhow::Result;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ContainerStatusRequest};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

pub async fn cmd_compose_logs(
    addr: &str,
    file: Option<&Path>,
    env_file: Option<&Path>,
    project_override: Option<&str>,
    service_filter: Option<&str>,
    follow: bool,
    tail: Option<usize>,
) -> Result<()> {
    let spec_opt = load_compose_spec_optional(file, env_file);
    let project = resolve_project_name(
        project_override,
        spec_opt.as_ref().map(|(_, s)| s),
        spec_opt.as_ref().map(|(p, _)| p.as_path()),
    );

    let ch = connect_channel(addr, 5).await?;
    let mut rt = RuntimeServiceClient::new(ch);
    let services: Vec<String> = service_filter.into_iter().map(ToOwned::to_owned).collect();
    let rows = list_project_containers(&mut rt, &project, &services).await?;

    if rows.is_empty() {
        println!("No containers found for Compose project '{project}'.");
        return Ok(());
    }

    let mut offsets: HashMap<String, usize> = HashMap::new();
    loop {
        let mut any_running = false;
        for c in &rows {
            let svc = c
                .labels
                .get("coral.compose.service")
                .cloned()
                .unwrap_or_else(|| c.id.clone());
            let resp = rt
                .container_status(ContainerStatusRequest {
                    container_id: c.id.clone(),
                    verbose: true,
                })
                .await?
                .into_inner();
            if resp
                .status
                .as_ref()
                .is_some_and(|s| s.state == ContainerState::ContainerRunning as i32)
            {
                any_running = true;
            }
            if let Some(log) = resp.info.get("logOutput") {
                let all_lines: Vec<&str> = log.lines().collect();
                let start = match offsets.get(&c.id).copied() {
                    Some(prev) => prev.min(all_lines.len()),
                    None => match tail {
                        Some(n) => all_lines.len().saturating_sub(n),
                        None => 0,
                    },
                };
                for line in &all_lines[start..] {
                    println!("[{svc}] {line}");
                }
                offsets.insert(c.id.clone(), all_lines.len());
            }
        }
        if !follow || !any_running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(())
}
