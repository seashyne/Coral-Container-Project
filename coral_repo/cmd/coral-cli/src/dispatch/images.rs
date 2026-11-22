//! `coral` image commands (`catalog`, `pull`, `images`, `rmi`, `build`, `tag`, `save`,
//! `load`, `push`) and OCI registry authentication (`login`, `logout`).

use super::{Ctx, run_async};
use crate::args::{
    BuildArgs, CatalogArgs, ImagesArgs, LoadArgs, LoginArgs, LogoutArgs, PullArgs, PushArgs,
    RmiArgs, SaveArgs, TagArgs,
};
use crate::daemon_client;
use anyhow::Result;
use std::process::ExitCode;

pub(crate) fn catalog(ctx: &Ctx, args: CatalogArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_catalog(
        &ctx.addr,
        &ctx.images_dir,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn pull(ctx: &Ctx, args: PullArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_pull(
        &ctx.addr,
        &ctx.images_dir,
        &args.image,
        args.rootfs.as_deref(),
        args.platform.as_deref(),
        args.local,
        &ctx.state_root,
        args.progress,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn images(ctx: &Ctx, args: ImagesArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_images(
        &ctx.addr,
        &ctx.images_dir,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn rmi(ctx: &Ctx, args: RmiArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_rmi(
        &ctx.addr,
        &ctx.images_dir,
        &args.image,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn build(ctx: &Ctx, args: BuildArgs) -> Result<ExitCode> {
    let BuildArgs {
        tag,
        file,
        target,
        build_args,
        platform,
        context,
        local,
        no_cache,
    } = args;
    let dockerfile_path = file.unwrap_or_else(|| context.join("Dockerfile"));
    run_async(daemon_client::cmd_build(
        &ctx.addr,
        &ctx.state_root,
        &ctx.images_dir,
        &tag,
        &dockerfile_path,
        &context,
        local,
        no_cache,
        target.as_deref(),
        &build_args,
        platform.as_deref(),
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn tag(ctx: &Ctx, args: TagArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_tag(
        &ctx.addr,
        &ctx.images_dir,
        &args.source,
        &args.target,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn save(ctx: &Ctx, args: SaveArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_save(
        &ctx.addr,
        &ctx.images_dir,
        &args.image,
        &args.output,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn load(ctx: &Ctx, args: LoadArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_load(
        &ctx.addr,
        &ctx.images_dir,
        &args.input,
        args.tag.as_deref(),
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn push(ctx: &Ctx, args: PushArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_push(
        &ctx.addr,
        &ctx.images_dir,
        &args.image,
        args.local,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn login(ctx: &Ctx, args: LoginArgs) -> Result<ExitCode> {
    let LoginArgs {
        registry,
        username,
        password,
        password_stdin,
    } = args;
    let pass = if password_stdin {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf.trim_end_matches(&['\r', '\n'][..]).to_string()
    } else if let Some(p) = password {
        p
    } else {
        anyhow::bail!("provide --password <TOKEN> or --password-stdin");
    };
    run_async(daemon_client::cmd_login(
        &ctx.addr, &registry, &username, &pass,
    ))?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn logout(ctx: &Ctx, args: LogoutArgs) -> Result<ExitCode> {
    run_async(daemon_client::cmd_logout(&ctx.addr, &args.registry))?;
    Ok(ExitCode::SUCCESS)
}
