//! `coral compose` — multi-container stack commands.

use super::{Ctx, run_async};
use crate::args::{ComposeAction, ComposeArgs};
use crate::daemon_client;
use anyhow::Result;
use std::process::ExitCode;

pub(crate) fn compose(ctx: &Ctx, args: ComposeArgs) -> Result<ExitCode> {
    let ComposeArgs {
        file,
        project_name,
        env_file,
        profiles,
        action,
    } = args;
    match action {
        ComposeAction::Up {
            detach,
            build,
            wait,
            no_deps,
            scale,
            services,
        } => {
            run_async(daemon_client::cmd_compose_up(
                &ctx.addr,
                &ctx.state_root,
                &ctx.images_dir,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &profiles,
                &services,
                detach,
                build,
                wait,
                no_deps,
                &scale,
            ))?;
        }
        ComposeAction::Down { volumes } => {
            run_async(daemon_client::cmd_compose_down(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                volumes,
            ))?;
        }
        ComposeAction::Ps { all } => {
            run_async(daemon_client::cmd_compose_ps(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                all,
            ))?;
        }
        ComposeAction::Logs {
            follow,
            tail,
            service,
        } => {
            run_async(daemon_client::cmd_compose_logs(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                service.as_deref(),
                follow,
                tail,
            ))?;
        }
        ComposeAction::Build { no_cache, services } => {
            run_async(daemon_client::cmd_compose_build(
                &ctx.addr,
                &ctx.state_root,
                &ctx.images_dir,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &profiles,
                &services,
                no_cache,
            ))?;
        }
        ComposeAction::Start { services } => {
            run_async(daemon_client::cmd_compose_start(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &services,
            ))?;
        }
        ComposeAction::Stop { timeout, services } => {
            run_async(daemon_client::cmd_compose_stop(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &services,
                timeout,
            ))?;
        }
        ComposeAction::Restart { timeout, services } => {
            run_async(daemon_client::cmd_compose_restart(
                &ctx.addr,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &services,
                timeout,
            ))?;
        }
        ComposeAction::Exec {
            interactive,
            tty,
            envs,
            workdir,
            timeout,
            service,
            cmd,
        } => {
            return run_async(daemon_client::cmd_compose_exec(
                &ctx.addr,
                &ctx.state_root,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &service,
                interactive,
                tty,
                envs,
                workdir,
                timeout,
                cmd,
            ));
        }
        ComposeAction::Watch {
            interval_ms,
            services,
        } => {
            run_async(daemon_client::cmd_compose_watch(
                &ctx.addr,
                &ctx.state_root,
                &ctx.images_dir,
                file.as_deref(),
                env_file.as_deref(),
                project_name.as_deref(),
                &profiles,
                &services,
                interval_ms,
            ))?;
        }
        ComposeAction::Config => {
            daemon_client::cmd_compose_config(file.as_deref(), env_file.as_deref())?;
        }
    }
    Ok(ExitCode::SUCCESS)
}
