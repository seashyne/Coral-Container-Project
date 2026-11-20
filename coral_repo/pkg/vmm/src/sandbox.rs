//! AI Agent / Untrusted Code Sandbox orchestrator (`coral sandbox run`) and
//! Linux MicroVM launcher (QEMU-KVM / Firecracker / Cloud-Hypervisor + native KVM attestation).

use crate::initramfs::{build_initramfs_cpio_gz, inspect_bzimage};
use crate::kvm::probe_kvm;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Detects installed external MicroVM/KVM hypervisor binaries in `$PATH`.
pub fn detect_external_vmm() -> Option<(&'static str, PathBuf)> {
    let path_var = std::env::var_os("PATH")?;
    for name in ["firecracker", "cloud-hypervisor", "qemu-system-x86_64"] {
        for dir in std::env::split_paths(&path_var) {
            let p = dir.join(name);
            if p.is_file() {
                return Some((name, p));
            }
        }
    }
    None
}

/// Configuration for running an untrusted command inside Coral's AI Agent Sandbox.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    pub coral_bin: PathBuf,
    pub images_dir: PathBuf,
    pub state_root: PathBuf,
    pub image: String,
    pub cmd: Vec<String>,
    pub envs: Vec<String>,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub timeout_secs: u64,
    pub allow_network: bool,
    pub kernel_path: Option<PathBuf>,
    pub rootfs_override: Option<PathBuf>,
}

/// Structured execution report returned by `coral sandbox run`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxReport {
    pub isolation_mode: String,
    pub kvm_hardware_available: bool,
    pub exit_code: i32,
    pub timed_out: bool,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
}

/// Boots a real Linux guest kernel + initramfs inside a hardware KVM MicroVM.
/// Prefers Coral's built-in pure-Rust `/dev/kvm` 64-bit Linux Boot Engine (`coral-vmm` native,
/// zero QEMU/Firecracker required) on x86_64 when `/dev/kvm` is accessible, and falls back
/// to `qemu-system-x86_64` when running without KVM or on non-x86 hosts.
pub fn boot_linux_microvm(
    kernel_path: &Path,
    initrd_path: &Path,
    memory_mb: i64,
    timeout_secs: u64,
    extra_cmdline: Option<&str>,
) -> Result<SandboxReport> {
    boot_linux_microvm_with_devices(
        kernel_path,
        initrd_path,
        memory_mb,
        timeout_secs,
        extra_cmdline,
        None,
        false,
        false,
        &[],
    )
}

/// Boots a real Linux kernel + initramfs inside Coral's pure-Rust KVM hypervisor (or QEMU),
/// optionally attaching VirtIO-Blk persistent disk and pure-Rust VirtIO-Net networking.
#[allow(clippy::too_many_arguments)]
pub fn boot_linux_microvm_with_devices(
    kernel_path: &Path,
    initrd_path: &Path,
    memory_mb: i64,
    timeout_secs: u64,
    extra_cmdline: Option<&str>,
    disk_image: Option<&Path>,
    disk_read_only: bool,
    enable_net: bool,
    port_forwards: &[crate::virtio_net::PortForwardRule],
) -> Result<SandboxReport> {
    let start = Instant::now();
    let _hdr = inspect_bzimage(kernel_path)?;
    let kvm = probe_kvm();

    // Primary path (v2.2): Pure-Rust native /dev/kvm x86-64 Linux kernel boot with VirtIO-Blk & Net.
    #[cfg(target_arch = "x86_64")]
    if kvm.available {
        let base_flags = if disk_image.is_some() || enable_net {
            "console=ttyS0 earlyprintk=serial,ttyS0,115200 nokaslr reboot=k panic=1 pci=off quiet rdinit=/coral-init"
        } else {
            "console=ttyS0 earlyprintk=serial,ttyS0,115200 nokaslr reboot=k panic=1 pci=off nomodules quiet rdinit=/coral-init"
        };
        let cmdline = format!("{base_flags} {}", extra_cmdline.unwrap_or(""));
        let res = crate::linux_boot::boot_linux_bzimage_kvm(&crate::linux_boot::LinuxBootOptions {
            kernel_path: kernel_path.to_path_buf(),
            initrd_path: Some(initrd_path.to_path_buf()),
            cmdline: cmdline.trim().to_string(),
            memory_mb: memory_mb.max(128) as usize,
            timeout_ms: timeout_secs.max(1).saturating_mul(1000),
            max_vm_exits: 0,
            disk_image: disk_image.map(Path::to_path_buf),
            disk_read_only,
            enable_net,
            port_forwards: port_forwards.to_vec(),
        })?;
        let mut clean_stdout = String::new();
        for line in res.serial_output.lines() {
            if !line.trim().starts_with("__CORAL_VMM_EXIT__=") {
                clean_stdout.push_str(line);
                clean_stdout.push('\n');
            }
        }
        return Ok(SandboxReport {
            isolation_mode: "coral-native-kvm-linux".into(),
            kvm_hardware_available: true,
            exit_code: res.exit_code,
            timed_out: res.timed_out,
            duration_ms: res.duration_ms,
            stdout: clean_stdout,
            stderr: String::new(),
        });
    }

    let Some((vmm_name, vmm_bin)) = detect_external_vmm() else {
        bail!(
            "Linux kernel '{}' validated, but neither hardware `/dev/kvm` nor an external VMM (`qemu-system-x86_64`) is available",
            kernel_path.display()
        );
    };
    if vmm_name != "qemu-system-x86_64" {
        bail!("external Linux initrd fallback currently supports `qemu-system-x86_64` (found '{vmm_name}')");
    }

    let cmdline = format!(
        "console=ttyS0 rdinit=/coral-init panic=-1 quiet {}",
        extra_cmdline.unwrap_or("")
    );
    let mut cmd = Command::new(vmm_bin);
    if kvm.available {
        cmd.arg("-enable-kvm").arg("-cpu").arg("host");
    }
    cmd.arg("-m")
        .arg(format!("{}M", memory_mb.max(64)))
        .arg("-nographic")
        .arg("-no-reboot")
        .arg("-net")
        .arg("none")
        .arg("-kernel")
        .arg(kernel_path)
        .arg("-initrd")
        .arg(initrd_path)
        .arg("-append")
        .arg(cmdline)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (code, timed_out, stdout, stderr) = run_child_with_timeout(cmd, timeout_secs)?;
    let mut exit_code = code;
    let mut clean_stdout = String::new();
    for line in stdout.lines() {
        if let Some(ec_str) = line.strip_prefix("__CORAL_VMM_EXIT__=") {
            if let Ok(parsed) = ec_str.trim().parse::<i32>() {
                exit_code = parsed;
            }
        } else {
            clean_stdout.push_str(line);
            clean_stdout.push('\n');
        }
    }

    Ok(SandboxReport {
        isolation_mode: if kvm.available {
            "kvm-microvm-linux".into()
        } else {
            "tcg-microvm-linux".into()
        },
        kvm_hardware_available: kvm.available,
        exit_code,
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
        stdout: clean_stdout,
        stderr,
    })
}

/// Executes an untrusted command inside Coral's AI Agent Sandbox:
/// - If a readable Linux guest kernel (`policy.kernel_path`, `CORAL_VMM_KERNEL`, or
///   `~/.local/state/coral/vmm/vmlinuz-virt`) + rootfs + hardware `/dev/kvm` (or QEMU) are available,
///   packs the rootfs into an ephemeral `initramfs.cpio.gz` and boots a full separate Linux Kernel!
/// - Otherwise runs inside Coral's maximum-hardening ephemeral sandbox (Air-gapped `CLONE_NEWNET`,
///   ephemeral OverlayFS CoW wiped on exit, Seccomp-BPF 27-syscall filter, Capabilities Drop,
///   `NO_NEW_PRIVS`, Memory/CPU/PIDs caps, and hard wall-clock timeout killer), reporting KVM hardware status.
pub fn run_ai_sandbox(policy: &SandboxPolicy) -> Result<SandboxReport> {
    let start = Instant::now();
    let kvm = probe_kvm();

    let kernel_opt = policy.kernel_path.clone().or_else(|| {
        std::env::var("CORAL_VMM_KERNEL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
    });

    // Upgrade to full Linux Kernel MicroVM automatically when guest kernel + rootfs + (KVM or QEMU) exist.
    let can_boot_linux = (cfg!(target_arch = "x86_64") && kvm.available)
        || detect_external_vmm().is_some_and(|(n, _)| n == "qemu-system-x86_64");
    if let Some(kernel) = kernel_opt
        && kernel.is_file()
        && let Some(rootfs) = &policy.rootfs_override
        && can_boot_linux
    {
        let tmp_initrd = policy
            .state_root
            .join(format!("vmm-initrd-{}.cpio.gz", std::process::id()));
        build_initramfs_cpio_gz(rootfs, &policy.cmd, &tmp_initrd)?;
        let res = boot_linux_microvm(
            &kernel,
            &tmp_initrd,
            policy.memory_mb.max(256),
            policy.timeout_secs.max(5),
            None,
        );
        let _ = std::fs::remove_file(&tmp_initrd);
        return res;
    }

    let mut cmd = Command::new(&policy.coral_bin);
    cmd.arg("--root")
        .arg(&policy.state_root)
        .arg("--images-dir")
        .arg(&policy.images_dir)
        .arg("run")
        .arg("--image")
        .arg(&policy.image)
        .arg("--network")
        .arg(if policy.allow_network { "bridge" } else { "none" })
        .arg("--memory")
        .arg(policy.memory_mb.max(16).to_string())
        .arg("--cpu")
        .arg(policy.cpu_percent.max(10).to_string())
        .arg("--pids")
        .arg(policy.pids_limit.max(8).to_string());
    for env in &policy.envs {
        cmd.arg("-e").arg(env);
    }
    cmd.arg("--");
    if policy.cmd.is_empty() {
        cmd.arg("/bin/sh");
    } else {
        cmd.args(&policy.cmd);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let (exit_code, timed_out, stdout, stderr) =
        run_child_with_timeout(cmd, policy.timeout_secs.max(1))?;

    let isolation_mode = if kvm.available {
        "hardened-sandbox+kvm-ready"
    } else {
        "hardened-sandbox"
    };

    Ok(SandboxReport {
        isolation_mode: isolation_mode.into(),
        kvm_hardware_available: kvm.available,
        exit_code,
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
        stdout,
        stderr,
    })
}

fn run_child_with_timeout(
    mut cmd: Command,
    timeout_secs: u64,
) -> Result<(i32, bool, String, String)> {
    // Isolate the sandbox child in its own process group so timeout kills the entire process tree
    // (including container PID 1 and any forked grandchildren holding stdout/stderr pipes open).
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            let _ = libc::setpgid(0, 0);
            Ok(())
        });
    }

    let mut child = cmd.spawn().context("spawn sandbox child process")?;
    let pgid = child.id() as i32;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let out_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stdout_pipe {
            use std::io::Read;
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });
    let err_thread = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut r) = stderr_pipe {
            use std::io::Read;
            let _ = r.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + Duration::from_secs(timeout_secs.max(1));
    let mut timed_out = false;
    let exit_code = loop {
        if let Some(status) = child.try_wait()? {
            break status.code().unwrap_or(1);
        }
        if Instant::now() >= deadline {
            timed_out = true;
            unsafe {
                let _ = libc::kill(-pgid, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            break 124;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout_bytes = out_thread.join().unwrap_or_default();
    let stderr_bytes = err_thread.join().unwrap_or_default();
    Ok((
        exit_code,
        timed_out,
        String::from_utf8_lossy(&stdout_bytes).into_owned(),
        String::from_utf8_lossy(&stderr_bytes).into_owned(),
    ))
}
