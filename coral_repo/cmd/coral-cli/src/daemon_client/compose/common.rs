//! Shared Compose helpers: file discovery, project naming, dependency waiting and
//! project container listing.

use anyhow::{Context, Result, bail};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerFilter, ContainerState, ContainerStatusRequest, ListContainersRequest};
use coral_daemon::compose::{ComposeSpec, DependsCondition};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) const DEFAULT_COMPOSE_FILES: &[&str] = &[
    "coral-compose.yml",
    "coral-compose.yaml",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

pub(super) fn resolve_compose_file(explicit: Option<&Path>) -> Result<PathBuf> {
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

pub(super) fn load_compose_spec_optional(
    file: Option<&Path>,
    env_file: Option<&Path>,
) -> Option<(PathBuf, ComposeSpec)> {
    resolve_compose_file(file).ok().and_then(|p| {
        ComposeSpec::from_file_with_env(&p, env_file)
            .ok()
            .map(|s| (p, s))
    })
}

pub(super) fn resolve_project_name(
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
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "coral-stack".into()
    } else {
        trimmed.to_string()
    }
}

pub(super) async fn wait_for_service_condition(
    rt: &mut RuntimeServiceClient<crate::daemon_client::AuthChannel>,
    dep_service: &str,
    dep_cid: &str,
    condition: DependsCondition,
    timeout_secs: u64,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: dep_cid.to_string(),
                verbose: true,
            })
            .await
            .with_context(|| format!("poll dependency service '{dep_service}' status"))?
            .into_inner();
        let state = resp.status.as_ref().map(|s| s.state).unwrap_or(0);
        let exit_code = resp.status.as_ref().map(|s| s.exit_code).unwrap_or(-1);
        let health = resp
            .info
            .get("healthStatus")
            .map(String::as_str)
            .unwrap_or("none");

        match condition {
            DependsCondition::ServiceStarted => return Ok(()),
            DependsCondition::ServiceHealthy => {
                if health == "healthy"
                    || (health == "none" && state == ContainerState::ContainerRunning as i32)
                {
                    return Ok(());
                }
                if state == ContainerState::ContainerExited as i32 {
                    bail!(
                        "dependency service '{dep_service}' exited (code {exit_code}) before becoming healthy"
                    );
                }
                if health == "unhealthy" {
                    bail!("dependency service '{dep_service}' became unhealthy");
                }
            }
            DependsCondition::ServiceCompletedSuccessfully => {
                if state == ContainerState::ContainerExited as i32 {
                    if exit_code == 0 {
                        return Ok(());
                    }
                    bail!("dependency service '{dep_service}' exited with status code {exit_code}");
                }
            }
        }

        if Instant::now() >= deadline {
            bail!("timed out waiting for dependency service '{dep_service}'");
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
}

pub(super) async fn list_project_containers(
    rt: &mut RuntimeServiceClient<crate::daemon_client::AuthChannel>,
    project: &str,
    services: &[String],
) -> Result<Vec<coral_api::Container>> {
    let mut rows = rt
        .list_containers(ListContainersRequest {
            filter: Some(ContainerFilter {
                id: String::new(),
                pod_sandbox_id: String::new(),
                label_selector: HashMap::from([(
                    "coral.compose.project".into(),
                    project.to_string(),
                )]),
            }),
        })
        .await
        .with_context(|| format!("list compose containers for '{project}'"))?
        .into_inner()
        .containers;

    if !services.is_empty() {
        rows.retain(|c| {
            c.labels
                .get("coral.compose.service")
                .is_some_and(|s| services.iter().any(|req| req == s))
        });
    }
    Ok(rows)
}
