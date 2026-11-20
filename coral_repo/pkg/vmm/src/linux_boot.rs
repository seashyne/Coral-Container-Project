//! Pure-Rust Linux x86-64 64-bit Kernel Boot Engine over native `/dev/kvm` (Zero QEMU / Zero Firecracker).
//!
//! Implements the official Linux x86-64 64-bit Boot Protocol (v2.06+):
//! - In-kernel KVM IRQChip (LAPIC + IOAPIC + PIC) + PIT2 timer
//! - 4-level identity page tables covering 0..4 GiB (2 MiB huge pages) + flat 64-bit GDT
//! - `boot_params` (`zero_page`) with `setup_header`, E820 RAM map, kernel cmdline, and initramfs
//! - Full 16550A COM1 UART (`0x3F8..0x3FF`) with loopback autoconfig & KVM IRQ 4 pulsing
//! - VirtIO-MMIO v2 transport device plane (`0xD000_0000..0xD000_1000`)
//! - Watchdog signal interruption for deterministic wall-clock timeout even during guest `HLT`

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for booting a real Linux kernel (`bzImage`) inside Coral's native `/dev/kvm` hypervisor.
#[derive(Debug, Clone)]
pub struct LinuxBootOptions {
    /// Path to a Linux x86-64 `bzImage` kernel (e.g., Alpine `vmlinuz-virt`).
    pub kernel_path: PathBuf,
    /// Optional path to a gzipped Linux `newc` CPIO initramfs archive.
    pub initrd_path: Option<PathBuf>,
    /// Linux kernel command line (defaults to serial console + fast keyboard-controller reboot).
    pub cmdline: String,
    /// Guest RAM in MiB (clamped to `64..=1024`, default `256`).
    pub memory_mb: usize,
    /// Hard wall-clock deadline in milliseconds (default `15_000`).
    pub timeout_ms: u64,
    /// Optional VM-exit budget (0 = unlimited, bounded by `timeout_ms`).
    pub max_vm_exits: u64,
}

impl Default for LinuxBootOptions {
    fn default() -> Self {
        Self {
            kernel_path: crate::kernel::default_kernel_path(),
            initrd_path: None,
            cmdline: "console=ttyS0 earlyprintk=serial,ttyS0,115200 nokaslr reboot=k panic=1 pci=off nomodules rdinit=/coral-init".into(),
            memory_mb: 256,
            timeout_ms: 15_000,
            max_vm_exits: 0,
        }
    }
}

/// Telemetry and captured serial output from a native Linux kernel MicroVM boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinuxBootResult {
    pub exit_code: i32,
    pub serial_output: String,
    pub exit_reason: String,
    pub vm_exits: u64,
    pub duration_ms: u64,
    pub memory_mb: usize,
    pub kernel_protocol: String,
    pub timed_out: bool,
    pub virtio_mmio_probed: bool,
}

/// Pure-Rust VirtIO-MMIO v2 transport state (`0xD000_0000..0xD000_1000`, VirtIO Console device ID 3).
#[derive(Debug, Default)]
pub struct VirtioMmioConsole {
    pub status: u32,
    pub driver_features_sel: u32,
    pub device_features_sel: u32,
    pub queue_sel: u32,
    pub probed: bool,
}

impl VirtioMmioConsole {
    pub const BASE_ADDR: u64 = 0xD000_0000;
    pub const SIZE: u64 = 0x1000;
    pub const MAGIC_VALUE: u32 = 0x7472_6976; // "virt"
    pub const VERSION: u32 = 2; // Modern VirtIO-MMIO v2
    pub const DEVICE_ID_CONSOLE: u32 = 3;
    pub const VENDOR_ID_CORAL: u32 = 0x4352_4C21; // "CRL!"

    pub fn read_u32(&mut self, offset: u64) -> u32 {
        self.probed = true;
        match offset {
            0x000 => Self::MAGIC_VALUE,
            0x004 => Self::VERSION,
            0x008 => Self::DEVICE_ID_CONSOLE,
            0x00C => Self::VENDOR_ID_CORAL,
            0x010 => {
                // DeviceFeatures: advertise VIRTIO_F_VERSION_1 (bit 32) on word 1
                if self.device_features_sel == 1 { 1 } else { 0 }
            }
            0x034 => 64, // QueueNumMax
            0x070 => self.status,
            0x0FC => 1, // ConfigGeneration
            _ => 0,
        }
    }

    pub fn write_u32(&mut self, offset: u64, val: u32) {
        self.probed = true;
        match offset {
            0x014 => self.device_features_sel = val,
            0x024 => self.driver_features_sel = val,
            0x030 => self.queue_sel = val,
            0x070 => {
                if val == 0 {
                    self.status = 0;
                } else {
                    self.status = val;
                }
            }
            _ => {}
        }
    }
}

/// Emulated 16550A COM1 UART (`0x3F8..0x3FF`) with loopback autoconfig support and IRQ 4 state.
#[derive(Debug, Default)]
pub struct Uart16550 {
    pub ier: u8,
    pub lcr: u8,
    pub mcr: u8,
    pub scr: u8,
    pub dll: u8,
    pub dlm: u8,
    pub tx_irq_pending: bool,
    pub rx_buf: std::collections::VecDeque<u8>,
    pub output: Vec<u8>,
}

impl Uart16550 {
    pub fn read_port(&mut self, offset: u16) -> u8 {
        let dlab = (self.lcr & 0x80) != 0;
        match offset {
            0 => {
                if dlab {
                    self.dll
                } else {
                    self.rx_buf.pop_front().unwrap_or(0)
                }
            }
            1 => {
                if dlab {
                    self.dlm
                } else {
                    self.ier
                }
            }
            2 => {
                // IIR: FIFO enabled bits (0xC0) + highest priority pending interrupt
                if !self.rx_buf.is_empty() && (self.ier & 0x01) != 0 {
                    0xC4 // Received Data Available
                } else if self.tx_irq_pending && (self.ier & 0x02) != 0 {
                    self.tx_irq_pending = false;
                    0xC2 // Transmitter Holding Register Empty
                } else {
                    0xC1 // No interrupt pending
                }
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => {
                // LSR: THRE (0x20) | TEMT (0x40) | optional Data Ready (0x01)
                0x60 | if self.rx_buf.is_empty() { 0 } else { 1 }
            }
            6 => {
                // MSR: loopback mirrors MCR bits, normal reports DCD | DSR | CTS
                if (self.mcr & 0x10) != 0 {
                    let mut msr = 0u8;
                    if (self.mcr & 0x02) != 0 { msr |= 0x10; } // RTS -> CTS
                    if (self.mcr & 0x01) != 0 { msr |= 0x20; } // DTR -> DSR
                    if (self.mcr & 0x04) != 0 { msr |= 0x40; } // OUT1 -> RI
                    if (self.mcr & 0x08) != 0 { msr |= 0x80; } // OUT2 -> DCD
                    msr
                } else {
                    0xB0
                }
            }
            7 => self.scr,
            _ => 0,
        }
    }

    /// Returns `true` if COM1 IRQ 4 should be pulsed to the in-kernel KVM IRQChip.
    pub fn write_port(&mut self, offset: u16, val: u8) -> bool {
        let dlab = (self.lcr & 0x80) != 0;
        match offset {
            0 => {
                if dlab {
                    self.dll = val;
                    false
                } else if (self.mcr & 0x10) != 0 {
                    // Loopback mode (used by Linux 8250 autoconfig probe)
                    if self.rx_buf.len() < 64 {
                        self.rx_buf.push_back(val);
                    }
                    (self.ier & 0x01) != 0
                } else {
                    if self.output.len() < 4 * 1024 * 1024 {
                        self.output.push(val);
                    }
                    if (self.ier & 0x02) != 0 {
                        self.tx_irq_pending = true;
                        true
                    } else {
                        false
                    }
                }
            }
            1 => {
                if dlab {
                    self.dlm = val;
                    false
                } else {
                    let prev_tx = (self.ier & 0x02) != 0;
                    self.ier = val & 0x0F;
                    if !prev_tx && (self.ier & 0x02) != 0 {
                        self.tx_irq_pending = true;
                        true
                    } else {
                        false
                    }
                }
            }
            2 => {
                // FCR: bit 1 clears RX FIFO
                if (val & 0x02) != 0 {
                    self.rx_buf.clear();
                }
                false
            }
            3 => {
                self.lcr = val;
                false
            }
            4 => {
                self.mcr = val;
                false
            }
            7 => {
                self.scr = val;
                false
            }
            _ => false,
        }
    }
}

/// Boots a real Linux x86-64 `bzImage` kernel (plus optional CPIO initramfs) directly inside
/// Coral's pure-Rust `/dev/kvm` hypervisor without QEMU or Firecracker.
#[cfg(target_arch = "x86_64")]
pub fn boot_linux_bzimage_kvm(opts: &LinuxBootOptions) -> Result<LinuxBootResult> {
    use crate::kvm::{
        COM1_DATA, DEBUG_EXIT_PORT, KVM_CREATE_IRQCHIP, KVM_CREATE_PIT2, KVM_CREATE_VCPU,
        KVM_CREATE_VM, KVM_EXIT_HLT, KVM_EXIT_INTERNAL_ERROR, KVM_EXIT_IO, KVM_EXIT_IO_IN,
        KVM_EXIT_IO_OUT, KVM_EXIT_MMIO, KVM_EXIT_SHUTDOWN, KVM_EXIT_SYSTEM_EVENT,
        KVM_GET_API_VERSION, KVM_GET_SREGS, KVM_GET_VCPU_MMAP_SIZE, KVM_RUN,
        KVM_SET_IDENTITY_MAP_ADDR, KVM_SET_REGS, KVM_SET_SREGS, KVM_SET_TSS_ADDR,
        KVM_SET_USER_MEMORY_REGION, KvmDtable, KvmRegs, KvmSegment, KvmSregs,
        KvmUserspaceMemoryRegion, MmapGuard, setup_vcpu_cpuid, write_u64_le,
    };
    use std::fs::{self, OpenOptions};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    const GDT_ADDR: u64 = 0x0500;
    const ZERO_PAGE_ADDR: u64 = 0x7000;
    const PML4_ADDR: u64 = 0x9000;
    const PDPT_ADDR: u64 = 0xA000;
    const PD_BASE_ADDR: u64 = 0xB000; // 4 pages: 0xB000..0xEFFF -> covers 0..4 GiB
    const CMDLINE_ADDR: u64 = 0x2_0000;

    let start = Instant::now();
    let bzimage = fs::read(&opts.kernel_path)
        .with_context(|| format!("read Linux bzImage '{}'", opts.kernel_path.display()))?;
    let bz_info = crate::initramfs::inspect_bzimage(&opts.kernel_path)?;
    let kernel_protocol = format!("{}.{:02}", bz_info.protocol_major, bz_info.protocol_minor);

    let setup_size = (bz_info.setup_sects as usize + 1) * 512;
    if setup_size >= bzimage.len() {
        bail!("invalid bzImage: setup_size ({setup_size}) exceeds file size");
    }
    let kernel_payload = &bzimage[setup_size..];

    let initrd_bytes = match &opts.initrd_path {
        Some(p) => Some(fs::read(p).with_context(|| format!("read initramfs '{}'", p.display()))?),
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
    let api = unsafe { libc::ioctl(kvm_fd, KVM_GET_API_VERSION, 0) };
    if api != 12 {
        bail!("unsupported KVM API version {api} (expected 12)");
    }

    let vm_raw = unsafe { libc::ioctl(kvm_fd, KVM_CREATE_VM, 0) };
    if vm_raw < 0 {
        bail!("KVM_CREATE_VM failed: {}", io::Error::last_os_error());
    }
    let vm_fd = unsafe { OwnedFd::from_raw_fd(vm_raw) };

    // Configure TSS & Identity map addresses required by Intel VMX IRQChip.
    let tss_addr: libc::c_ulong = 0xfffb_d000;
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_SET_TSS_ADDR, tss_addr) } < 0 {
        bail!("KVM_SET_TSS_ADDR failed: {}", io::Error::last_os_error());
    }
    let mut ident_addr: u64 = 0xfffb_c000;
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_SET_IDENTITY_MAP_ADDR, &mut ident_addr) } < 0 {
        bail!("KVM_SET_IDENTITY_MAP_ADDR failed: {}", io::Error::last_os_error());
    }
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_CREATE_IRQCHIP, 0) } < 0 {
        bail!("KVM_CREATE_IRQCHIP failed: {}", io::Error::last_os_error());
    }
    #[repr(C)]
    struct KvmPitConfig {
        flags: u32,
        pad: [u32; 15],
    }
    let pit_cfg = KvmPitConfig { flags: 0, pad: [0; 15] };
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_CREATE_PIT2, &pit_cfg) } < 0 {
        bail!("KVM_CREATE_PIT2 failed: {}", io::Error::last_os_error());
    }

    // 2. Allocate guest physical RAM and map slot 0 into EPT/NPT.
    let guest_mem = MmapGuard::anon_rw(mem_size)?;
    let mem = unsafe { std::slice::from_raw_parts_mut(guest_mem.ptr, guest_mem.len) };

    // 3. Populate flat 64-bit GDT at 0x0500 (0x10 = __BOOT_CS, 0x18 = __BOOT_DS).
    write_u64_le(mem, GDT_ADDR as usize, 0);
    write_u64_le(mem, GDT_ADDR as usize + 8, 0);
    write_u64_le(mem, GDT_ADDR as usize + 16, 0x00AF_9A00_0000_FFFF); // 0x10: 64-bit code
    write_u64_le(mem, GDT_ADDR as usize + 24, 0x00CF_9200_0000_FFFF); // 0x18: 64-bit data

    // 4. Populate 4-level identity page tables covering 0..4 GiB via 2 MiB huge pages.
    write_u64_le(mem, PML4_ADDR as usize, PDPT_ADDR | 0x3);
    for gb in 0..4usize {
        let pd_addr = PD_BASE_ADDR + (gb as u64) * 0x1000;
        write_u64_le(mem, PDPT_ADDR as usize + gb * 8, pd_addr | 0x3);
        for entry in 0..512usize {
            let phys = ((gb as u64) << 30) + ((entry as u64) << 21);
            write_u64_le(mem, pd_addr as usize + entry * 8, phys | 0x83);
        }
    }

    // 5. Load Linux bzImage 64-bit kernel payload (at pref_address if relocatable & fits, else 1 MiB).
    let relocatable = bzimage.get(0x234).copied().unwrap_or(0) != 0;
    let pref_address = if bzimage.len() >= 0x260 {
        u64::from_le_bytes(bzimage[0x258..0x260].try_into().unwrap())
    } else {
        0x10_0000
    };
    let load_addr: usize = if relocatable
        && pref_address >= 0x10_0000
        && (pref_address as usize) + kernel_payload.len() + 0x200_0000 < mem_size
    {
        pref_address as usize
    } else {
        0x10_0000
    };
    if load_addr + kernel_payload.len() >= mem_size {
        bail!(
            "Linux kernel payload ({} bytes) does not fit in {} MiB guest RAM",
            kernel_payload.len(),
            mem_mb
        );
    }
    mem[load_addr..load_addr + kernel_payload.len()].copy_from_slice(kernel_payload);

    // 6. Copy kernel command line to 0x20000.
    let cmdline_bytes = opts.cmdline.as_bytes();
    let max_cmdline = 4095usize;
    let copy_len = cmdline_bytes.len().min(max_cmdline);
    mem[CMDLINE_ADDR as usize..CMDLINE_ADDR as usize + copy_len]
        .copy_from_slice(&cmdline_bytes[..copy_len]);
    mem[CMDLINE_ADDR as usize + copy_len] = 0;

    // 7. Populate Linux boot_params (zero_page) at 0x7000.
    let zp = ZERO_PAGE_ADDR as usize;
    let header_end = (0x202 + bzimage[0x201] as usize).min(0x290).min(bzimage.len());
    mem[zp + 0x1F1..zp + header_end].copy_from_slice(&bzimage[0x1F1..header_end]);
    mem[zp + 0x210] = 0xFF; // type_of_loader = undefined
    mem[zp + 0x211] |= 0x80 | 0x40 | 0x01; // CAN_USE_HEAP | KEEP_SEGMENTS | LOADED_HIGH
    mem[zp + 0x224..zp + 0x226].copy_from_slice(&0xFE00u16.to_le_bytes()); // heap_end_ptr
    mem[zp + 0x214..zp + 0x218].copy_from_slice(&(load_addr as u32).to_le_bytes()); // code32_start
    mem[zp + 0x228..zp + 0x22C].copy_from_slice(&(CMDLINE_ADDR as u32).to_le_bytes());
    mem[zp + 0x238..zp + 0x23C].copy_from_slice(&(copy_len as u32).to_le_bytes());

    // Optional initramfs placed near top of guest RAM (respecting initrd_addr_max).
    if let Some(initrd) = &initrd_bytes {
        let initrd_max = if bzimage.len() >= 0x230 {
            u32::from_le_bytes(bzimage[0x22C..0x230].try_into().unwrap()) as usize
        } else {
            0x37FF_FFFF
        };
        let top = mem_size.min(initrd_max).saturating_sub(0x1_0000);
        if initrd.len() + load_addr + kernel_payload.len() + 0x10_0000 >= top {
            bail!(
                "initramfs ({} bytes) + kernel exceeds {} MiB guest RAM",
                initrd.len(),
                mem_mb
            );
        }
        let initrd_addr = (top - initrd.len()) & !0xFFF;
        mem[initrd_addr..initrd_addr + initrd.len()].copy_from_slice(initrd);
        mem[zp + 0x218..zp + 0x21C].copy_from_slice(&(initrd_addr as u32).to_le_bytes());
        mem[zp + 0x21C..zp + 0x220].copy_from_slice(&(initrd.len() as u32).to_le_bytes());
    }

    // Write E820 memory map (3 entries: low RAM, BIOS/EBDA reserved, high RAM).
    mem[zp + 0x1E8] = 3;
    write_e820_entry(mem, zp + 0x2D0, 0, 0x9_FC00, 1);
    write_e820_entry(mem, zp + 0x2D0 + 20, 0x9_FC00, 0x6_0400, 2);
    write_e820_entry(mem, zp + 0x2D0 + 40, 0x10_0000, (mem_size - 0x10_0000) as u64, 1);

    let region = KvmUserspaceMemoryRegion {
        slot: 0,
        flags: 0,
        guest_phys_addr: 0,
        memory_size: mem_size as u64,
        userspace_addr: guest_mem.ptr as u64,
    };
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_SET_USER_MEMORY_REGION, &region) } < 0 {
        bail!("KVM_SET_USER_MEMORY_REGION failed: {}", io::Error::last_os_error());
    }

    // 8. Create vCPU 0, initialize CPUID2, mmap kvm_run, and enter 64-bit startup_64.
    let vcpu_raw = unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_CREATE_VCPU, 0) };
    if vcpu_raw < 0 {
        bail!("KVM_CREATE_VCPU failed: {}", io::Error::last_os_error());
    }
    let vcpu_fd = unsafe { OwnedFd::from_raw_fd(vcpu_raw) };
    setup_vcpu_cpuid(kvm_fd, vcpu_fd.as_raw_fd())?;

    let mmap_sz = unsafe { libc::ioctl(kvm_fd, KVM_GET_VCPU_MMAP_SIZE, 0) };
    if mmap_sz <= 0 {
        bail!("KVM_GET_VCPU_MMAP_SIZE failed: {}", io::Error::last_os_error());
    }
    let run_map = MmapGuard::fd_rw(vcpu_fd.as_raw_fd(), mmap_sz as usize)?;

    let mut sregs = KvmSregs::default();
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_GET_SREGS, &mut sregs) } < 0 {
        bail!("KVM_GET_SREGS failed: {}", io::Error::last_os_error());
    }
    sregs.cr3 = PML4_ADDR;
    sregs.cr4 = 1 << 5; // PAE
    sregs.cr0 = (1 << 0) | (1 << 1) | (1 << 4) | (1 << 5) | (1 << 16) | (1 << 31);
    sregs.efer = (1 << 8) | (1 << 10); // LME | LMA
    sregs.gdt = KvmDtable { base: GDT_ADDR, limit: 31, padding: [0; 3] };

    let code_seg = KvmSegment {
        base: 0,
        limit: 0xFFFF_FFFF,
        selector: 0x10,
        type_: 11,
        present: 1,
        dpl: 0,
        db: 0,
        s: 1,
        l: 1,
        g: 1,
        avl: 0,
        unusable: 0,
        padding: 0,
    };
    let data_seg = KvmSegment {
        base: 0,
        limit: 0xFFFF_FFFF,
        selector: 0x18,
        type_: 3,
        present: 1,
        dpl: 0,
        db: 1,
        s: 1,
        l: 0,
        g: 1,
        avl: 0,
        unusable: 0,
        padding: 0,
    };
    sregs.cs = code_seg;
    sregs.ds = data_seg;
    sregs.es = data_seg;
    sregs.fs = data_seg;
    sregs.gs = data_seg;
    sregs.ss = data_seg;
    sregs.tr.type_ = 11;
    sregs.tr.present = 1;
    sregs.tr.s = 0;
    sregs.tr.unusable = 0;

    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_SET_SREGS, &sregs) } < 0 {
        bail!("KVM_SET_SREGS failed: {}", io::Error::last_os_error());
    }

    // 64-bit Linux Boot Protocol entry point: startup_64 = load_addr + 0x200, rsi = boot_params.
    let regs = KvmRegs {
        rip: (load_addr + 0x200) as u64,
        rsi: ZERO_PAGE_ADDR,
        rsp: 0x1_FF00,
        rflags: 0x2,
        ..KvmRegs::default()
    };
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_SET_REGS, &regs) } < 0 {
        bail!("KVM_SET_REGS failed: {}", io::Error::last_os_error());
    }

    // 9. Install no-op SIGUSR1 handler + watchdog thread so idle HLT never hangs past timeout_ms.
    install_sigusr1_wakeup();
    let deadline = start + Duration::from_millis(opts.timeout_ms.max(250));
    let finished = Arc::new(AtomicBool::new(false));
    let finished_wd = finished.clone();
    let vcpu_tid = unsafe { libc::pthread_self() };
    let watchdog = std::thread::spawn(move || {
        while !finished_wd.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                unsafe {
                    libc::pthread_kill(vcpu_tid, libc::SIGUSR1);
                }
                std::thread::sleep(Duration::from_millis(20));
            } else {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    });

    // 10. KVM_RUN hardware execution loop.
    let mut uart = Uart16550::default();
    let mut virtio_console = VirtioMmioConsole::default();
    let mut vm_exits = 0u64;
    let mut exit_code = 0i32;
    let mut timed_out = false;
    let exit_reason_label: String;

    loop {
        if Instant::now() >= deadline {
            timed_out = true;
            exit_code = 124;
            exit_reason_label = "timeout".into();
            break;
        }
        if opts.max_vm_exits > 0 && vm_exits >= opts.max_vm_exits {
            timed_out = true;
            exit_code = 137;
            exit_reason_label = "vm-exit-budget-exceeded".into();
            break;
        }

        let rc = unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_RUN, 0) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            finished.store(true, Ordering::Relaxed);
            let _ = watchdog.join();
            bail!("KVM_RUN failed: {err}");
        }
        vm_exits += 1;

        let exit_reason = unsafe { *(run_map.ptr.add(8) as *const u32) };
        match exit_reason {
            KVM_EXIT_IO => {
                let direction = unsafe { *run_map.ptr.add(32) };
                let size = unsafe { *run_map.ptr.add(33) } as usize;
                let port = unsafe { *(run_map.ptr.add(34) as *const u16) };
                let count = unsafe { *(run_map.ptr.add(36) as *const u32) } as usize;
                let data_offset = unsafe { *(run_map.ptr.add(40) as *const u64) } as usize;
                let total = size.saturating_mul(count);

                if data_offset.saturating_add(total) <= run_map.len {
                    let io_slice = unsafe {
                        std::slice::from_raw_parts_mut(run_map.ptr.add(data_offset), total)
                    };
                    if direction == KVM_EXIT_IO_OUT {
                        if (COM1_DATA..=COM1_DATA + 7).contains(&port) {
                            let mut trigger_irq = false;
                            let mut saw_newline = false;
                            for &b in io_slice.iter() {
                                if port == COM1_DATA && b == b'\n' {
                                    saw_newline = true;
                                }
                                if uart.write_port(port - COM1_DATA, b) {
                                    trigger_irq = true;
                                }
                            }
                            if trigger_irq {
                                pulse_irq_line(vm_fd.as_raw_fd(), 4);
                            }
                            if saw_newline
                                && let Some(code) = parse_coral_vmm_exit_marker(
                                    &String::from_utf8_lossy(&uart.output),
                                )
                            {
                                exit_code = code;
                                exit_reason_label = "coral-init-exit".into();
                                break;
                            }
                        } else if port == 0x64 && !io_slice.is_empty() && io_slice[0] == 0xFE {
                            // i8042 keyboard controller CPU reset (`reboot=k` clean shutdown)
                            exit_reason_label = "guest-reboot-poweroff".into();
                            break;
                        } else if port == 0xCF9 && !io_slice.is_empty() && (io_slice[0] & 0x04) != 0 {
                            exit_reason_label = "guest-pci-reset".into();
                            break;
                        } else if port == DEBUG_EXIT_PORT && !io_slice.is_empty() {
                            exit_code = io_slice[0] as i32;
                            exit_reason_label = "debug-exit".into();
                            break;
                        }
                    } else if direction == KVM_EXIT_IO_IN {
                        if (COM1_DATA..=COM1_DATA + 7).contains(&port) {
                            for b in io_slice.iter_mut() {
                                *b = uart.read_port(port - COM1_DATA);
                            }
                        } else if port == 0x61 {
                            // NMI / speaker refresh toggle bit (prevents busy loops during calibration)
                            io_slice.fill(0x20);
                        } else if port == 0xCF8 || port == 0xCFC {
                            // PCI config space absent (`pci=off`) -> all 1s
                            io_slice.fill(0xFF);
                        } else {
                            io_slice.fill(0);
                        }
                    }
                }
            }
            KVM_EXIT_MMIO => {
                let phys_addr = unsafe { *(run_map.ptr.add(32) as *const u64) };
                let data_ptr = unsafe { run_map.ptr.add(40) };
                let len = unsafe { *(run_map.ptr.add(48) as *const u32) } as usize;
                let is_write = unsafe { *run_map.ptr.add(52) } != 0;

                if (VirtioMmioConsole::BASE_ADDR
                    ..VirtioMmioConsole::BASE_ADDR + VirtioMmioConsole::SIZE)
                    .contains(&phys_addr)
                {
                    let reg_off = phys_addr - VirtioMmioConsole::BASE_ADDR;
                    if is_write && len >= 4 {
                        let val = unsafe { *(data_ptr as *const u32) };
                        virtio_console.write_u32(reg_off, val);
                    } else if !is_write && len >= 4 {
                        let val = virtio_console.read_u32(reg_off);
                        unsafe {
                            *(data_ptr as *mut u32) = val;
                        }
                    }
                } else if !is_write {
                    let safe_len = len.min(8);
                    unsafe {
                        std::slice::from_raw_parts_mut(data_ptr, safe_len).fill(0);
                    }
                }
            }
            KVM_EXIT_HLT => {
                exit_reason_label = "hlt".into();
                break;
            }
            KVM_EXIT_SHUTDOWN => {
                exit_reason_label = "guest-shutdown".into();
                break;
            }
            KVM_EXIT_SYSTEM_EVENT => {
                exit_reason_label = "system-event".into();
                break;
            }
            KVM_EXIT_INTERNAL_ERROR => {
                exit_code = 1;
                exit_reason_label = "kvm-internal-error".into();
                break;
            }
            other => {
                exit_code = 1;
                exit_reason_label = format!("kvm-exit-{other}");
                break;
            }
        }
    }

    finished.store(true, Ordering::Relaxed);
    let _ = watchdog.join();

    let serial_output = String::from_utf8_lossy(&uart.output).into_owned();
    if let Some(marker_code) = parse_coral_vmm_exit_marker(&serial_output) {
        exit_code = marker_code;
        timed_out = false;
    }

    Ok(LinuxBootResult {
        exit_code,
        serial_output,
        exit_reason: exit_reason_label,
        vm_exits,
        duration_ms: start.elapsed().as_millis() as u64,
        memory_mb: mem_mb,
        kernel_protocol,
        timed_out,
        virtio_mmio_probed: virtio_console.probed,
    })
}

#[cfg(not(target_arch = "x86_64"))]
pub fn boot_linux_bzimage_kvm(_opts: &LinuxBootOptions) -> Result<LinuxBootResult> {
    bail!("pure-Rust x86-64 Linux bzImage KVM boot is currently supported on x86_64 Linux hosts")
}

#[cfg(target_arch = "x86_64")]
fn write_e820_entry(mem: &mut [u8], offset: usize, addr: u64, size: u64, kind: u32) {
    mem[offset..offset + 8].copy_from_slice(&addr.to_le_bytes());
    mem[offset + 8..offset + 16].copy_from_slice(&size.to_le_bytes());
    mem[offset + 16..offset + 20].copy_from_slice(&kind.to_le_bytes());
}

#[cfg(target_arch = "x86_64")]
fn pulse_irq_line(vm_fd: i32, irq: u32) {
    #[repr(C)]
    struct KvmIrqLevel {
        irq: u32,
        level: u32,
    }
    let high = KvmIrqLevel { irq, level: 1 };
    let low = KvmIrqLevel { irq, level: 0 };
    unsafe {
        let _ = libc::ioctl(vm_fd, crate::kvm::KVM_IRQ_LINE, &high);
        let _ = libc::ioctl(vm_fd, crate::kvm::KVM_IRQ_LINE, &low);
    }
}

#[cfg(target_arch = "x86_64")]
extern "C" fn sigusr1_noop(_sig: libc::c_int) {}

#[cfg(target_arch = "x86_64")]
fn install_sigusr1_wakeup() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = sigusr1_noop as *const () as usize;
        sa.sa_flags = 0; // Explicitly omit SA_RESTART so KVM_RUN returns EINTR
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut());
    });
}

pub(crate) fn parse_coral_vmm_exit_marker(serial: &str) -> Option<i32> {
    serial
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("__CORAL_VMM_EXIT__="))
        .and_then(|v| v.trim().parse::<i32>().ok())
}
