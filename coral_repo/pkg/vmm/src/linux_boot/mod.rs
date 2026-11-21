//! Pure-Rust Linux x86-64 64-bit Kernel Boot Engine over native `/dev/kvm` (Zero QEMU / Zero Firecracker).
//!
//! Implements the official Linux x86-64 64-bit Boot Protocol (v2.06+):
//! - In-kernel KVM IRQChip (LAPIC + IOAPIC + PIC) + PIT2 timer
//! - 4-level identity page tables covering 0..4 GiB (2 MiB huge pages) + flat 64-bit GDT
//! - `boot_params` (`zero_page`) with `setup_header`, E820 RAM map, kernel cmdline, and initramfs
//! - Full 16550A COM1 UART (`0x3F8..0x3FF`) with loopback autoconfig & KVM IRQ 4 pulsing
//! - VirtIO-MMIO v2 transport device plane (`0xD000_0000..0xD000_1000`)
//! - Watchdog signal interruption for deterministic wall-clock timeout even during guest `HLT`

#[cfg(target_arch = "x86_64")]
mod boot;
#[cfg(target_arch = "x86_64")]
mod irq;
#[cfg(target_arch = "x86_64")]
mod regs;
#[cfg(target_arch = "x86_64")]
mod run_loop;
#[cfg(target_arch = "x86_64")]
mod setup;

use anyhow::Result;
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
    /// Optional raw/ext4 disk image attached via VirtIO-Blk (`/dev/vda`, `0xD0001000`, IRQ 5).
    pub disk_image: Option<PathBuf>,
    /// Attach `disk_image` read-only.
    pub disk_read_only: bool,
    /// Attach pure-Rust VirtIO-Net userspace L2/L3 switch (`eth0` `10.0.2.15/24`, IRQ 6).
    pub enable_net: bool,
    /// Host-to-guest TCP port forwarding rules when `enable_net` is true.
    pub port_forwards: Vec<crate::virtio_net::PortForwardRule>,
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
            disk_image: None,
            disk_read_only: false,
            enable_net: false,
            port_forwards: Vec::new(),
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
    #[serde(default)]
    pub virtio_blk_ops: u64,
    #[serde(default)]
    pub virtio_net_packets: u64,
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
    boot::run_linux_boot(opts)
}

#[cfg(not(target_arch = "x86_64"))]
pub fn boot_linux_bzimage_kvm(_opts: &LinuxBootOptions) -> Result<LinuxBootResult> {
    anyhow::bail!("pure-Rust x86-64 Linux bzImage KVM boot is currently supported on x86_64 Linux hosts")
}

pub(crate) fn parse_coral_vmm_exit_marker(serial: &str) -> Option<i32> {
    serial
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("__CORAL_VMM_EXIT__="))
        .and_then(|v| v.trim().parse::<i32>().ok())
}
