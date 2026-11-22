//! Compose YAML parser.

use super::deps::{apply_list_item, topological_sort_services};
use super::model::{ComposeService, ComposeSpec};
use super::text::{resolve_maybe_rel, strip_yaml_comment, unquote_yaml};
use super::util::expand_compose_vars;
use super::yaml_props::{
    TopSection, apply_deep_sub_property, apply_service_property, apply_sub_property,
    apply_top_level_key, collect_top_volume, detect_service_name, merge_service_envs,
};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::Path;

impl ComposeSpec {
    /// Parses a Docker Compose / Coral Compose YAML string, interpolates `${VAR:-default}`
    /// variables from `interp_vars`, resolves `env_file` and relative paths against `base_dir`,
    /// topologically sorts by `depends_on`, and injects sibling `extra_hosts`.
    pub fn from_yaml_with_env(
        raw: &str,
        base_dir: Option<&Path>,
        interp_vars: &HashMap<String, String>,
    ) -> Result<Self> {
        let expanded_yaml = expand_compose_vars(raw, interp_vars);
        let mut project_name: Option<String> = None;
        let mut services_map: HashMap<String, ComposeService> = HashMap::new();
        let mut service_order: Vec<String> = Vec::new();
        let mut top_volumes: Vec<String> = Vec::new();
        let mut top_secrets: HashMap<String, String> = HashMap::new();
        let mut top_configs: HashMap<String, String> = HashMap::new();
        let mut included_files: Vec<String> = Vec::new();
        let mut cur_top_item: Option<String> = None;
        let mut explicit_envs: HashMap<String, Vec<String>> = HashMap::new();

        let mut section = TopSection::None;
        let mut cur_service: Option<String> = None;
        let mut service_indent: Option<usize> = None;
        let mut prop_indent: Option<usize> = None;
        let mut cur_prop: Option<String> = None;
        let mut sub_indent: Option<usize> = None;
        let mut cur_sub_prop: Option<String> = None;

        for (line_no, raw_line) in expanded_yaml.lines().enumerate() {
            let stripped = strip_yaml_comment(raw_line);
            if stripped.trim().is_empty() {
                continue;
            }
            let indent = stripped.len() - stripped.trim_start().len();
            let trimmed = stripped.trim();

            if indent == 0 {
                cur_service = None;
                cur_top_item = None;
                cur_prop = None;
                cur_sub_prop = None;
                service_indent = None;
                prop_indent = None;
                sub_indent = None;

                if let Some((k, v)) = trimmed.split_once(':') {
                    let key = k.trim();
                    let val = unquote_yaml(v.trim());
                    if let Some(name) = apply_top_level_key(key, &val, line_no, &mut section)? {
                        project_name = Some(name);
                    }
                }
                continue;
            }

            match section {
                TopSection::Volumes => {
                    if indent <= 2 {
                        collect_top_volume(trimmed, &mut top_volumes);
                    }
                }
                TopSection::Secrets | TopSection::Configs => {
                    let target_map = if section == TopSection::Secrets {
                        &mut top_secrets
                    } else {
                        &mut top_configs
                    };
                    if indent <= 2 && let Some((k, _)) = trimmed.split_once(':') {
                        cur_top_item = Some(unquote_yaml(k.trim()));
                    } else if let Some(ref item_name) = cur_top_item
                        && let Some((k, v)) = trimmed.split_once(':')
                        && k.trim() == "file"
                    {
                        let p = unquote_yaml(v.trim());
                        if !p.is_empty() {
                            target_map.insert(item_name.clone(), resolve_maybe_rel(&p, base_dir));
                        }
                    }
                }
                TopSection::Include => {
                    if let Some(rest) = trimmed.strip_prefix('-') {
                        let r = rest.trim();
                        let path_val = if let Some((k, v)) = r.split_once(':')
                            && k.trim() == "path"
                        {
                            unquote_yaml(v.trim())
                        } else {
                            unquote_yaml(r)
                        };
                        if !path_val.is_empty() {
                            included_files.push(resolve_maybe_rel(&path_val, base_dir));
                        }
                    } else if let Some((k, v)) = trimmed.split_once(':')
                        && k.trim() == "path"
                    {
                        let p = unquote_yaml(v.trim());
                        if !p.is_empty() {
                            included_files.push(resolve_maybe_rel(&p, base_dir));
                        }
                    }
                }
                TopSection::Networks | TopSection::None => {}
                TopSection::Services => {
                    let svc_ind = *service_indent.get_or_insert(indent);
                    if indent == svc_ind && !trimmed.starts_with('-') {
                        let name = detect_service_name(trimmed, line_no)?;
                        services_map.entry(name.clone()).or_insert_with(|| ComposeService {
                            name: name.clone(),
                            ..ComposeService::default()
                        });
                        if !service_order.contains(&name) {
                            service_order.push(name.clone());
                        }
                        cur_service = Some(name);
                        cur_prop = None;
                        cur_sub_prop = None;
                        prop_indent = None;
                        sub_indent = None;
                        continue;
                    }

                    let Some(active_svc_name) = cur_service.as_ref() else {
                        continue;
                    };
                    let svc = services_map
                        .get_mut(active_svc_name)
                        .expect("active service exists");
                    let svc_env_list = explicit_envs.entry(active_svc_name.clone()).or_default();

                    let p_ind = *prop_indent.get_or_insert(indent);
                    if indent == p_ind && !trimmed.starts_with('-') {
                        let Some((k, v)) = trimmed.split_once(':') else {
                            bail!("line {}: malformed service property '{trimmed}'", line_no + 1);
                        };
                        let key = k.trim();
                        let val_raw = v.trim();
                        let val = unquote_yaml(val_raw);
                        cur_prop = Some(key.to_string());
                        cur_sub_prop = None;
                        sub_indent = None;

                        apply_service_property(svc, svc_env_list, key, &val, val_raw, base_dir)?;
                        continue;
                    }

                    // Nested items under `cur_prop` (lists, maps, and 3rd-level nested keys
                    // such as `build.args.*` or `depends_on.<svc>.condition`).
                    if let Some(prop) = cur_prop.as_deref() {
                        if let Some(item_raw) = trimmed.strip_prefix('-') {
                            let rest = item_raw.trim();
                            if prop == "env_file"
                                && let Some((ek, ev)) = rest.split_once(':')
                                && ek.trim() == "path"
                            {
                                let p = unquote_yaml(ev.trim());
                                if !p.is_empty() {
                                    svc.env_files.push(resolve_maybe_rel(&p, base_dir));
                                }
                            } else {
                                let item = unquote_yaml(rest);
                                if let Some(sp) = cur_sub_prop.as_deref()
                                    && prop == "build"
                                    && sp == "args"
                                {
                                    if !item.is_empty() {
                                        svc.build_args.push(item);
                                    }
                                } else {
                                    apply_list_item(svc, svc_env_list, prop, &item, base_dir);
                                }
                            }
                            continue;
                        }

                        if let Some((k, v)) = trimmed.split_once(':') {
                            let sub_key = unquote_yaml(k.trim());
                            let sub_val_raw = v.trim();
                            let sub_val = unquote_yaml(sub_val_raw);
                            let s_ind = *sub_indent.get_or_insert(indent);

                            if indent == s_ind {
                                cur_sub_prop = Some(sub_key.clone());
                                apply_sub_property(
                                    svc,
                                    svc_env_list,
                                    prop,
                                    &sub_key,
                                    &sub_val,
                                    sub_val_raw,
                                    base_dir,
                                );
                            } else if indent > s_ind
                                && let Some(sp) = cur_sub_prop.as_deref()
                            {
                                apply_deep_sub_property(svc, prop, sp, &sub_key, &sub_val);
                            }
                        }
                    }
                }
            }
        }

        // Merge any `include:` compose YAML files before final validation and sorting.
        for inc_path_str in included_files {
            let inc_path = Path::new(&inc_path_str);
            if let Ok(inc_raw) = std::fs::read_to_string(inc_path)
                && let Ok(inc_spec) =
                    Self::from_yaml_with_env(&inc_raw, inc_path.parent(), interp_vars)
            {
                for v in inc_spec.volumes {
                    if !top_volumes.contains(&v) {
                        top_volumes.push(v);
                    }
                }
                for (k, v) in inc_spec.secrets {
                    top_secrets.entry(k).or_insert(v);
                }
                for (k, v) in inc_spec.configs {
                    top_configs.entry(k).or_insert(v);
                }
                for svc in inc_spec.services {
                    let sname = svc.name.clone();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        services_map.entry(sname.clone())
                    {
                        service_order.push(sname);
                        e.insert(svc);
                    }
                }
            }
        }

        if services_map.is_empty() {
            bail!("Compose file defines no services under 'services:'");
        }

        // Load `env_file` entries first, then layer explicit `environment` keys on top so explicit
        // declarations always win over file defaults.
        merge_service_envs(&mut services_map, &explicit_envs, interp_vars)?;

        let ordered_names = topological_sort_services(&service_order, &services_map)?;
        let all_service_names: Vec<String> = ordered_names.clone();

        let mut ordered_services = Vec::with_capacity(ordered_names.len());
        for name in ordered_names {
            let mut svc = services_map.remove(&name).unwrap();
            // Inject sibling service names into `extra_hosts` (`<sibling>:127.0.0.1`).
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
            // Collect named volumes referenced in `svc.volumes`.
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
            // Bind-mount declared `secrets:` to `/run/secrets/<name>:ro` and `configs:` to `/<name>:ro`.
            for sec_name in &svc.secrets {
                if let Some(host_file) = top_secrets.get(sec_name) {
                    let mount = format!("{host_file}:/run/secrets/{sec_name}:ro");
                    if !svc.volumes.contains(&mount) {
                        svc.volumes.push(mount);
                    }
                }
            }
            for cfg_name in &svc.configs {
                if let Some(host_file) = top_configs.get(cfg_name) {
                    let mount = format!("{host_file}:/{cfg_name}:ro");
                    if !svc.volumes.contains(&mount) {
                        svc.volumes.push(mount);
                    }
                }
            }
            ordered_services.push(svc);
        }

        Ok(Self {
            project_name,
            services: ordered_services,
            volumes: top_volumes,
            secrets: top_secrets,
            configs: top_configs,
        })
    }
}
