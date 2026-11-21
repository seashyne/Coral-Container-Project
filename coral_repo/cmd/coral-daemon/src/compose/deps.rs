//! Dependency-graph helpers for the Compose parser.

use super::model::{ComposeDependency, ComposeService, DependsCondition};
use super::text::{resolve_maybe_rel, resolve_volume_entry};
use anyhow::{Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(super) fn push_dependency(svc: &mut ComposeService, dep_name: &str, cond: DependsCondition) {
    if !svc.depends_on.iter().any(|d| d == dep_name) {
        svc.depends_on.push(dep_name.to_string());
    }
    if let Some(existing) = svc
        .depends_on_conditions
        .iter_mut()
        .find(|d| d.service == dep_name)
    {
        existing.condition = cond;
    } else {
        svc.depends_on_conditions.push(ComposeDependency {
            service: dep_name.to_string(),
            condition: cond,
        });
    }
}

pub(super) fn apply_list_item(
    svc: &mut ComposeService,
    svc_envs: &mut Vec<String>,
    prop: &str,
    item: &str,
    base_dir: Option<&Path>,
) {
    if item.is_empty() {
        return;
    }
    match prop {
        "ports" => svc.ports.push(item.to_string()),
        "expose" => svc.expose.push(item.to_string()),
        "volumes" => svc.volumes.push(resolve_volume_entry(item, base_dir)),
        "env_file" => svc.env_files.push(resolve_maybe_rel(item, base_dir)),
        "environment" => svc_envs.push(item.to_string()),
        "depends_on" => push_dependency(svc, item, DependsCondition::ServiceStarted),
        "dns" => svc.dns.push(item.to_string()),
        "extra_hosts" => svc.extra_hosts.push(item.to_string()),
        "command" => svc.command.push(item.to_string()),
        "entrypoint" => svc.entrypoint.push(item.to_string()),
        "profiles" => {
            if !svc.profiles.iter().any(|p| p == item) {
                svc.profiles.push(item.to_string());
            }
        }
        "networks" => {
            if !svc.networks.iter().any(|n| n == item) {
                svc.networks.push(item.to_string());
            }
        }
        "labels" => {
            if let Some((k, v)) = item.split_once('=') {
                svc.labels.insert(k.trim().to_string(), v.to_string());
            }
        }
        _ => {}
    }
}

pub(super) fn topological_sort_services(
    order: &[String],
    services: &HashMap<String, ComposeService>,
) -> Result<Vec<String>> {
    let mut visited = HashSet::new();
    let mut visiting = HashSet::new();
    let mut sorted = Vec::with_capacity(order.len());

    fn visit(
        name: &str,
        services: &HashMap<String, ComposeService>,
        visiting: &mut HashSet<String>,
        visited: &mut HashSet<String>,
        sorted: &mut Vec<String>,
    ) -> Result<()> {
        if visited.contains(name) {
            return Ok(());
        }
        if !visiting.insert(name.to_string()) {
            bail!("circular dependency detected in compose services involving '{name}'");
        }
        let svc = services
            .get(name)
            .ok_or_else(|| anyhow!("depends_on references unknown service '{name}'"))?;
        for dep in &svc.depends_on {
            visit(dep, services, visiting, visited, sorted)?;
        }
        visiting.remove(name);
        visited.insert(name.to_string());
        sorted.push(name.to_string());
        Ok(())
    }

    for name in order {
        visit(name, services, &mut visiting, &mut visited, &mut sorted)?;
    }
    Ok(sorted)
}
