//! Orchestration of a native `/dev/kvm` Linux x86-64 MicroVM boot.
//!
//! Sequences the boot phases: KVM VM setup, guest memory layout, vCPU programming,
//! device attachment, the hardware run loop, and telemetry assembly.

use super::{LinuxBootOptions, LinuxBootResult, Uart16550, VirtioMmioConsole};
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

pub(super) fn run_linux_boot(opts: &LinuxBootOptions) -> Result<LinuxBootResult> {
    let start = Instant::now();
    let bzimage = std::fs::read(&opts.kernel_path)
        .with_context(|| format!("read Linux bzImage '{}'", opts.kernel_path.display()))?;
    let bz_info = crate::initramfs::inspect_bzimage(&opts.kernel_path)?;
    let kernel_protocol = format!("{}.{:02}", bz_info.protocol_major, bz_info.protocol_minor);

    let setup_size = (bz_info.setup_sects as usize + 1) * 512;
    if setup_size >= bzimage.len() {
        bail!("invalid bzImage: setup_size ({setup_size}) exceeds file size");
    }
    let kernel_payload = &bzimage[setup_size..];

    let initrd_bytes = match &opts.initrd_path {
        Some(p) => Some(
            std::fs::read(p).with_context(|| format!("read initramfs '{}'", p.display()))?,
        ),
        None => None,
    };

    let mem_mb = opts.memory_mb.clamp(64, 1024);
    let mem_size = mem_mb * 1024 * 1024;

    // 1. Open /dev/kvm and create VM with IRQChip + PIT2.
    let kvm_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .context("open /dev/kvm for pure-Rust Linux MicroVM boot")?;
    let kvm_fd = kvm_file.as_raw_fd();
    let api = unsafe { libc::ioctl(kvm_fd, crate::kvm::KVM_GET_API_VERSION, 0) };
    if api != 12 {
        bail!("unsupported KVM API version {api} (expected 12)");
    }

    let vm_raw = unsafe { libc::ioctl(kvm_fd, crate::kvm::KVM_CREATE_VM, 0) };
    if vm_raw < 0 {
        bail!("KVM_CREATE_VM failed: {}", io::Error::last_os_error());
    }
    let vm_fd = unsafe { OwnedFd::from_raw_fd(vm_raw) };

    // Configure TSS & Identity map addresses required by Intel VMX IRQChip.
    let tss_addr: libc::c_ulong = 0xfffb_d000;
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), crate::kvm::KVM_SET_TSS_ADDR, tss_addr) } < 0 {
        bail!("KVM_SET_TSS_ADDR failed: {}", io::Error::last_os_error());
    }
    let mut ident_addr: u64 = 0xfffb_c000;
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), crate::kvm::KVM_SET_IDENTITY_MAP_ADDR, &mut ident_addr) } < 0 {
        bail!("KVM_SET_IDENTITY_MAP_ADDR failed: {}", io::Error::last_os_error());
    }
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), crate::kvm::KVM_CREATE_IRQCHIP, 0) } < 0 {
        bail!("KVM_CREATE_IRQCHIP failed: {}", io::Error::last_os_error());
    }
    #[repr(C)]
    struct KvmPitConfig {
        flags: u32,
        pad: [u32; 15],
    }
    let pit_cfg = KvmPitConfig { flags: 0, pad: [0; 15] };
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), crate::kvm::KVM_CREATE_PIT2, &pit_cfg) } < 0 {
        bail!("KVM_CREATE_PIT2 failed: {}", io::Error::last_os_error());
    }

    // 2. Allocate guest physical RAM and map slot 0 into EPT/NPT.
    let guest_mem = crate::kvm::MmapGuard::anon_rw(mem_size)?;
    let mem = unsafe { std::slice::from_raw_parts_mut(guest_mem.ptr, guest_mem.len) };

    // 3. Populate guest RAM (GDT, page tables, kernel, cmdline, boot_params, E820).
    let load_addr = super::setup::build_guest_memory(
        mem,
        &bzimage,
        kernel_payload,
        initrd_bytes.as_deref(),
        opts,
        mem_size,
        mem_mb,
    )?;

    let region = crate::kvm::KvmUserspaceMemoryRegion {
        slot: 0,
        flags: 0,
        guest_phys_addr: 0,
        memory_size: mem_size as u64,
        userspace_addr: guest_mem.ptr as u64,
    };
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), crate::kvm::KVM_SET_USER_MEMORY_REGION, &region) } < 0 {
        bail!(
            "KVM_SET_USER_MEMORY_REGION failed: {}",
            io::Error::last_os_error()
        );
    }

    // 4. Create vCPU 0, initialize CPUID2, mmap kvm_run, and enter 64-bit startup_64.
    let vcpu = super::regs::create_vcpu(kvm_fd, vm_fd.as_raw_fd(), load_addr)?;

    let mut virtio_blk = opts
        .disk_image
        .as_ref()
        .map(|p| crate::virtio_blk::VirtioBlkDevice::open(p, opts.disk_read_only))
        .transpose()?;
    let mut virtio_net = if opts.enable_net {
        Some(crate::virtio_net::VirtioNetDevice::new(&opts.port_forwards)?)
    } else {
        None
    };

    // 5. Run the guest under a watchdog-bounded wall-clock deadline.
    let deadline = start + Duration::from_millis(opts.timeout_ms.max(250));
    let mut uart = Uart16550::default();
    let mut virtio_console = VirtioMmioConsole::default();
    let outcome = super::run_loop::run_guest(
        opts,
        deadline,
        vm_fd.as_raw_fd(),
        vcpu.vcpu_fd.as_raw_fd(),
        &vcpu.run_map,
        mem,
        &mut uart,
        &mut virtio_console,
        &mut virtio_blk,
        &mut virtio_net,
    )?;

    let mut exit_code = outcome.exit_code;
    let mut timed_out = outcome.timed_out;
    let serial_output = String::from_utf8_lossy(&uart.output).into_owned();
    if let Some(marker_code) = super::parse_coral_vmm_exit_marker(&serial_output) {
        exit_code = marker_code;
        timed_out = false;
    }

    let virtio_blk_ops = virtio_blk
        .as_ref()
        .map(|b| b.reads + b.writes + b.flushes)
        .unwrap_or(0);
    let virtio_net_packets = virtio_net
        .as_ref()
        .map(|n| n.tx_packets + n.rx_packets)
        .unwrap_or(0);

    Ok(LinuxBootResult {
        exit_code,
        serial_output,
        exit_reason: outcome.exit_reason,
        vm_exits: outcome.vm_exits,
        duration_ms: start.elapsed().as_millis() as u64,
        memory_mb: mem_mb,
        kernel_protocol,
        timed_out,
        virtio_mmio_probed: virtio_console.probed,
        virtio_blk_ops,
        virtio_net_packets,
    })
}
