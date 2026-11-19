//! CLI handlers for Coral v2.0 Next-Gen workloads: `coral wasm`, `coral vm`, and `coral sandbox`.

use crate::catalog;
use anyhow::{Context, Result, bail};
use coral_daemon::images;
use coral_vmm::{
    KvmRunOptions, SandboxPolicy, boot_linux_microvm, probe_kvm, run_ai_sandbox,
    run_kvm_hardware_selftest, run_long_mode_payload,
};
use coral_wasm::{WasmRunOptions, find_wasm_module_in_rootfs, run_wasm_file};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn cmd_wasm_run(
    images_dir: &Path,
    target: &str,
    fuel: u64,
    memory_mb: usize,
    envs: Vec<String>,
    args: Vec<String>,
    as_json: bool,
) -> Result<ExitCode> {
    let module_path = resolve_wasm_target(images_dir, target)?;
    let mut argv = vec![
        module_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "module.wasm".into()),
    ];
    argv.extend(args);

    let out = run_wasm_file(
        &module_path,
        &WasmRunOptions {
            args: argv,
            envs,
            stdin: Vec::new(),
            fuel_limit: fuel,
            memory_limit_mb: memory_mb,
            passthrough_stdio: !as_json,
        },
    )?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if let Some(trap) = &out.trapped {
        eprintln!("coral wasm trap: {trap}");
    }

    let code = u8::try_from(out.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

pub fn cmd_wasm_pack(images_dir: &Path, file: &Path, tag: &str) -> Result<()> {
    if !file.is_file() {
        bail!("WebAssembly source file '{}' not found", file.display());
    }
    let bytes = fs::read(file).with_context(|| format!("read '{}'", file.display()))?;
    // Validate Wasm/WAT syntax immediately before packing!
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wasm")
        .to_ascii_lowercase();
    let dest_name = if ext == "wat" {
        "module.wat"
    } else {
        "module.wasm"
    };

    let slug: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let rootfs = images_dir.join(format!("wasm-{slug}")).join("rootfs");
    fs::create_dir_all(&rootfs)?;
    fs::write(rootfs.join(dest_name), &bytes)?;

    let abs_rootfs = fs::canonicalize(&rootfs).unwrap_or(rootfs);
    let size = images::dir_size(&abs_rootfs);
    let id = format!(
        "sha256:wasm-{}-{}",
        slug,
        chrono::Utc::now().timestamp_millis()
    );
    let stored = images::StoredImage {
        id: id.clone(),
        repo_tags: vec![tag.to_string()],
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    };

    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, stored);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    images::save_index(images_dir, &index)?;
    println!(
        "Packed Wasm OCI image '{}'\n  ID:     {}\n  Size:   {}\n  Module: {}",
        tag,
        id,
        catalog::format_bytes(size),
        abs_rootfs.join(dest_name).display()
    );
    Ok(())
}

pub fn cmd_vm_probe(as_json: bool) -> Result<()> {
    let caps = probe_kvm();
    let selftest = if caps.available {
        Some(run_kvm_hardware_selftest()?)
    } else {
        None
    };

    if as_json {
        let payload = serde_json::json!({
            "capabilities": caps,
            "selftest": selftest,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Coral VMM — Native Linux /dev/kvm Probe");
    println!("  Architecture: {}", caps.arch);
    println!("  KVM Ready:    {}", if caps.available { "YES (hardware VT-x/AMD-V)" } else { "NO" });
    println!("  API Version:  {}", caps.api_version);
    println!("  Max vCPUs:    {}", caps.max_vcpus);
    println!(
        "  External VMM: {}",
        caps.external_vmm.as_deref().unwrap_or("none (using built-in pure-Rust KVM engine)")
    );
    println!("  Detail:       {}", caps.detail);
    if let Some(st) = selftest {
        println!(
            "  64-bit KVM Self-Test: PASS (exit={}, vm_exits={}, boot+exec={}us, output={:?})",
            st.exit_code,
            st.vm_exits,
            st.duration_usec,
            st.serial_output.trim()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_vm_run(
    payload: Option<PathBuf>,
    kernel: Option<PathBuf>,
    initrd: Option<PathBuf>,
    cmdline: Option<String>,
    memory_mb: usize,
    timeout: u64,
    max_exits: u64,
    as_json: bool,
) -> Result<ExitCode> {
    if let Some(kpath) = kernel {
        let ipath = initrd.ok_or_else(|| {
            anyhow::anyhow!("--initrd <PATH> is required when booting a Linux kernel with --kernel")
        })?;
        let report = boot_linux_microvm(
            &kpath,
            &ipath,
            memory_mb as i64,
            timeout,
            cmdline.as_deref(),
        )?;
        if as_json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", report.stdout);
            if !report.stderr.is_empty() {
                eprint!("{}", report.stderr);
            }
        }
        let code = u8::try_from(report.exit_code.clamp(0, 255)).unwrap_or(1);
        return Ok(ExitCode::from(code));
    }

    let ppath = payload.ok_or_else(|| {
        anyhow::anyhow!("provide a raw 64-bit guest payload path or `--kernel <vmlinuz> --initrd <cpio>`")
    })?;
    let bytes = fs::read(&ppath)
        .with_context(|| format!("read guest payload '{}'", ppath.display()))?;
    let res = run_long_mode_payload(
        &bytes,
        &KvmRunOptions {
            memory_mb,
            timeout_ms: timeout.saturating_mul(1_000),
            max_vm_exits: max_exits,
        },
    )?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&res)?);
    } else {
        print!("{}", res.serial_output);
        if res.timed_out {
            eprintln!("coral vm terminated: {}", res.exit_reason);
        }
    }
    let code = u8::try_from(res.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

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

pub fn resolve_wasm_target(images_dir: &Path, target: &str) -> Result<PathBuf> {
    let p = Path::new(target);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if p.is_dir()
        && let Some(m) = find_wasm_module_in_rootfs(p)
    {
        return Ok(m);
    }
    let index = images::load_index(images_dir);
    if let Some(img) = images::find_image(&index, target)
        && let Some(m) = find_wasm_module_in_rootfs(Path::new(&img.rootfs_path))
    {
        return Ok(m);
    }
    bail!(
        "WebAssembly module or Wasm OCI image '{target}' not found (pack one with `coral wasm pack <file.wasm> -t <tag>`)"
    )
}
