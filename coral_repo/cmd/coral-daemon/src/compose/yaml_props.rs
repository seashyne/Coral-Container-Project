//! Per-service property handling for the Compose YAML parser.

use super::deps::{apply_list_item, push_dependency};
use super::model::{ComposeService, DependsCondition};
use super::text::{
    parse_command_value, parse_cpu_percent, parse_duration_ms, parse_health_test,
    parse_inline_list, parse_memory_mb, resolve_maybe_rel, unquote_yaml,
};
use super::util::{parse_dotenv_file, upsert_env_kv};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::Path;

/// Top-level sections of a Compose file.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TopSection {
    None,
    Services,
    Volumes,
    Networks,
}

/// Applies a top-level (`indent == 0`) Compose key.
///
/// Returns the project name when the key sets one.
pub(super) fn apply_top_level_key(
    key: &str,
    val: &str,
    line_no: usize,
    section: &mut TopSection,
) -> Result<Option<String>> {
    match key {
        "name" | "project" => {
            let project = if val.is_empty() { None } else { Some(val.to_string()) };
            *section = TopSection::None;
            Ok(project)
        }
        "version" => {
            *section = TopSection::None;
            Ok(None)
        }
        "services" => {
            *section = TopSection::Services;
            Ok(None)
        }
        "volumes" => {
            *section = TopSection::Volumes;
            Ok(None)
        }
        "networks" => {
            *section = TopSection::Networks;
            Ok(None)
        }
        other => {
            bail!("line {}: unsupported top-level key '{other}'", line_no + 1);
        }
    }
}

/// Records a named volume declared in the top-level `volumes:` section.
pub(super) fn collect_top_volume(trimmed: &str, top_volumes: &mut Vec<String>) {
    if let Some((vname, _)) = trimmed.split_once(':') {
        let name = unquote_yaml(vname.trim().trim_start_matches('-').trim());
        if !name.is_empty() && !top_volumes.contains(&name) {
            top_volumes.push(name);
        }
    } else if let Some(item) = trimmed.strip_prefix('-') {
        let name = unquote_yaml(item.trim());
        if !name.is_empty() && !top_volumes.contains(&name) {
            top_volumes.push(name);
        }
    }
}

/// Extracts the service name from a `  <name>:` line.
pub(super) fn detect_service_name(trimmed: &str, line_no: usize) -> Result<String> {
    let Some((sname, _)) = trimmed.split_once(':') else {
        bail!(
            "line {}: expected service name `<name>:`",
            line_no + 1
        );
    };
    let name = unquote_yaml(sname.trim());
    if name.is_empty() {
        bail!("line {}: empty service name", line_no + 1);
    }
    Ok(name)
}

/// Applies a scalar service property (`image:`, `build:`, `restart:`, ...).
///
/// Inline list syntax (`ports: ["a", "b"]`) is expanded into list items.
pub(super) fn apply_service_property(
    svc: &mut ComposeService,
    svc_env_list: &mut Vec<String>,
    key: &str,
    val: &str,
    val_raw: &str,
    base_dir: Option<&Path>,
) -> Result<()> {
    match key {
        "image" => {
            if !val.is_empty() {
                svc.image = Some(val.to_string());
            }
        }
        "build" => {
            if !val.is_empty() {
                svc.build_context = Some(resolve_maybe_rel(val, base_dir));
            }
        }
        "entrypoint" => {
            if !val_raw.is_empty() {
                svc.entrypoint = parse_command_value(val_raw);
            }
        }
        "command" => {
            if !val_raw.is_empty() {
                svc.command = parse_command_value(val_raw);
            }
        }
        "env_file" => {
            if val_raw.starts_with('[') && val_raw.ends_with(']') {
                for item in parse_inline_list(val_raw) {
                    if !item.is_empty() {
                        svc.env_files.push(resolve_maybe_rel(&item, base_dir));
                    }
                }
            } else if !val.is_empty() {
                svc.env_files.push(resolve_maybe_rel(val, base_dir));
            }
        }
        "working_dir" | "workdir" => {
            if !val.is_empty() {
                svc.working_dir = val.to_string();
            }
        }
        "hostname" | "container_name" => {
            if !val.is_empty() && svc.hostname.is_none() {
                svc.hostname = Some(val.to_string());
            }
        }
        "user" => {
            if !val.is_empty() {
                svc.user = Some(val.to_string());
            }
        }
        "privileged" => {
            svc.privileged = matches!(val.to_ascii_lowercase().as_str(), "true" | "1" | "yes");
        }
        "network_mode" | "network" => {
            if !val.is_empty() {
                svc.network = val.to_string();
            }
        }
        "memory_mb" | "mem_limit" | "memory" => {
            if !val.is_empty() {
                svc.memory_mb = parse_memory_mb(val)?;
            }
        }
        "cpu_percent" | "cpus" => {
            if !val.is_empty() {
                svc.cpu_percent = parse_cpu_percent(val)?;
            }
        }
        "pids_limit" | "pids" => {
            if !val.is_empty() {
                svc.pids_limit = val.parse().unwrap_or(0);
            }
        }
        "restart" => {
            if !val.is_empty() {
                svc.restart = val.to_string();
            }
        }
        "gpus" => {
            if !val.is_empty() {
                svc.gpus = Some(val.to_string());
            }
        }
        "log_driver" => {
            if !val.is_empty() {
                svc.log_driver = Some(val.to_string());
            }
        }
        "ports" | "expose" | "volumes" | "environment" | "depends_on" | "dns" | "extra_hosts"
        | "labels" | "profiles" | "networks" | "devices" | "aliases" | "network_aliases"
            if val_raw.starts_with('[') && val_raw.ends_with(']') =>
        {
            for item in parse_inline_list(val_raw) {
                apply_list_item(svc, svc_env_list, key, &item, base_dir);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Applies a nested (second level) service property such as `environment.FOO`,
/// `build.target` or `healthcheck.interval`.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_sub_property(
    svc: &mut ComposeService,
    svc_env_list: &mut Vec<String>,
    prop: &str,
    sub_key: &str,
    sub_val: &str,
    sub_val_raw: &str,
    base_dir: Option<&Path>,
) {
    match prop {
        "environment" => {
            if !sub_key.is_empty() {
                upsert_env_kv(svc_env_list, sub_key, sub_val);
            }
        }
        "labels" => {
            if !sub_key.is_empty() {
                svc.labels.insert(sub_key.to_string(), sub_val.to_string());
            }
        }
        "networks" => {
            if !sub_key.is_empty() && !svc.networks.iter().any(|n| n == sub_key) {
                svc.networks.push(sub_key.to_string());
            }
        }
        "build" => match sub_key {
            "context" => {
                svc.build_context = Some(resolve_maybe_rel(sub_val, base_dir));
            }
            "dockerfile" => {
                svc.dockerfile = Some(sub_val.to_string());
            }
            "target" if !sub_val.is_empty() => {
                svc.build_target = Some(sub_val.to_string());
            }
            _ => {}
        },
        "healthcheck" => match sub_key {
            "test" => {
                let cmd = parse_health_test(sub_val_raw);
                if !cmd.is_empty() {
                    svc.health_cmd = Some(cmd);
                }
            }
            "interval" => {
                if let Some(ms) = parse_duration_ms(sub_val) {
                    svc.health_interval_ms = ms;
                }
            }
            "retries" => {
                if let Ok(r) = sub_val.parse::<u32>() {
                    svc.health_retries = r.max(1);
                }
            }
            _ => {}
        },
        "depends_on" if !sub_key.is_empty() => {
            push_dependency(svc, sub_key, DependsCondition::ServiceStarted);
        }
        "logging" if sub_key == "driver" && !sub_val.is_empty() => {
            svc.log_driver = Some(sub_val.to_string());
        }
        _ => {}
    }
}

/// Applies a third-level nested property such as `depends_on.<svc>.condition`
/// or `build.args.<key>`.
pub(super) fn apply_deep_sub_property(
    svc: &mut ComposeService,
    prop: &str,
    sp: &str,
    sub_key: &str,
    sub_val: &str,
) {
    match (prop, sp, sub_key) {
        ("depends_on", dep_name, "condition") => {
            let cond = match sub_val.to_ascii_lowercase().as_str() {
                "service_healthy" => DependsCondition::ServiceHealthy,
                "service_completed_successfully" => DependsCondition::ServiceCompletedSuccessfully,
                _ => DependsCondition::ServiceStarted,
            };
            push_dependency(svc, dep_name, cond);
        }
        ("build", "args", key_name) if !key_name.is_empty() => {
            svc.build_args.push(format!("{key_name}={sub_val}"));
        }
        _ => {}
    }
}

/// Merges `env_file` contents and explicit `environment` entries into each service.
///
/// `env_file` entries are loaded first, then explicit `environment` keys are layered
/// on top so explicit declarations always win over file defaults.
pub(super) fn merge_service_envs(
    services_map: &mut HashMap<String, ComposeService>,
    explicit_envs: &HashMap<String, Vec<String>>,
    interp_vars: &HashMap<String, String>,
) -> Result<()> {
    for (name, svc) in services_map.iter_mut() {
        let mut merged_envs = Vec::new();
        for ef in &svc.env_files {
            let path = Path::new(ef);
            for (k, v) in parse_dotenv_file(path)
                .with_context(|| format!("load env_file '{}' for service '{name}'", path.display()))?
            {
                upsert_env_kv(&mut merged_envs, &k, &v);
            }
        }
        if let Some(explicit) = explicit_envs.get(name) {
            for entry in explicit {
                if let Some((k, v)) = entry.split_once('=') {
                    upsert_env_kv(&mut merged_envs, k.trim(), v);
                } else if let Some(val) = interp_vars.get(entry.trim()) {
                    upsert_env_kv(&mut merged_envs, entry.trim(), val);
                }
            }
        }
        svc.envs = merged_envs;
    }
    Ok(())
}
