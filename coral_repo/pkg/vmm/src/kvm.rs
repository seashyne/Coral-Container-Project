//! Native pure-Rust Linux `/dev/kvm` hypervisor backend (x86-64 Long Mode).
//! Boots hardware-isolated guests directly via KVM ioctls with 4-level paging,
//! emulated 16550 COM1 serial console (`0x3F8`), debug exit port (`0xF4`),
//! and strict VM-exit + wall-clock timeout enforcement.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

// Exact Linux x86-64 KVM ioctl constants verified against <linux/kvm.h>.
const KVM_GET_API_VERSION: libc::c_ulong = 0xae00;
const KVM_CREATE_VM: libc::c_ulong = 0xae01;
const KVM_CHECK_EXTENSION: libc::c_ulong = 0xae03;
const KVM_GET_VCPU_MMAP_SIZE: libc::c_ulong = 0xae04;
const KVM_GET_SUPPORTED_CPUID: libc::c_ulong = 0xc008_ae05;
const KVM_CREATE_VCPU: libc::c_ulong = 0xae41;
const KVM_SET_USER_MEMORY_REGION: libc::c_ulong = 0x4020_ae46;
const KVM_RUN: libc::c_ulong = 0xae80;
const KVM_SET_REGS: libc::c_ulong = 0x4090_ae82;
const KVM_GET_SREGS: libc::c_ulong = 0x8138_ae83;
const KVM_SET_SREGS: libc::c_ulong = 0x4138_ae84;
const KVM_SET_CPUID2: libc::c_ulong = 0x4008_ae90;

const KVM_CAP_IRQCHIP: libc::c_ulong = 0;
const KVM_CAP_USER_MEMORY: libc::c_ulong = 3;
const KVM_CAP_NR_VCPUS: libc::c_ulong = 9;

const KVM_EXIT_IO: u32 = 2;
const KVM_EXIT_HLT: u32 = 5;
const KVM_EXIT_SHUTDOWN: u32 = 8;
const KVM_EXIT_FAIL_ENTRY: u32 = 9;
const KVM_EXIT_INTERNAL_ERROR: u32 = 17;
const KVM_EXIT_IO_IN: u8 = 0;
const KVM_EXIT_IO_OUT: u8 = 1;

// Guest physical memory layout for 64-bit Long Mode MicroVMs.
const PML4_ADDR: u64 = 0x1000;
const PDPT_ADDR: u64 = 0x2000;
const PD_ADDR: u64 = 0x3000;
const PAYLOAD_ADDR: u64 = 0x10000;

const COM1_DATA: u16 = 0x3F8;
const COM1_LSR: u16 = 0x3FD;
const DEBUG_EXIT_PORT: u16 = 0x0F4;

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KvmUserspaceMemoryRegion {
    slot: u32,
    flags: u32,
    guest_phys_addr: u64,
    memory_size: u64,
    userspace_addr: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KvmRegs {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    rsp: u64,
    rbp: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rip: u64,
    rflags: u64,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KvmSegment {
    base: u64,
    limit: u32,
    selector: u16,
    type_: u8,
    present: u8,
    dpl: u8,
    db: u8,
    s: u8,
    l: u8,
    g: u8,
    avl: u8,
    unusable: u8,
    padding: u8,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KvmDtable {
    base: u64,
    limit: u16,
    padding: [u16; 3],
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KvmSregs {
    cs: KvmSegment,
    ds: KvmSegment,
    es: KvmSegment,
    fs: KvmSegment,
    gs: KvmSegment,
    ss: KvmSegment,
    tr: KvmSegment,
    ldt: KvmSegment,
    gdt: KvmDtable,
    idt: KvmDtable,
    cr0: u64,
    cr2: u64,
    cr3: u64,
    cr4: u64,
    cr8: u64,
    efer: u64,
    apic_base: u64,
    interrupt_bitmap: [u64; 4],
}

// Compile-time ABI size assertions matching Linux x86-64 <linux/kvm.h>.
const _: () = assert!(std::mem::size_of::<KvmUserspaceMemoryRegion>() == 32);
const _: () = assert!(std::mem::size_of::<KvmRegs>() == 144);
const _: () = assert!(std::mem::size_of::<KvmSregs>() == 312);

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

/// Boots a raw x86-64 Long Mode payload inside a fresh hardware-isolated Linux `/dev/kvm` MicroVM.
#[cfg(target_arch = "x86_64")]
pub fn run_long_mode_payload(payload: &[u8], opts: &KvmRunOptions) -> Result<KvmRunResult> {
    let start = Instant::now();
    let mem_mb = opts.memory_mb.clamp(2, 512);
    let mem_size = mem_mb * 1024 * 1024;
    if payload.len() + PAYLOAD_ADDR as usize + 4096 >= mem_size {
        bail!("guest payload ({} bytes) exceeds guest RAM ({mem_mb} MB)", payload.len());
    }

    let kvm_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/kvm")
        .context("open /dev/kvm (ensure VT-x/AMD-V and kvm permissions)")?;
    let kvm_fd = kvm_file.as_raw_fd();

    let api = unsafe { libc::ioctl(kvm_fd, KVM_GET_API_VERSION, 0) };
    if api != 12 {
        bail!("unsupported KVM API version {api} (expected 12)");
    }

    // 1. Create hardware VM fd.
    let vm_raw = unsafe { libc::ioctl(kvm_fd, KVM_CREATE_VM, 0) };
    if vm_raw < 0 {
        bail!("KVM_CREATE_VM failed: {}", io::Error::last_os_error());
    }
    let vm_fd = unsafe { OwnedFd::from_raw_fd(vm_raw) };

    // 2. Allocate isolated anonymous guest RAM and map into hardware EPT/NPT.
    let guest_mem = MmapGuard::anon_rw(mem_size)?;
    let mem_slice = unsafe { std::slice::from_raw_parts_mut(guest_mem.ptr, guest_mem.len) };

    // 3. Build 64-bit identity page tables (2 MiB huge pages covering guest RAM).
    setup_long_mode_page_tables(mem_slice, mem_mb);

    // 4. Copy guest payload at `PAYLOAD_ADDR` (0x10000).
    let p_off = PAYLOAD_ADDR as usize;
    mem_slice[p_off..p_off + payload.len()].copy_from_slice(payload);

    let region = KvmUserspaceMemoryRegion {
        slot: 0,
        flags: 0,
        guest_phys_addr: 0,
        memory_size: mem_size as u64,
        userspace_addr: guest_mem.ptr as u64,
    };
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), KVM_SET_USER_MEMORY_REGION, &region) } < 0 {
        bail!(
            "KVM_SET_USER_MEMORY_REGION failed: {}",
            io::Error::last_os_error()
        );
    }

    // 5. Create vCPU 0, initialize supported CPUID2 (enables Long Mode LM/PAE/PSE), and mmap `kvm_run`.
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

    // 6. Configure vCPU 0 in 64-bit Long Mode (CR0/CR3/CR4/EFER + flat 64-bit CS/DS/SS).
    let mut sregs = KvmSregs::default();
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_GET_SREGS, &mut sregs) } < 0 {
        bail!("KVM_GET_SREGS failed: {}", io::Error::last_os_error());
    }
    configure_long_mode_sregs(&mut sregs);
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_SET_SREGS, &sregs) } < 0 {
        bail!("KVM_SET_SREGS failed: {}", io::Error::last_os_error());
    }

    let regs = KvmRegs {
        rip: PAYLOAD_ADDR,
        rsp: (mem_size as u64).saturating_sub(0x1000),
        rflags: 0x2, // bit 1 is architecturally always 1
        ..KvmRegs::default()
    };
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_SET_REGS, &regs) } < 0 {
        bail!("KVM_SET_REGS failed: {}", io::Error::last_os_error());
    }

    // 7. Enter hardware `KVM_RUN` VM-exit dispatch loop.
    let deadline = start + Duration::from_millis(opts.timeout_ms.max(10));
    let mut serial = Vec::new();
    let mut vm_exits = 0u64;
    let mut exit_code = 0i32;
    let exit_reason_label: String;
    let mut timed_out = false;

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
                        if port == COM1_DATA {
                            if serial.len() < 1024 * 1024 {
                                serial.extend_from_slice(io_slice);
                            }
                        } else if port == DEBUG_EXIT_PORT && !io_slice.is_empty() {
                            exit_code = io_slice[0] as i32;
                            exit_reason_label = "debug-exit".into();
                            break;
                        }
                    } else if direction == KVM_EXIT_IO_IN {
                        if port == COM1_LSR {
                            // UART Line Status Register: TX Holding Register Empty + Transmitter Empty
                            io_slice.fill(0x60);
                        } else {
                            io_slice.fill(0);
                        }
                    }
                }
            }
            KVM_EXIT_HLT => {
                exit_reason_label = "hlt".into();
                break;
            }
            KVM_EXIT_SHUTDOWN => {
                exit_code = 1;
                exit_reason_label = "triple-fault-shutdown".into();
                break;
            }
            KVM_EXIT_FAIL_ENTRY => {
                exit_code = 1;
                exit_reason_label = "fail-entry".into();
                break;
            }
            KVM_EXIT_INTERNAL_ERROR => {
                exit_code = 1;
                exit_reason_label = "internal-error".into();
                break;
            }
            other => {
                exit_code = 1;
                exit_reason_label = format!("kvm-exit-{other}");
                break;
            }
        }
    }

    Ok(KvmRunResult {
        exit_code,
        serial_output: String::from_utf8_lossy(&serial).into_owned(),
        exit_reason: exit_reason_label,
        vm_exits,
        duration_usec: start.elapsed().as_micros() as u64,
        memory_mb: mem_mb,
        timed_out,
    })
}

#[cfg(not(target_arch = "x86_64"))]
pub fn run_long_mode_payload(_payload: &[u8], _opts: &KvmRunOptions) -> Result<KvmRunResult> {
    bail!("native x86-64 KVM Long Mode runner is only supported on x86_64 Linux hosts")
}

fn setup_long_mode_page_tables(mem: &mut [u8], mem_mb: usize) {
    // PML4[0] -> PDPT (Present | Writable)
    write_u64_le(mem, PML4_ADDR as usize, PDPT_ADDR | 0x3);
    // PDPT[0] -> PD (Present | Writable)
    write_u64_le(mem, PDPT_ADDR as usize, PD_ADDR | 0x3);
    // Map enough 2 MiB huge pages (Present | Writable | PS=0x80) to cover guest RAM (up to 512 entries = 1 GiB)
    let num_pages = mem_mb.div_ceil(2).clamp(1, 512);
    for i in 0..num_pages {
        let phys = (i as u64) * 0x20_0000;
        write_u64_le(mem, PD_ADDR as usize + i * 8, phys | 0x83);
    }
}

fn write_u64_le(mem: &mut [u8], offset: usize, val: u64) {
    mem[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

fn setup_vcpu_cpuid(kvm_fd: i32, vcpu_fd: i32) -> Result<()> {
    // Allocate a flexible `kvm_cpuid2` buffer holding up to 128 `kvm_cpuid_entry2` (40 bytes each).
    const MAX_ENTRIES: usize = 128;
    let total_u32s = 2 + MAX_ENTRIES * 10;
    let mut buf = vec![0u32; total_u32s];
    buf[0] = MAX_ENTRIES as u32; // nent

    let rc = unsafe { libc::ioctl(kvm_fd, KVM_GET_SUPPORTED_CPUID, buf.as_mut_ptr()) };
    if rc < 0 {
        bail!(
            "KVM_GET_SUPPORTED_CPUID failed: {}",
            io::Error::last_os_error()
        );
    }
    let rc2 = unsafe { libc::ioctl(vcpu_fd, KVM_SET_CPUID2, buf.as_ptr()) };
    if rc2 < 0 {
        bail!("KVM_SET_CPUID2 failed: {}", io::Error::last_os_error());
    }
    Ok(())
}

fn configure_long_mode_sregs(sregs: &mut KvmSregs) {
    sregs.cr3 = PML4_ADDR;
    sregs.cr4 = 1 << 5; // PAE
    sregs.cr0 = (1 << 0) | (1 << 1) | (1 << 4) | (1 << 5) | (1 << 16) | (1 << 31); // PE | MP | ET | NE | WP | PG
    sregs.efer = (1 << 8) | (1 << 10); // LME | LMA

    let code_seg = KvmSegment {
        base: 0,
        limit: 0xFFFF_FFFF,
        selector: 0x08,
        type_: 11, // Execute/Read, accessed
        present: 1,
        dpl: 0,
        db: 0,
        s: 1,
        l: 1, // 64-bit Long Mode code segment
        g: 1,
        avl: 0,
        unusable: 0,
        padding: 0,
    };
    let data_seg = KvmSegment {
        base: 0,
        limit: 0xFFFF_FFFF,
        selector: 0x10,
        type_: 3, // Read/Write, accessed
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
    // Intel VMX requires 32/64-bit busy TSS (`type_ = 11`) in Long Mode.
    sregs.tr.type_ = 11;
    sregs.tr.present = 1;
    sregs.tr.s = 0;
    sregs.tr.unusable = 0;
}

struct MmapGuard {
    ptr: *mut u8,
    len: usize,
}

impl MmapGuard {
    fn anon_rw(len: usize) -> Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            bail!("mmap guest RAM ({len} bytes) failed: {}", io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }

    fn fd_rw(fd: i32, len: usize) -> Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            bail!("mmap kvm_run failed: {}", io::Error::last_os_error());
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }
}

impl Drop for MmapGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::munmap(self.ptr.cast(), self.len);
        }
    }
}
