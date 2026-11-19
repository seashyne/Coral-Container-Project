//! Multi-container stack specification and parser for `coral compose`
//! (`coral-compose.yml` / `docker-compose.yml`).

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// Parsed multi-container stack specification with services ordered topologically by `depends_on`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeSpec {
    pub project_name: Option<String>,
    pub services: Vec<ComposeService>,
    pub volumes: Vec<String>,
}

/// Configuration for a single service inside a Compose stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub image: Option<String>,
    pub build_context: Option<String>,
    pub dockerfile: Option<String>,
    pub command: Vec<String>,
    pub envs: Vec<String>,
    pub ports: Vec<String>,
    pub volumes: Vec<String>,
    pub depends_on: Vec<String>,
    pub network: String,
    pub dns: Vec<String>,
    pub extra_hosts: Vec<String>,
    pub working_dir: String,
    pub hostname: Option<String>,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub restart: String,
    pub health_cmd: Option<String>,
    pub health_interval_ms: u64,
    pub health_retries: u32,
}

impl Default for ComposeService {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: None,
            build_context: None,
            dockerfile: None,
            command: Vec::new(),
            envs: Vec::new(),
            ports: Vec::new(),
            volumes: Vec::new(),
            depends_on: Vec::new(),
            network: "host".into(),
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            working_dir: "/".into(),
            hostname: None,
            memory_mb: 0,
            cpu_percent: 0,
            pids_limit: 0,
            restart: "no".into(),
            health_cmd: None,
            health_interval_ms: 5_000,
            health_retries: 3,
        }
    }
}

impl ComposeSpec {
    /// Loads and parses a `docker-compose.yml` / `coral-compose.yml` file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .map_err(|e| anyhow!("read compose file '{}': {e}", path.display()))?;
        let base_dir = path.parent();
        Self::from_yaml_str(&content, base_dir)
    }

    /// Parses a Docker Compose / Coral Compose YAML string, resolves relative volume/build paths
    /// against `base_dir`, topologically sorts services by `depends_on`, and injects sibling
    /// service names into `extra_hosts`.
    pub fn from_yaml_str(raw: &str, base_dir: Option<&Path>) -> Result<Self> {
        let mut project_name: Option<String> = None;
        let mut services_map: HashMap<String, ComposeService> = HashMap::new();
        let mut service_order: Vec<String> = Vec::new();
        let mut top_volumes: Vec<String> = Vec::new();

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum TopSection {
            None,
            Services,
            Volumes,
            Networks,
        }

        let mut section = TopSection::None;
        let mut cur_service: Option<String> = None;
        let mut service_indent: Option<usize> = None;
        let mut prop_indent: Option<usize> = None;
        let mut cur_prop: Option<String> = None;

        for (line_no, raw_line) in raw.lines().enumerate() {
            let stripped = strip_yaml_comment(raw_line);
            if stripped.trim().is_empty() {
                continue;
            }
            let indent = stripped.len() - stripped.trim_start().len();
            let trimmed = stripped.trim();

            if indent == 0 {
                cur_service = None;
                cur_prop = None;
                service_indent = None;
                prop_indent = None;

                if let Some((k, v)) = trimmed.split_once(':') {
                    let key = k.trim();
                    let val = unquote_yaml(v.trim());
                    match key {
                        "name" | "project" => {
                            if !val.is_empty() {
                                project_name = Some(val);
                            }
                            section = TopSection::None;
                        }
                        "version" => {
                            section = TopSection::None;
                        }
                        "services" => {
                            section = TopSection::Services;
                        }
                        "volumes" => {
                            section = TopSection::Volumes;
                        }
                        "networks" => {
                            section = TopSection::Networks;
                        }
                        other => {
                            bail!("line {}: unsupported top-level key '{other}'", line_no + 1);
                        }
                    }
                }
                continue;
            }

            match section {
                TopSection::Volumes => {
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
                TopSection::Networks | TopSection::None => {}
                TopSection::Services => {
                    let svc_ind = *service_indent.get_or_insert(indent);
                    if indent == svc_ind && !trimmed.starts_with('-') {
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
                        services_map.entry(name.clone()).or_insert_with(|| ComposeService {
                            name: name.clone(),
                            ..ComposeService::default()
                        });
                        if !service_order.contains(&name) {
                            service_order.push(name.clone());
                        }
                        cur_service = Some(name);
                        cur_prop = None;
                        prop_indent = None;
                        continue;
                    }

                    let Some(active_svc_name) = cur_service.as_ref() else {
                        continue;
                    };
                    let svc = services_map
                        .get_mut(active_svc_name)
                        .expect("active service exists");

                    let p_ind = *prop_indent.get_or_insert(indent);
                    if indent == p_ind && !trimmed.starts_with('-') {
                        let Some((k, v)) = trimmed.split_once(':') else {
                            bail!("line {}: malformed service property '{trimmed}'", line_no + 1);
                        };
                        let key = k.trim();
                        let val_raw = v.trim();
                        let val = unquote_yaml(val_raw);
                        cur_prop = Some(key.to_string());

                        match key {
                            "image" => {
                                if !val.is_empty() {
                                    svc.image = Some(val);
                                }
                            }
                            "build" => {
                                if !val.is_empty() {
                                    svc.build_context = Some(resolve_maybe_rel(&val, base_dir));
                                }
                            }
                            "command" | "entrypoint" => {
                                if !val_raw.is_empty() {
                                    svc.command = parse_command_value(val_raw);
                                }
                            }
                            "working_dir" | "workdir" => {
                                if !val.is_empty() {
                                    svc.working_dir = val;
                                }
                            }
                            "hostname" | "container_name" => {
                                if !val.is_empty() && svc.hostname.is_none() {
                                    svc.hostname = Some(val);
                                }
                            }
                            "network_mode" | "network" => {
                                if !val.is_empty() {
                                    svc.network = val;
                                }
                            }
                            "memory_mb" | "mem_limit" | "memory" => {
                                if !val.is_empty() {
                                    svc.memory_mb = parse_memory_mb(&val)?;
                                }
                            }
                            "cpu_percent" | "cpus" => {
                                if !val.is_empty() {
                                    svc.cpu_percent = parse_cpu_percent(&val)?;
                                }
                            }
                            "pids_limit" | "pids" => {
                                if !val.is_empty() {
                                    svc.pids_limit = val.parse().unwrap_or(0);
                                }
                            }
                            "restart" => {
                                if !val.is_empty() {
                                    svc.restart = val;
                                }
                            }
                            "ports" | "volumes" | "environment" | "depends_on" | "dns"
                            | "extra_hosts"
                                if val_raw.starts_with('[') && val_raw.ends_with(']') =>
                            {
                                for item in parse_inline_list(val_raw) {
                                    apply_list_item(svc, key, &item, base_dir);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Nested item under `cur_prop` (indent > p_ind or `- ...` list item)
                    if let Some(prop) = cur_prop.as_deref() {
                        if let Some(item_raw) = trimmed.strip_prefix('-') {
                            let item = unquote_yaml(item_raw.trim());
                            apply_list_item(svc, prop, &item, base_dir);
                        } else if let Some((k, v)) = trimmed.split_once(':') {
                            let sub_key = unquote_yaml(k.trim());
                            let sub_val_raw = v.trim();
                            let sub_val = unquote_yaml(sub_val_raw);
                            match prop {
                                "environment" => {
                                    if !sub_key.is_empty() {
                                        svc.envs.push(format!("{sub_key}={sub_val}"));
                                    }
                                }
                                "build" => match sub_key.as_str() {
                                    "context" => {
                                        svc.build_context =
                                            Some(resolve_maybe_rel(&sub_val, base_dir));
                                    }
                                    "dockerfile" => {
                                        svc.dockerfile = Some(sub_val);
                                    }
                                    _ => {}
                                },
                                "healthcheck" => match sub_key.as_str() {
                                    "test" => {
                                        let cmd = parse_health_test(sub_val_raw);
                                        if !cmd.is_empty() {
                                            svc.health_cmd = Some(cmd);
                                        }
                                    }
                                    "interval" => {
                                        if let Some(ms) = parse_duration_ms(&sub_val) {
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
                                "depends_on"
                                    if !sub_key.is_empty()
                                        && !svc.depends_on.contains(&sub_key) =>
                                {
                                    svc.depends_on.push(sub_key);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        if services_map.is_empty() {
            bail!("Compose file defines no services under 'services:'");
        }

        // Topologically sort services according to `depends_on`.
        let ordered_names = topological_sort_services(&service_order, &services_map)?;
        let all_service_names: Vec<String> = ordered_names.clone();

        let mut ordered_services = Vec::with_capacity(ordered_names.len());
        for name in ordered_names {
            let mut svc = services_map.remove(&name).unwrap();
            // Inject sibling service names into `extra_hosts` (`<sibling>:127.0.0.1`) so services
            // can connect to each other by service name.
            for sibling in &all_service_names {
                let mapping = format!("{sibling}:127.0.0.1");
                if !svc
                    .extra_hosts
                    .iter()
                    .any(|h| h.starts_with(&format!("{sibling}:")))
                {
                    svc.extra_hosts.push(mapping);
                }
            }
            // Collect any named volumes referenced in `svc.volumes`.
            for v in &svc.volumes {
                if let Some((src, _)) = v.split_once(':') {
                    let s = src.trim();
                    if !s.contains('/')
                        && s != "."
                        && s != ".."
                        && coral_runtime::rootfs::is_valid_volume_name(s)
                        && !top_volumes.contains(&s.to_string())
                    {
                        top_volumes.push(s.to_string());
                    }
                }
            }
            ordered_services.push(svc);
        }

        Ok(Self {
            project_name,
            services: ordered_services,
            volumes: top_volumes,
        })
    }
}

fn apply_list_item(
    svc: &mut ComposeService,
    prop: &str,
    item: &str,
    base_dir: Option<&Path>,
) {
    if item.is_empty() {
        return;
    }
    match prop {
        "ports" => svc.ports.push(item.to_string()),
        "volumes" => svc.volumes.push(resolve_volume_entry(item, base_dir)),
        "environment" => svc.envs.push(item.to_string()),
        "depends_on" => {
            if !svc.depends_on.contains(&item.to_string()) {
                svc.depends_on.push(item.to_string());
            }
        }
        "dns" => svc.dns.push(item.to_string()),
        "extra_hosts" => svc.extra_hosts.push(item.to_string()),
        "command" | "entrypoint" => svc.command.push(item.to_string()),
        _ => {}
    }
}

fn resolve_maybe_rel(raw: &str, base_dir: Option<&Path>) -> String {
    if let Some(base) = base_dir
        && (raw == "." || raw.starts_with("./") || raw.starts_with("../"))
    {
        let joined = base.join(raw);
        return fs::canonicalize(&joined)
            .unwrap_or(joined)
            .to_string_lossy()
            .into_owned();
    }
    raw.to_string()
}

fn resolve_volume_entry(raw: &str, base_dir: Option<&Path>) -> String {
    let Some((src, rest)) = raw.split_once(':') else {
        return raw.to_string();
    };
    let resolved_src = resolve_maybe_rel(src.trim(), base_dir);
    format!("{resolved_src}:{rest}")
}

fn parse_command_value(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        return parse_inline_list(trimmed);
    }
    let unquoted = unquote_yaml(trimmed);
    split_shell_words(&unquoted)
}

fn parse_inline_list(raw: &str) -> Vec<String> {
    let inner = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    inner
        .split(',')
        .map(|s| unquote_yaml(s.trim()))
        .filter(|s| !s.is_empty())
        .collect()
}

fn split_shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '\\' if !in_single => {
                if let Some(next) = chars.next() {
                    cur.push(next);
                }
            }
            ws if ws.is_whitespace() && !in_single && !in_double => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            other => cur.push(other),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_health_test(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let items = parse_inline_list(trimmed);
        if items.first().is_some_and(|s| s.eq_ignore_ascii_case("CMD-SHELL")) {
            return items[1..].join(" ");
        }
        if items.first().is_some_and(|s| s.eq_ignore_ascii_case("CMD")) {
            return items[1..].join(" ");
        }
        return items.join(" ");
    }
    unquote_yaml(trimmed)
}

fn parse_duration_ms(raw: &str) -> Option<u64> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(v) = lower.strip_suffix("ms") {
        return v.trim().parse::<u64>().ok();
    }
    if let Some(v) = lower.strip_suffix('s') {
        return v.trim().parse::<u64>().ok().map(|s| s.saturating_mul(1_000));
    }
    if let Some(v) = lower.strip_suffix('m') {
        return v.trim().parse::<u64>().ok().map(|m| m.saturating_mul(60_000));
    }
    lower.parse::<u64>().ok().map(|s| s.saturating_mul(1_000))
}

fn parse_memory_mb(raw: &str) -> Result<i64> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(num) = lower.strip_suffix("gb").or_else(|| lower.strip_suffix('g')) {
        return Ok(num.trim().parse::<i64>()? * 1024);
    }
    if let Some(num) = lower.strip_suffix("mb").or_else(|| lower.strip_suffix('m')) {
        return Ok(num.trim().parse::<i64>()?);
    }
    Ok(lower.parse::<i64>()?)
}

fn parse_cpu_percent(raw: &str) -> Result<i64> {
    let trimmed = raw.trim().trim_end_matches('%');
    if trimmed.contains('.') {
        let f: f64 = trimmed.parse()?;
        Ok((f * 100.0).round() as i64)
    } else {
        let n: i64 = trimmed.parse()?;
        Ok(if n <= 16 { n * 100 } else { n })
    }
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'#' if !in_single && !in_double && (i == 0 || bytes[i - 1].is_ascii_whitespace()) => {
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

fn unquote_yaml(s: &str) -> String {
    let t = s.trim();
    if ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
        && t.len() >= 2
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

fn topological_sort_services(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compose_yaml_and_sorts_depends_on_topologically() {
        let yaml = r#"
name: coral-shop
services:
  web:
    image: alpine:3.20
    command: /bin/sh -c "echo web"
    depends_on:
      - api
      - redis
    ports:
      - "18080:8080"
    environment:
      API_URL: http://api:9000
      MODE: production
    volumes:
      - shop-assets:/var/www
  api:
    image: alpine:3.20
    depends_on:
      - redis
    environment:
      - REDIS_HOST=redis
    mem_limit: 128m
  redis:
    image: alpine:3.20
    command: ["/bin/sh", "-c", "echo redis"]
volumes:
  shop-assets:
"#;
        let spec = ComposeSpec::from_yaml_str(yaml, None).unwrap();
        assert_eq!(spec.project_name.as_deref(), Some("coral-shop"));
        let names: Vec<&str> = spec.services.iter().map(|s| s.name.as_str()).collect();
        // redis must come first, then api, then web!
        assert_eq!(names, vec!["redis", "api", "web"]);
        assert_eq!(spec.volumes, vec!["shop-assets"]);

        let web = &spec.services[2];
        assert_eq!(web.ports, vec!["18080:8080"]);
        assert!(web.envs.contains(&"API_URL=http://api:9000".to_string()));
        assert!(web.extra_hosts.contains(&"redis:127.0.0.1".to_string()));
        assert!(web.extra_hosts.contains(&"api:127.0.0.1".to_string()));
    }

    #[test]
    fn parses_compose_restart_policy_and_healthcheck() {
        let yaml = r#"
services:
  worker:
    image: alpine:3.20
    restart: on-failure:3
    healthcheck:
      test: ["CMD-SHELL", "test -f /tmp/ready"]
      interval: 2s
      retries: 4
"#;
        let spec = ComposeSpec::from_yaml_str(yaml, None).unwrap();
        let w = &spec.services[0];
        assert_eq!(w.restart, "on-failure:3");
        assert_eq!(w.health_cmd.as_deref(), Some("test -f /tmp/ready"));
        assert_eq!(w.health_interval_ms, 2_000);
        assert_eq!(w.health_retries, 4);
    }
}
