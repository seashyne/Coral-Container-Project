//! Parallel Multi-Stage BuildKit DAG executor: executes independent Dockerfile stages
//! concurrently in topological waves using `futures_util::future::try_join_all`.

use super::build::{BuildOptions, BuiltStage};
use super::dag::{
    buildkit_platform_args, check_cross_arch_qemu, plan_stage_waves, split_into_stages,
};
use super::dockerfile::unquote;
use super::steps::execute_single_stage;
use crate::images::{self, OciImageConfig};
use anyhow::{Context, Result};
use futures_util::future::try_join_all;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

/// Executes a single-stage or multi-stage Dockerfile using topological parallel DAG waves
/// and dead-stage elimination, returning `(final_chain_key, final_oci_config)`.
pub(super) async fn execute_dag_build(
    instructions: &[(String, String)],
    staging_dir: &Path,
    staging_rootfs: &Path,
    opts: &BuildOptions<'_>,
    ts: i64,
) -> Result<(String, OciImageConfig)> {
    // 1. Seed predefined BuildKit platform ARGs + user `--build-arg` overrides.
    let mut build_arg_overrides = buildkit_platform_args(opts.platform);
    if let Some(target_arch) = build_arg_overrides.get("TARGETARCH")
        && let Err(msg) = check_cross_arch_qemu(target_arch)
    {
        warn!("{msg}");
        eprintln!("Warning: {msg}");
    }
    for raw_arg in opts.build_args {
        if let Some((k, v)) = raw_arg.split_once('=') {
            build_arg_overrides.insert(k.trim().to_string(), unquote(v.trim()));
        } else if !raw_arg.trim().is_empty()
            && let Ok(val) = std::env::var(raw_arg.trim())
        {
            build_arg_overrides.insert(raw_arg.trim().to_string(), val);
        }
    }

    // 2. Partition instructions into stages and plan topological parallel waves.
    let (global_args, stages, stage_names) =
        split_into_stages(instructions, &build_arg_overrides)?;
    let (waves, target_idx) = plan_stage_waves(&stages, opts.target)?;
    let total_steps = instructions.len();

    let stages_dir = staging_dir.join("stages");
    fs::create_dir_all(&stages_dir)?;
    let mut completed: HashMap<usize, BuiltStage> = HashMap::new();

    for (wave_idx, wave) in waves.iter().enumerate() {
        if wave.len() > 1 {
            info!(
                wave = wave_idx,
                parallel_stages = wave.len(),
                "executing independent Dockerfile stages concurrently"
            );
            eprintln!(
                "BuildKit DAG wave {}: executing {} independent stages in parallel...",
                wave_idx + 1,
                wave.len()
            );
        }

        let completed_ref = &completed;
        let stage_names_ref = &stage_names;
        let global_args_ref = &global_args;
        let build_args_ref = &build_arg_overrides;
        let stages_dir_ref = &stages_dir;

        let futs = wave.iter().map(|&stage_idx| {
            let node = &stages[stage_idx];
            async move {
                let stage_work = staging_dir.join(format!("work-stage-{stage_idx}"));
                let stage_rootfs = stages_dir_ref.join(format!("stage-{stage_idx}"));
                if stage_rootfs.exists() {
                    let _ = images::remove_dir_force(&stage_rootfs);
                }
                fs::create_dir_all(&stage_work)?;
                fs::create_dir_all(&stage_rootfs)?;

                let built = execute_single_stage(
                    node,
                    total_steps,
                    &stage_work,
                    &stage_rootfs,
                    completed_ref,
                    stage_names_ref,
                    global_args_ref,
                    build_args_ref,
                    opts,
                    ts,
                )
                .await?;
                let _ = images::remove_dir_force(&stage_work);
                Ok::<(usize, BuiltStage), anyhow::Error>((stage_idx, built))
            }
        });

        let finished = try_join_all(futs).await?;
        for (idx, built) in finished {
            completed.insert(idx, built);
        }
    }

    let final_stage = completed
        .remove(&target_idx)
        .context("target stage missing after DAG execution")?;
    if staging_rootfs.exists() {
        let _ = images::remove_dir_force(staging_rootfs);
    }
    fs::rename(&final_stage.rootfs_dir, staging_rootfs).with_context(|| {
        format!(
            "promote final stage rootfs '{}' to '{}'",
            final_stage.rootfs_dir.display(),
            staging_rootfs.display()
        )
    })?;
    Ok((final_stage.chain_key, final_stage.img_cfg))
}
