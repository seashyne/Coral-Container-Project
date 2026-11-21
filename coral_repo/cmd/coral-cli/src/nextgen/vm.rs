//! `coral vm` — native Linux `/dev/kvm` MicroVM hypervisor commands.

use crate::catalog;
use anyhow::{Context, Result, bail};
use coral_daemon::images;
use coral_vmm::{
    InitramfsOptions, KvmRunOptions, PortForwardRule, boot_linux_microvm,
    boot_linux_microvm_with_devices, build_initramfs_with_options, create_disk_image,
    default_kernel_path, default_modules_dir, install_kernel_from_file, probe_kvm,
    run_kvm_hardware_selftest, run_long_mode_payload, setup_alpine_microvm_kernel,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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
    println!(
        "  KVM Ready:    {}",
        if caps.available {
            "YES (hardware VT-x/AMD-V)"
        } else {
            "NO"
        }
    );
    println!("  API Version:  {}", caps.api_version);
    println!("  Max vCPUs:    {}", caps.max_vcpus);
    println!(
        "  External VMM: {}",
        caps.external_vmm
            .as_deref()
            .unwrap_or("none (using built-in pure-Rust KVM engine)")
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
        anyhow::anyhow!(
            "provide a raw 64-bit guest payload path or `--kernel <vmlinuz> --initrd <cpio>`"
        )
    })?;
    let bytes =
        fs::read(&ppath).with_context(|| format!("read guest payload '{}'", ppath.display()))?;
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

pub fn cmd_vm_setup_kernel(
    from: Option<PathBuf>,
    dest: Option<PathBuf>,
    as_json: bool,
) -> Result<()> {
    let target = dest.unwrap_or_else(default_kernel_path);
    let report = if let Some(src) = from {
        install_kernel_from_file(&src, &target)?
    } else {
        if !as_json {
            println!("Downloading official Alpine Linux 3.20 `linux-virt` guest kernel...");
        }
        setup_alpine_microvm_kernel(&target)?
    };
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("MicroVM Linux kernel ready!");
        println!("  Kernel:   {}", report.kernel_path);
        println!("  Size:     {}", catalog::format_bytes(report.size_bytes));
        println!("  Protocol: Linux x86-64 v{}", report.protocol_version);
        println!("  Source:   {}", report.source);
    }
    Ok(())
}

pub fn cmd_vm_disk_create(path: &Path, size_mb: u64, ext4: bool) -> Result<()> {
    create_disk_image(path, size_mb, ext4)?;
    println!(
        "Created VirtIO-Blk disk '{}' ({} MB{})",
        path.display(),
        size_mb,
        if ext4 {
            ", ext4 formatted"
        } else {
            ", raw sparse"
        }
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_vm_boot(
    state_root: &Path,
    images_dir: &Path,
    image: &str,
    kernel: Option<PathBuf>,
    memory_mb: usize,
    timeout: u64,
    cmdline: Option<String>,
    disk: Option<String>,
    disk_mount: Option<String>,
    enable_net: bool,
    ports: Vec<String>,
    cmd: &[String],
    as_json: bool,
) -> Result<ExitCode> {
    let kernel_path = kernel.unwrap_or_else(default_kernel_path);
    if !kernel_path.is_file() {
        bail!(
            "guest Linux kernel '{}' not found — install one automatically with `coral vm setup-kernel`",
            kernel_path.display()
        );
    }
    let rootfs = if Path::new(image).is_dir() {
        fs::canonicalize(image)?
    } else {
        let idx = images::load_index(images_dir);
        let img = images::find_image(&idx, image).ok_or_else(|| {
            anyhow::anyhow!("image '{image}' not found in {}", images_dir.display())
        })?;
        PathBuf::from(&img.rootfs_path)
    };

    let (disk_path, disk_ro) = match disk {
        Some(spec) => {
            if let Some(p) = spec.strip_suffix(":ro") {
                (Some(PathBuf::from(p)), true)
            } else {
                (Some(PathBuf::from(spec)), false)
            }
        }
        None => (None, false),
    };
    let port_rules: Vec<PortForwardRule> = ports
        .iter()
        .map(|s| {
            let (h, g) = s
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("invalid --publish '{s}' (expected HOST:GUEST)"))?;
            Ok(PortForwardRule {
                host_port: h.trim().parse().context("invalid host port")?,
                guest_port: g.trim().parse().context("invalid guest port")?,
            })
        })
        .collect::<Result<_>>()?;

    let mod_dir = default_modules_dir(&kernel_path);
    let initrd_path = state_root.join(format!("coral-vm-initrd-{}.cpio.gz", std::process::id()));
    build_initramfs_with_options(
        &rootfs,
        cmd,
        &initrd_path,
        &InitramfsOptions {
            modules_dir: mod_dir.is_dir().then_some(mod_dir.as_path()),
            enable_blk: disk_path.is_some(),
            enable_net,
            disk_mount: disk_mount.as_deref(),
        },
    )?;
    let report = boot_linux_microvm_with_devices(
        &kernel_path,
        &initrd_path,
        memory_mb as i64,
        timeout,
        cmdline.as_deref(),
        disk_path.as_deref(),
        disk_ro,
        enable_net,
        &port_rules,
    );
    let _ = fs::remove_file(&initrd_path);
    let report = report?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.stdout);
        if !report.stderr.is_empty() {
            eprint!("{}", report.stderr);
        }
        if report.timed_out {
            eprintln!(
                "coral vm boot: timed out after {}s (mode={})",
                timeout, report.isolation_mode
            );
        }
    }
    let code = u8::try_from(report.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}
