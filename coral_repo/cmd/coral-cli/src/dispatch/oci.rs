//! Local OCI runtime commands (`create`, `start`, `state`, `list`, `kill`, `delete`,
//! `spec`, `cni-config`) and shell completion generation.

use super::Ctx;
use crate::args::{
    Cli, CniConfigArgs, CreateArgs, DeleteArgs, KillArgs, SpecArgs, StartArgs, StateArgs,
};
use anyhow::{Context, Result};
use clap::CommandFactory;
use coral_net::CniNetworkConfig;
use coral_oci::OciSpec;
use coral_runtime::container::parse_signal;
use std::process::ExitCode;

pub(crate) fn create(ctx: &Ctx, args: CreateArgs) -> Result<ExitCode> {
    let CreateArgs { bundle, id } = args;
    let state = ctx
        .mgr
        .create(&id, &bundle)
        .with_context(|| format!("create container '{id}'"))?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn start(ctx: &Ctx, args: StartArgs) -> Result<ExitCode> {
    let StartArgs { id } = args;
    let state = ctx
        .mgr
        .start(&id)
        .with_context(|| format!("start container '{id}'"))?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn state(ctx: &Ctx, args: StateArgs) -> Result<ExitCode> {
    let StateArgs { id } = args;
    let state = ctx
        .mgr
        .state(&id)
        .with_context(|| format!("read state for '{id}'"))?;
    println!("{}", serde_json::to_string_pretty(&state)?);
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn list(ctx: &Ctx) -> Result<ExitCode> {
    let list = ctx.mgr.list().context("list containers")?;
    println!(
        "{:<20} {:<10} {:<10} {:<26} BUNDLE",
        "ID", "PID", "STATUS", "CREATED"
    );
    for item in list {
        let pid_str = item
            .pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<20} {:<10} {:<10} {:<26} {}",
            item.id,
            pid_str,
            item.status,
            item.created.to_rfc3339(),
            item.bundle.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn kill(ctx: &Ctx, args: KillArgs) -> Result<ExitCode> {
    let KillArgs { id, signal } = args;
    let sig = parse_signal(&signal)?;
    ctx.mgr
        .kill(&id, sig)
        .with_context(|| format!("kill container '{id}' with {sig}"))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn delete(ctx: &Ctx, args: DeleteArgs) -> Result<ExitCode> {
    let DeleteArgs { force, id } = args;
    let cli_bundle = ctx.mgr.state(&id).ok().map(|s| s.bundle);
    ctx.mgr
        .delete(&id, force)
        .with_context(|| format!("delete container '{id}'"))?;
    if let Some(bundle_path) = cli_bundle
        && bundle_path.starts_with(ctx.state_root.join(".cli-bundles"))
    {
        let _ = coral_daemon::images::remove_dir_force(&bundle_path);
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn spec(_ctx: &Ctx, args: SpecArgs) -> Result<ExitCode> {
    let SpecArgs {
        rootless,
        rootfs,
        output,
    } = args;
    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let use_rootless = rootless || host_uid != 0;
    let mut spec = OciSpec::default_spec(use_rootless, host_uid, host_gid);
    if let Some(r) = spec.root.as_mut() {
        r.path = rootfs;
    }
    spec.to_file(&output)
        .with_context(|| format!("write OCI spec to {}", output.display()))?;
    println!(
        "Wrote OCI v{} spec to {}",
        spec.oci_version,
        output.display()
    );
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn cni_config(_ctx: &Ctx, args: CniConfigArgs) -> Result<ExitCode> {
    let CniConfigArgs { output } = args;
    let cfg = CniNetworkConfig::default_bridge();
    if let Some(path) = output {
        cfg.to_file(&path)?;
        println!(
            "Wrote CNI v{} config to {}",
            cfg.cni_version,
            path.display()
        );
    } else {
        println!("{}", serde_json::to_string_pretty(&cfg)?);
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn completion(_ctx: &Ctx, args: crate::args::CompletionArgs) -> Result<ExitCode> {
    use std::io::Write;
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    clap_complete::generate(args.shell, &mut cmd, "coral", &mut buf);
    if let Err(e) = std::io::stdout().write_all(&buf)
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e.into());
    }
    Ok(ExitCode::SUCCESS)
}
