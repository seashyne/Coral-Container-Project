//! `coral sandbox` — hardened AI-agent / untrusted-code sandbox execution.

use anyhow::{Context, Result};
use coral_daemon::images;
use coral_vmm::{SandboxPolicy, run_ai_sandbox};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[allow(clippy::too_many_arguments)]
pub fn cmd_sandbox_run(
    state_root: &Path,
    images_dir: &Path,
    image: String,
    cmd: Vec<String>,
    envs: Vec<String>,
    memory_mb: i64,
    cpu_percent: i64,
    pids_limit: i64,
    timeout_secs: u64,
    allow_network: bool,
    kernel_path: Option<PathBuf>,
    as_json: bool,
) -> Result<ExitCode> {
    let coral_bin = std::env::current_exe().context("resolve current coral binary")?;
    let rootfs_override = if Path::new(&image).is_dir() {
        fs::canonicalize(&image).ok()
    } else {
        let index = images::load_index(images_dir);
        images::find_image(&index, &image).map(|i| PathBuf::from(&i.rootfs_path))
    };

    let report = run_ai_sandbox(&SandboxPolicy {
        coral_bin,
        images_dir: images_dir.to_path_buf(),
        state_root: state_root.to_path_buf(),
        image,
        cmd,
        envs,
        memory_mb,
        cpu_percent,
        pids_limit,
        timeout_secs,
        allow_network,
        kernel_path,
        rootfs_override,
    })?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.stdout);
        if !report.stderr.is_empty() {
            eprint!("{}", report.stderr);
        }
        if report.timed_out {
            eprintln!(
                "coral sandbox: execution exceeded {}s timeout (mode={})",
                timeout_secs, report.isolation_mode
            );
        }
    }

    let code = u8::try_from(report.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}
