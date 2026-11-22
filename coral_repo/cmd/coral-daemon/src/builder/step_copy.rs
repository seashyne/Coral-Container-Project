//! `COPY` / `ADD` Dockerfile instruction execution.

use super::cache::{compute_step_key, hash_sources, restore_cached_rootfs, save_cached_rootfs};
use super::context::{copy_context_item, resolve_or_pull_base_image, safe_join_context};
use super::dockerfile::parse_copy_args;
use super::exec::{StepExec, StepInfo};
use anyhow::{Result, anyhow, bail};
use coral_runtime::rootfs::safe_join_rootfs;
use tracing::warn;

/// Outcome of a `COPY` / `ADD` step.
pub(super) struct CopyOutcome {
    /// Chain key after the step.
    pub chain_key: String,
    /// `true` when the step was served from the build cache.
    pub cached: bool,
}

/// Resolves the copy sources, applies them to the staging rootfs, and updates the build cache.
pub(super) async fn copy_or_add_step(
    exec: &StepExec<'_, '_>,
    step: &StepInfo<'_>,
    chain_key: &str,
    cache_active: &mut bool,
) -> Result<CopyOutcome> {
    let verb = step.verb;
    let arg = step.arg;
    let (from_ref, path_args) = parse_copy_args(arg);
    if path_args.len() < 2 {
        bail!("{verb} requires <src>... <dest>");
    }
    let sources = &path_args[..path_args.len() - 1];
    let dest_raw = &path_args[path_args.len() - 1];

    // Resolve either a build context source or `--from=<stage|index|image>` source.
    let (src_root, is_stage_src, extra_tag) = if let Some(ref fref) = from_ref {
        if let Some(&idx) = exec.stage_names.get(&fref.to_ascii_lowercase()) {
            let st = &exec.stages[idx];
            (st.rootfs_dir.clone(), true, st.chain_key.as_bytes().to_vec())
        } else if let Ok(idx) = fref.parse::<usize>()
            && idx < exec.stages.len()
        {
            let st = &exec.stages[idx];
            (st.rootfs_dir.clone(), true, st.chain_key.as_bytes().to_vec())
        } else if exec
            .active_stage_name
            .is_some_and(|n| n.eq_ignore_ascii_case(fref))
            || fref.parse::<usize>().ok() == Some(exec.stages.len())
        {
            bail!("{verb} --from='{fref}' cannot reference the currently building stage");
        } else {
            let img_root = resolve_or_pull_base_image(fref, exec.opts.images_dir).await?;
            let tag_bytes = img_root.to_string_lossy().as_bytes().to_vec();
            (img_root, true, tag_bytes)
        }
    } else {
        let ctx_dir = exec.opts.context_dir.ok_or_else(|| {
            anyhow!("{verb} instruction requires a build context directory")
        })?;
        (ctx_dir.to_path_buf(), false, Vec::new())
    };

    let src_hash = hash_sources(&src_root, sources, is_stage_src, &extra_tag)?;
    let next_key = compute_step_key(chain_key, verb, arg, exec.workdir, exec.envs, &src_hash);
    let cache_dir = exec.cache_root.join(&next_key).join("rootfs");

    if *cache_active && cache_dir.is_dir() {
        eprintln!(
            "Step {}/{} : {verb} {arg} [CACHED]",
            step.step_num, step.total_steps
        );
        restore_cached_rootfs(&cache_dir, exec.staging_dir, exec.staging_rootfs)?;
        return Ok(CopyOutcome {
            chain_key: next_key,
            cached: true,
        });
    }

    *cache_active = false;
    eprintln!(
        "Step {}/{} : {verb} {arg}",
        step.step_num, step.total_steps
    );
    let dest_in_container = if dest_raw.starts_with('/') {
        dest_raw.clone()
    } else if exec.workdir == "/" {
        format!("/{dest_raw}")
    } else {
        format!("{}/{dest_raw}", exec.workdir)
    };
    let dest_target = safe_join_rootfs(exec.staging_rootfs, &dest_in_container)?;

    for src_rel in sources {
        let src_path = if is_stage_src {
            safe_join_rootfs(&src_root, src_rel)?
        } else {
            safe_join_context(&src_root, src_rel)?
        };
        copy_context_item(&src_path, &dest_target, dest_raw.ends_with('/'))?;
    }
    if !exec.opts.no_cache
        && let Err(err) = save_cached_rootfs(exec.staging_rootfs, exec.staging_dir, &cache_dir)
    {
        warn!(step = step.step_num, error = %err, "failed to save COPY/ADD build cache entry");
    }
    Ok(CopyOutcome {
        chain_key: next_key,
        cached: false,
    })
}
