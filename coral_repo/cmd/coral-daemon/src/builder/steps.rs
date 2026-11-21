//! Dockerfile build-step interpreter.

use super::build::{BuiltStage, BuildOptions};
use super::cache::compute_step_key;
use super::context::resolve_or_pull_base_image;
use super::dockerfile::{
    expand_vars, parse_exec_or_shell_instruction, parse_from_instruction, set_env_var,
    split_shell_tokens,
    unquote,
};
use super::exec::{StepExec, StepInfo};
use super::step_copy::copy_or_add_step;
use super::step_run::run_step;
use crate::images::{self, OciImageConfig};
use anyhow::{Context, Result, bail};
use coral_runtime::overlay::commit_overlay_to_rootfs;
use coral_runtime::rootfs::safe_join_rootfs;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::info;

pub(super) async fn execute_build_steps(
    instructions: &[(String, String)],
    staging_dir: &Path,
    staging_rootfs: &Path,
    opts: &BuildOptions<'_>,
    ts: i64,
) -> Result<(String, OciImageConfig)> {
    let mut has_from = false;
    let mut workdir = "/".to_string();
    let mut envs = vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "TERM=xterm".to_string(),
    ];
    let mut img_cfg = OciImageConfig::default();

    let mut build_arg_overrides: HashMap<String, String> = HashMap::new();
    for raw_arg in opts.build_args {
        if let Some((k, v)) = raw_arg.split_once('=') {
            build_arg_overrides.insert(k.trim().to_string(), unquote(v.trim()));
        } else if !raw_arg.trim().is_empty()
            && let Ok(val) = std::env::var(raw_arg.trim())
        {
            build_arg_overrides.insert(raw_arg.trim().to_string(), val);
        }
    }
    let mut global_args: HashMap<String, String> = build_arg_overrides.clone();
    let mut vars: HashMap<String, String> = global_args.clone();

    let stages_dir = staging_dir.join("stages");
    fs::create_dir_all(&stages_dir)?;
    let mut stages: Vec<BuiltStage> = Vec::new();
    let mut stage_names: HashMap<String, usize> = HashMap::new();
    let mut active_stage_name: Option<String> = None;

    let target_stage = opts
        .target
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());

    let cache_root = opts.images_dir.join(".build-cache");
    let mut chain_key = String::from("coral-build-v1");
    let mut cache_active = !opts.no_cache;
    let total_steps = instructions.len();

    for (step_idx, (verb, raw_arg)) in instructions.iter().enumerate() {
        let arg = expand_vars(raw_arg, &vars);
        let step_num = step_idx + 1;
        info!(step = step_num, total = total_steps, verb = %verb, arg = %arg, "building step");

        match verb.as_str() {
            "FROM" => {
                // Snapshot completed previous stage before starting a new stage.
                if has_from {
                    let prev_idx = stages.len();
                    let snapshot_path = stages_dir.join(format!("stage-{prev_idx}"));
                    if snapshot_path.exists() {
                        let _ = images::remove_dir_force(&snapshot_path);
                    }
                    fs::rename(staging_rootfs, &snapshot_path).with_context(|| {
                        format!(
                            "snapshot stage {prev_idx} rootfs to '{}'",
                            snapshot_path.display()
                        )
                    })?;
                    fs::create_dir_all(staging_rootfs)?;

                    let mut snap_cfg = img_cfg.clone();
                    snap_cfg.env = envs.clone();
                    snap_cfg.working_dir = workdir.clone();

                    if let Some(ref sname) = active_stage_name {
                        stage_names.insert(sname.to_ascii_lowercase(), prev_idx);
                    }
                    stages.push(BuiltStage {
                        rootfs_dir: snapshot_path,
                        chain_key: chain_key.clone(),
                        envs: envs.clone(),
                        workdir: workdir.clone(),
                        img_cfg: snap_cfg,
                    });

                    // Stop early if `--target <stage>` just completed!
                    if let Some(ref wanted) = target_stage
                        && active_stage_name
                            .as_ref()
                            .is_some_and(|n| n.eq_ignore_ascii_case(wanted))
                    {
                        let chosen = stages.pop().expect("just pushed target stage");
                        let _ = images::remove_dir_force(staging_rootfs);
                        fs::rename(&chosen.rootfs_dir, staging_rootfs)?;
                        return Ok((chosen.chain_key, chosen.img_cfg));
                    }
                }

                let from_info = parse_from_instruction(&arg)?;
                eprintln!(
                    "Step {step_num}/{total_steps} : FROM {}{}",
                    from_info.base_ref,
                    from_info
                        .stage_name
                        .as_deref()
                        .map(|n| format!(" AS {n}"))
                        .unwrap_or_default()
                );

                if staging_rootfs.exists() {
                    let _ = images::remove_dir_force(staging_rootfs);
                }
                fs::create_dir_all(staging_rootfs)?;

                // Reset per-stage build environment cleanly while keeping global `--build-arg`s.
                workdir = "/".to_string();
                envs = vec![
                    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
                        .to_string(),
                    "TERM=xterm".to_string(),
                ];
                img_cfg = OciImageConfig::default();
                vars = build_arg_overrides.clone();

                let mut h = Sha256::new();
                h.update(b"FROM:");
                h.update(from_info.base_ref.as_bytes());

                let prev_stage_idx = stage_names
                    .get(&from_info.base_ref.to_ascii_lowercase())
                    .copied();
                if let Some(idx) = prev_stage_idx {
                    let st = &stages[idx];
                    envs = images::merge_env_vars(&envs, &st.envs, &[]);
                    workdir = st.workdir.clone();
                    img_cfg = st.img_cfg.clone();
                    h.update(st.chain_key.as_bytes());
                    let empty_upper = staging_dir.join("empty-upper");
                    fs::create_dir_all(&empty_upper)?;
                    commit_overlay_to_rootfs(&st.rootfs_dir, &empty_upper, staging_rootfs)
                        .with_context(|| {
                            format!("clone stage '{}' rootfs", from_info.base_ref)
                        })?;
                    let _ = fs::remove_dir_all(&empty_upper);
                } else if from_info.base_ref != "scratch"
                    && from_info.base_ref != "scratch:latest"
                {
                    let base_path =
                        resolve_or_pull_base_image(&from_info.base_ref, opts.images_dir).await?;
                    let base_cfg = OciImageConfig::load_for_rootfs(&base_path);
                    envs = images::merge_env_vars(&envs, &base_cfg.env, &[]);
                    if !base_cfg.working_dir.trim().is_empty() {
                        workdir = base_cfg.working_dir.clone();
                    }
                    img_cfg = base_cfg;
                    h.update(base_path.to_string_lossy().as_bytes());
                    let empty_upper = staging_dir.join("empty-upper");
                    fs::create_dir_all(&empty_upper)?;
                    commit_overlay_to_rootfs(&base_path, &empty_upper, staging_rootfs)
                        .with_context(|| {
                            format!("copy base rootfs from '{}'", from_info.base_ref)
                        })?;
                    let _ = fs::remove_dir_all(&empty_upper);
                }

                active_stage_name = from_info.stage_name;
                chain_key = format!("{:x}", h.finalize());
                cache_active = !opts.no_cache;
                has_from = true;
            }
            "ARG" => {
                eprintln!("Step {step_num}/{total_steps} : ARG {arg}");
                for tok in split_shell_tokens(&arg) {
                    if let Some((k, v)) = tok.split_once('=') {
                        let key = k.trim().to_string();
                        let val = build_arg_overrides
                            .get(&key)
                            .cloned()
                            .unwrap_or_else(|| unquote(v.trim()));
                        if !has_from {
                            global_args.insert(key.clone(), val.clone());
                        }
                        vars.insert(key, val);
                    } else {
                        let key = tok.trim().to_string();
                        if !key.is_empty() {
                            let val = build_arg_overrides
                                .get(&key)
                                .cloned()
                                .or_else(|| global_args.get(&key).cloned())
                                .unwrap_or_default();
                            if !has_from {
                                global_args.insert(key.clone(), val.clone());
                            }
                            vars.entry(key).or_insert(val);
                        }
                    }
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "ENV" => {
                eprintln!("Step {step_num}/{total_steps} : ENV {arg}");
                if let Some((k, v)) = arg.split_once('=') {
                    let key = k.trim().to_string();
                    let val = unquote(v.trim());
                    vars.insert(key.clone(), val.clone());
                    set_env_var(&mut envs, &key, &val);
                } else if let Some((k, v)) = arg.split_once(char::is_whitespace) {
                    let key = k.trim().to_string();
                    let val = unquote(v.trim());
                    vars.insert(key.clone(), val.clone());
                    set_env_var(&mut envs, &key, &val);
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "WORKDIR" => {
                if !has_from {
                    bail!("WORKDIR cannot appear before FROM");
                }
                eprintln!("Step {step_num}/{total_steps} : WORKDIR {arg}");
                let dir = arg.trim();
                if dir.starts_with('/') {
                    workdir = dir.to_string();
                } else if workdir == "/" {
                    workdir = format!("/{dir}");
                } else {
                    workdir = format!("{workdir}/{dir}");
                }
                let target = safe_join_rootfs(staging_rootfs, &workdir)?;
                fs::create_dir_all(&target)?;
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "COPY" | "ADD" => {
                if !has_from {
                    bail!("{verb} cannot appear before FROM");
                }
                let exec = StepExec {
                    staging_dir,
                    staging_rootfs,
                    opts,
                    cache_root: &cache_root,
                    stages: &stages,
                    stage_names: &stage_names,
                    active_stage_name: active_stage_name.as_deref(),
                    workdir: &workdir,
                    envs: &envs,
                    ts,
                };
                let outcome = copy_or_add_step(
                    &exec,
                    &StepInfo {
                        verb,
                        arg: &arg,
                        step_num,
                        total_steps,
                        step_idx,
                    },
                    &chain_key,
                    &mut cache_active,
                )
                .await?;
                chain_key = outcome.chain_key;
                if outcome.cached {
                    continue;
                }
            }
            "RUN" => {
                if !has_from {
                    bail!("RUN cannot appear before FROM");
                }
                let exec = StepExec {
                    staging_dir,
                    staging_rootfs,
                    opts,
                    cache_root: &cache_root,
                    stages: &stages,
                    stage_names: &stage_names,
                    active_stage_name: active_stage_name.as_deref(),
                    workdir: &workdir,
                    envs: &envs,
                    ts,
                };
                let outcome = run_step(
                    &exec,
                    &StepInfo {
                        verb,
                        arg: &arg,
                        step_num,
                        total_steps,
                        step_idx,
                    },
                    &chain_key,
                    &mut cache_active,
                )
                .await?;
                chain_key = outcome.chain_key;
                if outcome.cached {
                    continue;
                }
            }
            "CMD" => {
                eprintln!("Step {step_num}/{total_steps} : CMD {arg}");
                img_cfg.cmd = parse_exec_or_shell_instruction(&arg);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "ENTRYPOINT" => {
                eprintln!("Step {step_num}/{total_steps} : ENTRYPOINT {arg}");
                img_cfg.entrypoint = parse_exec_or_shell_instruction(&arg);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "USER" => {
                eprintln!("Step {step_num}/{total_steps} : USER {arg}");
                img_cfg.user = unquote(arg.trim());
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "STOPSIGNAL" => {
                eprintln!("Step {step_num}/{total_steps} : STOPSIGNAL {arg}");
                img_cfg.stop_signal = unquote(arg.trim());
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "LABEL" => {
                eprintln!("Step {step_num}/{total_steps} : LABEL {arg}");
                if let Some((k, v)) = arg.split_once('=') {
                    img_cfg
                        .labels
                        .insert(unquote(k.trim()), unquote(v.trim()));
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "EXPOSE" | "VOLUME" | "SHELL" | "HEALTHCHECK" | "MAINTAINER" => {
                eprintln!("Step {step_num}/{total_steps} : {verb} {arg}");
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            other => {
                bail!("unsupported Dockerfile instruction '{other}'");
            }
        }
    }

    if !has_from {
        bail!("Dockerfile is missing a FROM instruction");
    }
    if let Some(ref wanted) = target_stage {
        let is_last = active_stage_name
            .as_ref()
            .is_some_and(|n| n.eq_ignore_ascii_case(wanted));
        if !is_last {
            bail!(
                "target stage '{}' not found in Dockerfile",
                opts.target.unwrap_or(wanted)
            );
        }
    }
    img_cfg.env = envs;
    img_cfg.working_dir = workdir;
    Ok((chain_key, img_cfg))
}
