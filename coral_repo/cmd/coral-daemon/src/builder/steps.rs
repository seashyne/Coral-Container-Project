//! Single-stage Dockerfile instruction interpreter (invoked per stage by the DAG executor).

use super::build::{BuildOptions, BuiltStage};
use super::buildkit::apply_metadata_instruction;
use super::cache::compute_step_key;
use super::context::resolve_or_pull_base_image;
use super::dag::StageNode;
use super::dockerfile::{
    apply_env_instruction, expand_vars, parse_exec_or_shell_instruction, parse_from_instruction,
    split_shell_tokens, sync_stage_vars_from_envs, unquote,
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

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_single_stage(
    node: &StageNode,
    total_steps: usize,
    stage_work: &Path,
    stage_rootfs: &Path,
    completed_stages: &HashMap<usize, BuiltStage>,
    stage_names: &HashMap<String, usize>,
    global_args: &HashMap<String, String>,
    build_arg_overrides: &HashMap<String, String>,
    opts: &BuildOptions<'_>,
    ts: i64,
) -> Result<BuiltStage> {
    let mut workdir = "/".to_string();
    let mut envs = vec![
        "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        "TERM=xterm".to_string(),
    ];
    let mut img_cfg = OciImageConfig::default();
    let mut vars: HashMap<String, String> = global_args.clone();

    let cache_root = opts.images_dir.join(".build-cache");
    let mut chain_key = String::from("coral-build-v1");
    let mut cache_active = !opts.no_cache;
    let active_stage_name = node.name.as_deref();

    for (local_idx, (verb, raw_arg)) in node.instructions.iter().enumerate() {
        let arg = expand_vars(raw_arg, &vars);
        let step_idx = node.step_offset + local_idx;
        let step_num = step_idx + 1;
        info!(step = step_num, total = total_steps, verb = %verb, arg = %arg, "building step");

        match verb.as_str() {
            "FROM" => {
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

                if stage_rootfs.exists() {
                    let _ = images::remove_dir_force(stage_rootfs);
                }
                fs::create_dir_all(stage_rootfs)?;

                // Reset per-stage build environment cleanly while keeping `--build-arg` + platform ARGs.
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
                if let Some(plat) = opts.platform {
                    h.update(b":plat:");
                    h.update(plat.as_bytes());
                }

                let prev_stage_idx = stage_names
                    .get(&from_info.base_ref.to_ascii_lowercase())
                    .copied()
                    .filter(|&i| i != node.index);
                if let Some(idx) = prev_stage_idx
                    && let Some(st) = completed_stages.get(&idx)
                {
                    envs = images::merge_env_vars(&envs, &st.envs, &[]);
                    workdir = st.workdir.clone();
                    img_cfg = st.img_cfg.clone();
                    h.update(st.chain_key.as_bytes());
                    let empty_upper = stage_work.join("empty-upper");
                    fs::create_dir_all(&empty_upper)?;
                    commit_overlay_to_rootfs(&st.rootfs_dir, &empty_upper, stage_rootfs)
                        .with_context(|| {
                            format!("clone stage '{}' rootfs", from_info.base_ref)
                        })?;
                    let _ = fs::remove_dir_all(&empty_upper);
                } else if from_info.base_ref != "scratch"
                    && from_info.base_ref != "scratch:latest"
                {
                    let base_path = resolve_or_pull_base_image(
                        &from_info.base_ref,
                        opts.images_dir,
                        opts.platform,
                    )
                    .await?;
                    let base_cfg = OciImageConfig::load_for_rootfs(&base_path);
                    envs = images::merge_env_vars(&envs, &base_cfg.env, &[]);
                    if !base_cfg.working_dir.trim().is_empty() {
                        workdir = base_cfg.working_dir.clone();
                    }
                    img_cfg = base_cfg;
                    h.update(base_path.to_string_lossy().as_bytes());
                    let empty_upper = stage_work.join("empty-upper");
                    fs::create_dir_all(&empty_upper)?;
                    commit_overlay_to_rootfs(&base_path, &empty_upper, stage_rootfs)
                        .with_context(|| {
                            format!("copy base rootfs from '{}'", from_info.base_ref)
                        })?;
                    let _ = fs::remove_dir_all(&empty_upper);
                }

                sync_stage_vars_from_envs(&envs, &mut vars);
                chain_key = format!("{:x}", h.finalize());
                cache_active = !opts.no_cache;
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
                        vars.insert(key, val);
                    } else {
                        let key = tok.trim().to_string();
                        if !key.is_empty() {
                            let val = build_arg_overrides
                                .get(&key)
                                .cloned()
                                .or_else(|| global_args.get(&key).cloned())
                                .unwrap_or_default();
                            vars.entry(key).or_insert(val);
                        }
                    }
                }
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "ENV" => {
                eprintln!("Step {step_num}/{total_steps} : ENV {arg}");
                apply_env_instruction(&arg, &mut vars, &mut envs);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "WORKDIR" => {
                eprintln!("Step {step_num}/{total_steps} : WORKDIR {arg}");
                let dir = arg.trim();
                if dir.starts_with('/') {
                    workdir = dir.to_string();
                } else if workdir == "/" {
                    workdir = format!("/{dir}");
                } else {
                    workdir = format!("{workdir}/{dir}");
                }
                let target = safe_join_rootfs(stage_rootfs, &workdir)?;
                fs::create_dir_all(&target)?;
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            "COPY" | "ADD" => {
                let exec = StepExec {
                    staging_dir: stage_work,
                    staging_rootfs: stage_rootfs,
                    opts,
                    cache_root: &cache_root,
                    stages: completed_stages,
                    stage_names,
                    active_stage_idx: node.index,
                    active_stage_name,
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
                let exec = StepExec {
                    staging_dir: stage_work,
                    staging_rootfs: stage_rootfs,
                    opts,
                    cache_root: &cache_root,
                    stages: completed_stages,
                    stage_names,
                    active_stage_idx: node.index,
                    active_stage_name,
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
            "EXPOSE" | "VOLUME" | "SHELL" | "HEALTHCHECK" | "ONBUILD" | "MAINTAINER" => {
                eprintln!("Step {step_num}/{total_steps} : {verb} {arg}");
                apply_metadata_instruction(verb, &arg, &mut img_cfg);
                chain_key = compute_step_key(&chain_key, verb, &arg, &workdir, &envs, b"");
            }
            other => {
                bail!("unsupported Dockerfile instruction '{other}'");
            }
        }
    }

    img_cfg.env = envs.clone();
    img_cfg.working_dir = workdir.clone();
    Ok(BuiltStage {
        rootfs_dir: stage_rootfs.to_path_buf(),
        chain_key,
        envs,
        workdir,
        img_cfg,
    })
}
