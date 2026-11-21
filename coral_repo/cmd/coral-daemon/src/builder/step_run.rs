//! `RUN` Dockerfile instruction execution.

use super::cache::{compute_step_key, restore_cached_rootfs, save_cached_rootfs};
use super::exec::{StepExec, StepInfo};
use crate::images;
use anyhow::{Context, Result, bail};
use coral_oci::OciSpec;
use coral_runtime::rootfs::host_dns_mounts;
use std::fs;
use std::process::Stdio;
use tokio::process::Command;

/// Outcome of a `RUN` step.
pub(super) struct RunOutcome {
    /// Chain key after the step.
    pub chain_key: String,
    /// `true` when the step was served from the build cache.
    pub cached: bool,
}

/// Executes a `RUN` instruction inside an isolated Coral container.
pub(super) async fn run_step(
    exec: &StepExec<'_, '_>,
    step: &StepInfo<'_>,
    chain_key: &str,
    cache_active: &mut bool,
) -> Result<RunOutcome> {
    let verb = step.verb;
    let arg = step.arg;
    let next_key = compute_step_key(chain_key, verb, arg, exec.workdir, exec.envs, b"");
    let cache_dir = exec.cache_root.join(&next_key).join("rootfs");

    if *cache_active && cache_dir.is_dir() {
        eprintln!(
            "Step {}/{} : RUN {arg} [CACHED]",
            step.step_num, step.total_steps
        );
        restore_cached_rootfs(&cache_dir, exec.staging_dir, exec.staging_rootfs)?;
        return Ok(RunOutcome {
            chain_key: next_key,
            cached: true,
        });
    }

    *cache_active = false;
    eprintln!(
        "Step {}/{} : RUN {arg}",
        step.step_num, step.total_steps
    );
    let cmd_args = if arg.trim_start().starts_with('[')
        && let Ok(parsed) = serde_json::from_str::<Vec<String>>(arg.trim())
        && !parsed.is_empty()
    {
        parsed
    } else {
        vec!["/bin/sh".into(), "-c".into(), arg.to_string()]
    };

    let step_bundle = exec.staging_dir.join(format!("step-{}", step.step_idx));
    fs::create_dir_all(&step_bundle)?;
    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
    if let Some(r) = spec.root.as_mut() {
        r.path = fs::canonicalize(exec.staging_rootfs)?
            .to_string_lossy()
            .into_owned();
    }
    if let Some(p) = spec.process.as_mut() {
        p.args = cmd_args;
        p.cwd = exec.workdir.to_string();
        p.env = exec.envs.to_vec();
    }
    spec.mounts.retain(|m| m.destination != "/tmp");
    spec.mounts.extend(host_dns_mounts());
    spec.annotations.insert("coral.overlay".into(), "false".into());
    spec.to_file(step_bundle.join("config.json"))?;

    let build_state_root = exec.opts.state_dir.join("build-state");
    let ctr_id = format!("build-{}-{}", exec.ts, step.step_idx);
    let out = Command::new(exec.opts.coral_bin)
        .arg("--root")
        .arg(&build_state_root)
        .arg("run")
        .arg("--bundle")
        .arg(&step_bundle)
        .arg(&ctr_id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .context("spawn coral for Dockerfile RUN step")?;
    let _ = images::remove_dir_force(&step_bundle);

    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "Dockerfile RUN step failed (`RUN {arg}`, exit {:?}):\n{}{}",
            out.status.code(),
            stdout,
            stderr
        );
    }

    if !exec.opts.no_cache {
        let _ = save_cached_rootfs(exec.staging_rootfs, exec.staging_dir, &cache_dir);
    }
    Ok(RunOutcome {
        chain_key: next_key,
        cached: false,
    })
}
