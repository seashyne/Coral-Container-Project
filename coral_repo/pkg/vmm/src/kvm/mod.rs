//! Native pure-Rust Linux `/dev/kvm` hypervisor backend (x86-64 Long Mode).
//! Boots hardware-isolated guests directly via KVM ioctls with 4-level paging,
//! emulated 16550 COM1 serial console (`0x3F8`), debug exit port (`0xF4`),
//! and strict VM-exit + wall-clock timeout enforcement.

mod mmap;
mod regs;
mod runner;

pub(crate) use mmap::*;
pub(crate) use regs::*;
pub use runner::*;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::os::fd::AsRawFd;

// Exact Linux KVM ioctl constants verified against <linux/kvm.h>.
pub(crate) const KVM_GET_API_VERSION: libc::c_ulong = 0xae00;
pub(crate) const KVM_CREATE_VM: libc::c_ulong = 0xae01;
const KVM_CHECK_EXTENSION: libc::c_ulong = 0xae03;
pub(crate) const KVM_GET_VCPU_MMAP_SIZE: libc::c_ulong = 0xae04;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_GET_SUPPORTED_CPUID: libc::c_ulong = 0xc008_ae05;
pub(crate) const KVM_CREATE_VCPU: libc::c_ulong = 0xae41;
pub(crate) const KVM_SET_USER_MEMORY_REGION: libc::c_ulong = 0x4020_ae46;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_SET_TSS_ADDR: libc::c_ulong = 0xae47;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_SET_IDENTITY_MAP_ADDR: libc::c_ulong = 0x4008_ae48;
pub(crate) const KVM_CREATE_IRQCHIP: libc::c_ulong = 0xae60;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_IRQ_LINE: libc::c_ulong = 0x4008_ae61;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_CREATE_PIT2: libc::c_ulong = 0x4040_ae77;
pub(crate) const KVM_RUN: libc::c_ulong = 0xae80;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_SET_REGS: libc::c_ulong = 0x4090_ae82;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_GET_SREGS: libc::c_ulong = 0x8138_ae83;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_SET_SREGS: libc::c_ulong = 0x4138_ae84;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_SET_CPUID2: libc::c_ulong = 0x4008_ae90;

const KVM_CAP_IRQCHIP: libc::c_ulong = 0;
const KVM_CAP_USER_MEMORY: libc::c_ulong = 3;
const KVM_CAP_NR_VCPUS: libc::c_ulong = 9;

#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_IO: u32 = 2;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_HLT: u32 = 5;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_MMIO: u32 = 6;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_SHUTDOWN: u32 = 8;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_FAIL_ENTRY: u32 = 9;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_INTERNAL_ERROR: u32 = 17;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_SYSTEM_EVENT: u32 = 24;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_IO_IN: u8 = 0;
#[cfg(target_arch = "x86_64")]
pub(crate) const KVM_EXIT_IO_OUT: u8 = 1;

#[cfg(target_arch = "x86_64")]
pub(crate) const COM1_DATA: u16 = 0x3F8;
#[cfg(target_arch = "x86_64")]
const COM1_LSR: u16 = 0x3FD;
#[cfg(target_arch = "x86_64")]
pub(crate) const DEBUG_EXIT_PORT: u16 = 0x0F4;

/// Host KVM virtualization capability summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvmCapabilities {
    pub available: bool,
    pub arch: String,
    pub api_version: i32,
    pub max_vcpus: i32,
    pub has_user_memory: bool,
    pub has_irqchip: bool,
    pub external_vmm: Option<String>,
    pub detail: String,
}

/// Configuration for running a raw 64-bit guest binary inside a native KVM MicroVM.
#[derive(Debug, Clone)]
pub struct KvmRunOptions {
    /// Guest RAM size in MB (clamped to 2..=512 MB).
    pub memory_mb: usize,
    /// Hard wall-clock timeout in milliseconds.
    pub timeout_ms: u64,
    /// Maximum VM-exit budget before terminating a runaway guest.
    pub max_vm_exits: u64,
}

impl Default for KvmRunOptions {
    fn default() -> Self {
        Self {
            memory_mb: 16,
            timeout_ms: 5_000,
            max_vm_exits: 100_000,
        }
    }
}

/// Outcome returned by executing a guest payload inside a hardware KVM MicroVM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KvmRunResult {
    pub exit_code: i32,
    pub serial_output: String,
    pub exit_reason: String,
    pub vm_exits: u64,
    pub duration_usec: u64,
    pub memory_mb: usize,
    pub timed_out: bool,
}

/// Probes host `/dev/kvm` permissions, KVM API version, capabilities, and optional external VMMs.
pub fn probe_kvm() -> KvmCapabilities {
    let arch = std::env::consts::ARCH.to_string();
    let external_vmm = crate::sandbox::detect_external_vmm().map(|(k, _)| k.to_string());

    let Ok(file) = OpenOptions::new().read(true).write(true).open("/dev/kvm") else {
        return KvmCapabilities {
            available: false,
            arch,
            api_version: 0,
            max_vcpus: 0,
            has_user_memory: false,
            has_irqchip: false,
            external_vmm,
            detail: "/dev/kvm not accessible (enable VT-x/AMD-V or add user to 'kvm' group)".into(),
        };
    };

    let fd = file.as_raw_fd();
    let api_version = unsafe { libc::ioctl(fd, KVM_GET_API_VERSION, 0) };
    let has_user_memory = unsafe { libc::ioctl(fd, KVM_CHECK_EXTENSION, KVM_CAP_USER_MEMORY) } > 0;
    let has_irqchip = unsafe { libc::ioctl(fd, KVM_CHECK_EXTENSION, KVM_CAP_IRQCHIP) } > 0;
    let max_vcpus = unsafe { libc::ioctl(fd, KVM_CHECK_EXTENSION, KVM_CAP_NR_VCPUS) }.max(1);

    let available = api_version == 12 && has_user_memory;
    let detail = if available {
        format!("Linux KVM API v{api_version} ready (hardware virtualization active, up to {max_vcpus} vCPUs)")
    } else {
        format!("unexpected KVM API version {api_version}")
    };

    KvmCapabilities {
        available,
        arch,
        api_version,
        max_vcpus,
        has_user_memory,
        has_irqchip,
        external_vmm,
        detail,
    }
}

/// Runs Coral's built-in 64-bit x86-64 Long Mode guest payload inside a real `/dev/kvm`
/// hardware-virtualized MicroVM to attest VT-x/AMD-V isolation and measure boot latency.
pub fn run_kvm_hardware_selftest() -> Result<KvmRunResult> {
    let payload = build_64bit_selftest_payload(b"CORAL_KVM_VMM_64BIT_OK\n");
    run_long_mode_payload(&payload, &KvmRunOptions::default())
}

/// Builds a self-contained x86-64 Long Mode machine-code guest that streams `message`
/// to emulated 16550 serial port `0x3F8`, signals clean exit `0` on port `0xF4`, and halts.
pub fn build_64bit_selftest_payload(message: &[u8]) -> Vec<u8> {
    let mut code = Vec::with_capacity(message.len() * 6 + 32);
    // mov dx, 0x03F8 (66 ba f8 03)
    code.extend_from_slice(&[0x66, 0xba, 0xf8, 0x03]);
    for &b in message {
        // mov al, b (b0 <b>)
        code.push(0xb0);
        code.push(b);
        // out dx, al (ee)
        code.push(0xee);
    }
    // Signal exit code 0 to debug exit port 0xF4: xor eax, eax; out 0xF4, al; hlt
    code.extend_from_slice(&[0x31, 0xc0, 0xe6, 0xf4, 0xf4]);
    code
}
