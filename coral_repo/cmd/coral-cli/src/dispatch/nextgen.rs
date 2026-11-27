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

pub(crate) fn scan(ctx: &Ctx, args: crate::args::ScanArgs) -> Result<ExitCode> {
    let report = super::run_async(coral_daemon::images::scan_image_vulnerabilities(
        &ctx.images_dir,
        &args.image,
        args.offline,
        None,
    ))?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "Scan: {} ({}) — {} package(s) scanned via {}",
            report.image, report.os_ecosystem, report.packages_scanned, report.source
        );
        println!(
            "Summary: {} total (CRITICAL: {}, HIGH: {}, MEDIUM: {}, LOW: {}, UNKNOWN: {})",
            report.summary.total,
            report.summary.critical,
            report.summary.high,
            report.summary.medium,
            report.summary.low,
            report.summary.unknown
        );
        if !report.findings.is_empty() {
            println!("{:<22} {:<10} {:<20} {:<18} SUMMARY", "CVE / ID", "SEVERITY", "PACKAGE", "VERSION");
            for f in &report.findings {
                println!(
                    "{:<22} {:<10} {:<20} {:<18} {}",
                    f.id, f.severity, f.package, f.version, f.summary
                );
            }
        }
    }
    if let Some(ref threshold) = args.fail_on
        && report.exceeds_threshold(threshold)
    {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn security(ctx: &Ctx, args: crate::args::SecurityArgs) -> Result<ExitCode> {
    match args.action {
        crate::args::SecurityAction::Audit { json } => {
            let report: coral_daemon::service::nextgen::SecurityAuditReport = run_async(async {
                if let Some(ch) = crate::daemon_client::try_connect_channel(&ctx.addr).await {
                    let mut client =
                        coral_api::runtime_service_client::RuntimeServiceClient::new(ch);
                    let mut annotations = std::collections::HashMap::new();
                    annotations.insert("coral.system.action".into(), "security_audit".into());
                    let mut req = tonic::Request::new(coral_api::RunPodSandboxRequest {
                        config: Some(coral_api::PodSandboxConfig {
                            metadata: Some(coral_api::PodSandboxMetadata {
                                name: "coral-security-audit".into(),
                                uid: "coral-security-audit".into(),
                                namespace: "coral-system".into(),
                                attempt: 0,
                            }),
                            hostname: "coral-system".into(),
                            log_directory: String::new(),
                            labels: std::collections::HashMap::new(),
                            annotations,
                        }),
                        runtime_handler: "coral".into(),
                    });
                    if let Ok(tok) = std::env::var("CORAL_AUTH_TOKEN")
                        && !tok.trim().is_empty()
                        && let Ok(val) = format!("Bearer {}", tok.trim()).parse()
                    {
                        req.metadata_mut().insert("authorization", val);
                    }
                    if let Ok(resp) = client.run_pod_sandbox(req).await {
                        let raw = resp.into_inner().pod_sandbox_id;
                        if let Ok(parsed) = serde_json::from_str(&raw) {
                            return Ok(parsed);
                        }
                    }
                }
                Ok(coral_daemon::service::nextgen::run_host_security_audit(
                    &ctx.state_root.join("daemon"),
                ))
            })?;

            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Coral CIS Security Audit — Score: {}/100 ({} pass, {} warn, {} fail)",
                    report.score, report.passed, report.warned, report.failed
                );
                println!(
                    "Host: rootless={}, landlock_abi={}",
                    report.rootless,
                    report
                        .landlock_abi
                        .map(|v| format!("v{v}"))
                        .unwrap_or_else(|| "unavailable".into())
                );
                println!("{:<10} {:<6} {:<42} DETAIL", "CHECK", "STATE", "TITLE");
                for c in &report.checks {
                    println!(
                        "{:<10} {:<6} {:<42} {}",
                        c.id,
                        c.status.to_uppercase(),
                        c.title,
                        c.detail
                    );
                    if let Some(rem) = &c.remediation {
                        println!("           fix -> {rem}");
                    }
                }
            }
            Ok(if report.failed > 0 {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            })
        }
    }
}
