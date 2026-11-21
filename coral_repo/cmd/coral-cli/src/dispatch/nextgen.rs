//! `coral` next-generation workload commands (`wasm`, `vm`, `sandbox`) plus image signing
//! and SBOM generation (`keygen`, `sign`, `verify`, `sbom`).

use super::{Ctx, run_async};
use crate::args::{
    KeygenArgs, SandboxAction, SandboxArgs, SbomArgs, SignArgs, VerifyArgs, VmAction, VmArgs,
    VmDiskAction, WasmAction, WasmArgs,
};
use crate::nextgen;
use anyhow::Result;
use std::process::ExitCode;

pub(crate) fn wasm(ctx: &Ctx, args: WasmArgs) -> Result<ExitCode> {
    match args.action {
        WasmAction::Run {
            target,
            fuel,
            memory_mb,
            envs,
            json,
            args,
        } => nextgen::cmd_wasm_run(&ctx.images_dir, &target, fuel, memory_mb, envs, args, json),
        WasmAction::Pack { file, tag } => {
            nextgen::cmd_wasm_pack(&ctx.images_dir, &file, &tag)?;
            Ok(ExitCode::SUCCESS)
        }
        WasmAction::Serve {
            target,
            listen,
            fuel,
            memory_mb,
            envs,
        } => {
            run_async(nextgen::cmd_wasm_serve(
                &ctx.images_dir,
                &target,
                &listen,
                fuel,
                memory_mb,
                envs,
            ))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

pub(crate) fn vm(ctx: &Ctx, args: VmArgs) -> Result<ExitCode> {
    match args.action {
        VmAction::Probe { json } => {
            nextgen::cmd_vm_probe(json)?;
            Ok(ExitCode::SUCCESS)
        }
        VmAction::SetupKernel { from, dest, json } => {
            nextgen::cmd_vm_setup_kernel(from, dest, json)?;
            Ok(ExitCode::SUCCESS)
        }
        VmAction::Boot {
            image,
            kernel,
            memory_mb,
            timeout,
            cmdline,
            disk,
            disk_mount,
            net,
            ports,
            json,
            cmd,
        } => nextgen::cmd_vm_boot(
            &ctx.state_root,
            &ctx.images_dir,
            &image,
            kernel,
            memory_mb,
            timeout,
            cmdline,
            disk,
            disk_mount,
            net || !ports.is_empty(),
            ports,
            &cmd,
            json,
        ),
        VmAction::Disk { action } => match action {
            VmDiskAction::Create {
                path,
                size_mb,
                ext4,
            } => {
                nextgen::cmd_vm_disk_create(&path, size_mb, ext4)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        VmAction::Run {
            payload,
            kernel,
            initrd,
            cmdline,
            memory_mb,
            timeout,
            max_exits,
            json,
        } => nextgen::cmd_vm_run(
            payload, kernel, initrd, cmdline, memory_mb, timeout, max_exits, json,
        ),
    }
}

pub(crate) fn sandbox(ctx: &Ctx, args: SandboxArgs) -> Result<ExitCode> {
    match args.action {
        SandboxAction::Run {
            image,
            timeout,
            memory_mb,
            cpu_percent,
            pids_limit,
            allow_network,
            kernel,
            envs,
            json,
            cmd,
        } => nextgen::cmd_sandbox_run(
            &ctx.state_root,
            &ctx.images_dir,
            image,
            cmd,
            envs,
            memory_mb,
            cpu_percent,
            pids_limit,
            timeout,
            allow_network,
            kernel,
            json,
        ),
    }
}

pub(crate) fn keygen(_ctx: &Ctx, args: KeygenArgs) -> Result<ExitCode> {
    let KeygenArgs {
        key,
        pub_key,
        force,
        json,
    } = args;
    nextgen::cmd_keygen(key, pub_key, force, json)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn sign(ctx: &Ctx, args: SignArgs) -> Result<ExitCode> {
    let SignArgs { image, key, json } = args;
    nextgen::cmd_sign(&ctx.images_dir, &image, key, json)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn verify(ctx: &Ctx, args: VerifyArgs) -> Result<ExitCode> {
    let VerifyArgs { image, key, json } = args;
    nextgen::cmd_verify(&ctx.images_dir, &image, key, json)?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn sbom(ctx: &Ctx, args: SbomArgs) -> Result<ExitCode> {
    let SbomArgs { image, output } = args;
    nextgen::cmd_sbom(&ctx.images_dir, &image, output)?;
    Ok(ExitCode::SUCCESS)
}
