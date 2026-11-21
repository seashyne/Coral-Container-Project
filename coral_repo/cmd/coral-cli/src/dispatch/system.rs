//! `coral system` (disk usage & prune), `coral pods` (CRI pod sandboxes) and daemon
//! status commands (`version`, `daemon-version`, `cri-status`).

use super::{Ctx, run_async};
use crate::args::{CriStatusArgs, PodAction, SystemAction};
use crate::daemon_client;
use anyhow::Result;
use std::process::ExitCode;

pub(crate) fn system(ctx: &Ctx, args: crate::args::SystemArgs) -> Result<ExitCode> {
    match args.action {
        SystemAction::Df => {
            run_async(daemon_client::cmd_system_df(
                &ctx.addr,
                &ctx.state_root,
                &ctx.images_dir,
            ))?;
        }
        SystemAction::Prune { all, volumes } => {
            run_async(daemon_client::cmd_system_prune(&ctx.addr, all, volumes))?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn pods(ctx: &Ctx, args: crate::args::PodsArgs) -> Result<ExitCode> {
    match args.action.unwrap_or(PodAction::Ls) {
        PodAction::Ls => run_async(daemon_client::cmd_pods_list(&ctx.addr))?,
        PodAction::Stop { id } => run_async(daemon_client::cmd_pods_stop(&ctx.addr, &id))?,
        PodAction::Rm { id } => run_async(daemon_client::cmd_pods_rm(&ctx.addr, &id))?,
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn version(ctx: &Ctx) -> Result<ExitCode> {
    run_async(daemon_client::cmd_version(&ctx.addr, false))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn daemon_version(ctx: &Ctx) -> Result<ExitCode> {
    run_async(daemon_client::cmd_version(&ctx.addr, true))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cri_status(ctx: &Ctx, args: CriStatusArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_cri_status(&ctx.addr, args.verbose))?;
    Ok(ExitCode::SUCCESS)
}
