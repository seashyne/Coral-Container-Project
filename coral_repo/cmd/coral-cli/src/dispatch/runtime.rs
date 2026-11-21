//! `coral run` — run a container from an OCI image (or OCI bundle), locally or via
//! `coral-daemon`.

use super::{Ctx, run_async};
use crate::args::RunArgs;
use crate::daemon_client::{self, DaemonRunOptions};
use crate::local_run::{LocalImageRunArgs, auto_container_id, run_from_image_local};
use crate::nextgen;
use crate::run_resolve::{ResolvedRunTarget, resolve_run_target};
use crate::units::PullPolicy;
use anyhow::{Context, Result};
use coral_daemon::images;
use coral_runtime::RunOptions;
use std::path::Path;
use std::process::ExitCode;

/// Ensures an image is present in `images_dir` via a child `coral pull --local` process
/// so the current CLI process stays strictly single-threaded before POSIX `fork()`/`unshare()`.
fn ensure_local_image_pulled(
    images_dir: &Path,
    image_ref: &str,
    pull_policy: PullPolicy,
) -> Result<()> {
    if pull_policy == PullPolicy::Never || Path::new(image_ref).is_dir() {
        return Ok(());
    }
    let needs_pull = if pull_policy == PullPolicy::Always {
        true
    } else {
        let index = images::load_index(images_dir);
        images::find_image(&index, image_ref).is_none()
    };
    if !needs_pull {
        return Ok(());
    }
    eprintln!("Unable to find image '{image_ref}' locally");
    let exe = std::env::current_exe().context("resolve current coral executable")?;
    let status = std::process::Command::new(exe)
        .arg("--images-dir")
        .arg(images_dir)
        .args(["pull", "--local", image_ref])
        .status()
        .with_context(|| format!("auto-pull image '{image_ref}'"))?;
    if !status.success() {
        anyhow::bail!("failed to auto-pull image '{image_ref}'");
    }
    Ok(())
}

pub(crate) fn run(ctx: &Ctx, args: RunArgs) -> Result<ExitCode> {
    let RunArgs {
        bundle,
        image,
        pull,
        runtime,
        name,
        interactive,
        tty,
        volumes,
        ports,
        network,
        dns,
        extra_hosts,
        no_overlay,
        privileged,
        envs,
        memory_mb,
        cpu_percent,
        pids_limit,
        workdir,
        user,
        entrypoint,
        hostname,
        restart,
        health_cmd,
        health_interval,
        health_retries,
        daemon,
        detach,
        keep,
        rm,
        target,
        cmd,
    } = args;

    let keep = keep && !rm;
    let resolved = resolve_run_target(
        bundle,
        image,
        name,
        target,
        cmd,
        Path::new("config.json").is_file(),
        |t| {
            let index = images::load_index(&ctx.images_dir);
            images::find_image(&index, t).is_some()
        },
    )?;

    match resolved {
        ResolvedRunTarget::Image {
            image_ref,
            explicit_id,
            cmd,
        } => {
            // Direct Wasm runtime or auto-detected Wasm OCI image!
            if runtime.eq_ignore_ascii_case("wasm")
                || (!daemon
                    && !detach
                    && runtime.eq_ignore_ascii_case("oci")
                    && nextgen::resolve_wasm_target(&ctx.images_dir, &image_ref).is_ok())
            {
                let mem = if memory_mb > 0 {
                    memory_mb as usize
                } else {
                    64
                };
                return nextgen::cmd_wasm_run(
                    &ctx.images_dir,
                    &image_ref,
                    50_000_000,
                    mem,
                    envs,
                    cmd,
                    false,
                );
            }
            // Direct AI Agent Sandbox / MicroVM runtime!
            if runtime.eq_ignore_ascii_case("vmm") || runtime.eq_ignore_ascii_case("sandbox") {
                return nextgen::cmd_sandbox_run(
                    &ctx.state_root,
                    &ctx.images_dir,
                    image_ref,
                    cmd,
                    envs,
                    if memory_mb > 0 { memory_mb } else { 64 },
                    if cpu_percent > 0 { cpu_percent } else { 50 },
                    if pids_limit > 0 { pids_limit } else { 32 },
                    30,
                    !network.eq_ignore_ascii_case("none"),
                    None,
                    false,
                );
            }

            let container_id = explicit_id.unwrap_or_else(|| auto_container_id(&image_ref));

            if daemon || detach {
                return run_async(daemon_client::cmd_run_via_daemon(
                    &ctx.addr,
                    DaemonRunOptions {
                        name: container_id,
                        image: image_ref,
                        command: cmd,
                        envs,
                        volumes,
                        ports,
                        network,
                        dns,
                        extra_hosts,
                        privileged,
                        no_overlay,
                        workdir,
                        user,
                        entrypoint,
                        hostname,
                        memory_mb,
                        cpu_percent,
                        pids_limit,
                        restart,
                        health_cmd,
                        health_interval_secs: health_interval,
                        health_retries,
                        detach,
                        rm,
                        pull_policy: pull,
                    },
                ));
            }

            ensure_local_image_pulled(&ctx.images_dir, &image_ref, pull)?;

            run_from_image_local(
                &ctx.mgr,
                &ctx.state_root,
                &ctx.images_dir,
                LocalImageRunArgs {
                    image: image_ref,
                    container_id,
                    cmd,
                    envs,
                    volumes,
                    ports,
                    network,
                    dns,
                    extra_hosts,
                    no_overlay,
                    privileged,
                    tty,
                    interactive,
                    memory_mb,
                    cpu_percent,
                    pids_limit,
                    workdir,
                    user,
                    entrypoint,
                    hostname,
                    keep,
                },
            )
        }

        ResolvedRunTarget::Bundle {
            bundle_dir,
            container_id,
        } => {
            // Standard OCI bundle mode: MUST remain single-threaded (zero Tokio runtime).
            let exit = ctx
                .mgr
                .run(
                    &container_id,
                    &bundle_dir,
                    &RunOptions {
                        keep_state: keep,
                        tty,
                        interactive,
                    },
                )
                .with_context(|| format!("run container '{container_id}'"))?;
            let code = u8::try_from(exit.exit_code.clamp(0, 255)).unwrap_or(1);
            Ok(ExitCode::from(code))
        }
    }
}
