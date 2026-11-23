//! `coral` container commands (`diff`, `commit`, `inspect`, `cp`, `ps`, `logs`, `exec`,
//! `stop`, `rm`, `stats`).

use super::{Ctx, run_async};
use crate::args::{
    CommitArgs, CpArgs, DiffArgs, ExecArgs, InspectArgs, LogsArgs, PauseArgs, RenameArgs, RmArgs,
    StatsArgs, StopArgs, TopArgs, UnpauseArgs, UpdateArgs, WaitArgs,
};
use crate::cp;
use crate::daemon_client;
use crate::local_run::{commit_local_container, resolve_local_container_overlay};
use anyhow::{Context, Result};
use coral_cgroups::CgroupManager;
use coral_oci::OciSpec;
use coral_runtime::overlay::diff_overlay;
use coral_runtime::{ContainerManager, ExecOptions, StateStore};
use std::process::ExitCode;

pub(crate) fn diff(ctx: &Ctx, args: DiffArgs) -> Result<ExitCode> {
    let DiffArgs { id, local } = args;
    if !local
        && let Ok(Some(diff_txt)) = run_async(daemon_client::cmd_diff_via_daemon(&ctx.addr, &id))
    {
        if diff_txt.trim().is_empty() {
            println!("(no OverlayFS changes recorded for '{id}')");
        } else {
            println!("{diff_txt}");
        }
        return Ok(ExitCode::SUCCESS);
    }

    let (lower_rootfs, upperdir) = resolve_local_container_overlay(&ctx.state_root, &id)?;
    let changes = diff_overlay(&upperdir, Some(&lower_rootfs))
        .with_context(|| format!("inspect OverlayFS diff for '{id}'"))?;
    if changes.is_empty() {
        println!("(no OverlayFS changes recorded for '{id}')");
    } else {
        for ch in changes {
            println!("{ch}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn commit(ctx: &Ctx, args: CommitArgs) -> Result<ExitCode> {
    let CommitArgs {
        container,
        image,
        local,
    } = args;
    if !local
        && let Ok(Some(image_ref)) = run_async(daemon_client::cmd_commit_via_daemon(
            &ctx.addr, &container, &image,
        ))
    {
        println!("Committed {container} -> {image} ({image_ref})");
        return Ok(ExitCode::SUCCESS);
    }

    commit_local_container(&ctx.state_root, &ctx.images_dir, &container, &image)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn inspect(ctx: &Ctx, args: InspectArgs) -> Result<ExitCode> {
    let InspectArgs { id, local } = args;
    if !local
        && let Ok(Some(json)) = run_async(daemon_client::cmd_inspect_via_daemon(&ctx.addr, &id))
    {
        println!("{json}");
        return Ok(ExitCode::SUCCESS);
    }
    let st = StateStore::new(&ctx.state_root)
        .load(&id)
        .or_else(|_| StateStore::new(ctx.state_root.join("daemon/runtime-state")).load(&id))
        .with_context(|| format!("inspect container '{id}'"))?;
    let spec = OciSpec::from_file(st.bundle.join("config.json")).ok();
    let info = serde_json::json!({
        "state": st,
        "spec": spec,
    });
    println!("{}", serde_json::to_string_pretty(&info)?);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cp(ctx: &Ctx, args: CpArgs) -> Result<ExitCode> {
    run_async(cp::cmd_cp(&ctx.addr, &ctx.state_root, &args.src, &args.dst))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn ps(ctx: &Ctx) -> Result<ExitCode> {
    run_async(daemon_client::cmd_ps(&ctx.addr))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn logs(ctx: &Ctx, args: LogsArgs) -> Result<ExitCode> {
    let LogsArgs {
        id,
        status,
        follow,
        tail,
    } = args;
    run_async(daemon_client::cmd_logs(
        &ctx.addr, &id, status, follow, tail,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn exec(ctx: &Ctx, args: ExecArgs) -> Result<ExitCode> {
    let ExecArgs {
        id,
        local,
        interactive,
        tty,
        envs,
        workdir,
        timeout,
        cmd,
    } = args;
    if local || interactive || tty {
        // Single-threaded `setns()` directly into live container's namespaces!
        // Auto-detects containers running in either local state_root or coral-daemon's runtime-state.
        let exec_opts = ExecOptions {
            cmd,
            env: envs,
            cwd: workdir,
            tty,
            interactive,
        };
        let target_mgr = if StateStore::new(&ctx.state_root).load(&id).is_ok() {
            ctx.mgr.clone()
        } else {
            let daemon_rt = ctx.state_root.join("daemon/runtime-state");
            if StateStore::new(&daemon_rt).load(&id).is_ok() {
                ContainerManager::new(daemon_rt, CgroupManager::detect())
            } else {
                ctx.mgr.clone()
            }
        };
        let exit_code = target_mgr
            .exec(&id, &exec_opts)
            .with_context(|| format!("exec into container '{id}'"))?;
        let code = u8::try_from(exit_code.clamp(0, 255)).unwrap_or(1);
        return Ok(ExitCode::from(code));
    }
    run_async(daemon_client::cmd_exec(&ctx.addr, &id, timeout, cmd))
}

pub(crate) fn stop(ctx: &Ctx, args: StopArgs) -> Result<ExitCode> {
    let StopArgs { id, timeout } = args;
    run_async(daemon_client::cmd_stop(&ctx.addr, &id, timeout))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn pause(ctx: &Ctx, args: PauseArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_pause(&ctx.addr, &args.id))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn unpause(ctx: &Ctx, args: UnpauseArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_unpause(&ctx.addr, &args.id))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn rename(ctx: &Ctx, args: RenameArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_rename(&ctx.addr, &args.id, &args.new_name))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn update(ctx: &Ctx, args: UpdateArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_update(
        &ctx.addr,
        &args.id,
        args.memory.as_deref(),
        args.cpus,
        args.pids_limit,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn rm(ctx: &Ctx, args: RmArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_rm(&ctx.addr, &args.id))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn stats(ctx: &Ctx, args: StatsArgs) -> Result<ExitCode> {
    let StatsArgs {
        id,
        all,
        watch,
        interval,
    } = args;
    if all || id.is_none() {
        run_async(daemon_client::cmd_stats_all(&ctx.addr, watch, interval))?;
        return Ok(ExitCode::SUCCESS);
    }
    let cid = id.unwrap();
    if run_async(daemon_client::cmd_stats(&ctx.addr, &cid, watch, interval))? {
        return Ok(ExitCode::SUCCESS);
    }
    let stats = ctx
        .mgr
        .cgroups()
        .read_stats(&cid)
        .with_context(|| format!("read cgroup stats for '{cid}'"))?;
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn top(ctx: &Ctx, args: TopArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_top(&ctx.addr, &args.id, args.json))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn wait(ctx: &Ctx, args: WaitArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_wait(&ctx.addr, &args.id))?;
    Ok(ExitCode::SUCCESS)
}
