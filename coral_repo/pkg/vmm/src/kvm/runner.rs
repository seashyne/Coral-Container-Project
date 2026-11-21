//! Long Mode guest runner: boots a raw payload inside a hardware-isolated `/dev/kvm` MicroVM.

use super::regs::{KvmRegs, KvmSegment, KvmSregs, KvmUserspaceMemoryRegion};
use anyhow::{Context, Result, bail};
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

#[cfg(target_arch = "x86_64")]
const PML4_ADDR: u64 = 0x1000;
#[cfg(target_arch = "x86_64")]
const PDPT_ADDR: u64 = 0x2000;
#[cfg(target_arch = "x86_64")]
const PD_ADDR: u64 = 0x3000;
#[cfg(target_arch = "x86_64")]
const PAYLOAD_ADDR: u64 = 0x10000;

/// Boots a raw x86-64 Long Mode payload inside a fresh hardware-isolated Linux `/dev/kvm` MicroVM.
#[cfg(target_arch = "x86_64")]
pub fn run_long_mode_payload(payload: &[u8], opts: &super::KvmRunOptions) -> Result<super::KvmRunResult> {
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

    let api = unsafe { libc::ioctl(kvm_fd, super::KVM_GET_API_VERSION, 0) };
    if api != 12 {
        bail!("unsupported KVM API version {api} (expected 12)");
    }

    // 1. Create hardware VM fd.
    let vm_raw = unsafe { libc::ioctl(kvm_fd, super::KVM_CREATE_VM, 0) };
    if vm_raw < 0 {
        bail!("KVM_CREATE_VM failed: {}", io::Error::last_os_error());
    }
    let vm_fd = unsafe { OwnedFd::from_raw_fd(vm_raw) };

    // 2. Allocate isolated anonymous guest RAM and map into hardware EPT/NPT.
    let guest_mem = super::MmapGuard::anon_rw(mem_size)?;
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
    if unsafe { libc::ioctl(vm_fd.as_raw_fd(), super::KVM_SET_USER_MEMORY_REGION, &region) } < 0 {
        bail!(
            "KVM_SET_USER_MEMORY_REGION failed: {}",
            io::Error::last_os_error()
        );
    }

    // 5. Create vCPU 0, initialize supported CPUID2 (enables Long Mode LM/PAE/PSE), and mmap `kvm_run`.
    let vcpu_raw = unsafe { libc::ioctl(vm_fd.as_raw_fd(), super::KVM_CREATE_VCPU, 0) };
    if vcpu_raw < 0 {
        bail!("KVM_CREATE_VCPU failed: {}", io::Error::last_os_error());
    }
    let vcpu_fd = unsafe { OwnedFd::from_raw_fd(vcpu_raw) };
    setup_vcpu_cpuid(kvm_fd, vcpu_fd.as_raw_fd())?;

    let mmap_sz = unsafe { libc::ioctl(kvm_fd, super::KVM_GET_VCPU_MMAP_SIZE, 0) };
    if mmap_sz <= 0 {
        bail!("KVM_GET_VCPU_MMAP_SIZE failed: {}", io::Error::last_os_error());
    }
    let run_map = super::MmapGuard::fd_rw(vcpu_fd.as_raw_fd(), mmap_sz as usize)?;

    // 6. Configure vCPU 0 in 64-bit Long Mode (CR0/CR3/CR4/EFER + flat 64-bit CS/DS/SS).
    let mut sregs = KvmSregs::default();
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), super::KVM_GET_SREGS, &mut sregs) } < 0 {
        bail!("KVM_GET_SREGS failed: {}", io::Error::last_os_error());
    }
    configure_long_mode_sregs(&mut sregs);
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), super::KVM_SET_SREGS, &sregs) } < 0 {
        bail!("KVM_SET_SREGS failed: {}", io::Error::last_os_error());
    }

    let regs = KvmRegs {
        rip: PAYLOAD_ADDR,
        rsp: (mem_size as u64).saturating_sub(0x1000),
        rflags: 0x2, // bit 1 is architecturally always 1
        ..KvmRegs::default()
    };
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), super::KVM_SET_REGS, &regs) } < 0 {
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

        let rc = unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), super::KVM_RUN, 0) };
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
            super::KVM_EXIT_IO => {
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
                    if direction == super::KVM_EXIT_IO_OUT {
                        if port == super::COM1_DATA {
                            if serial.len() < 1024 * 1024 {
                                serial.extend_from_slice(io_slice);
                            }
                        } else if port == super::DEBUG_EXIT_PORT && !io_slice.is_empty() {
                            exit_code = io_slice[0] as i32;
                            exit_reason_label = "debug-exit".into();
                            break;
                        }
                    } else if direction == super::KVM_EXIT_IO_IN {
                        if port == super::COM1_LSR {
                            // UART Line Status Register: TX Holding Register Empty + Transmitter Empty
                            io_slice.fill(0x60);
                        } else {
                            io_slice.fill(0);
                        }
                    }
                }
            }
            super::KVM_EXIT_HLT => {
                exit_reason_label = "hlt".into();
                break;
            }
            super::KVM_EXIT_SHUTDOWN => {
                exit_code = 1;
                exit_reason_label = "triple-fault-shutdown".into();
                break;
            }
            super::KVM_EXIT_FAIL_ENTRY => {
                exit_code = 1;
                exit_reason_label = "fail-entry".into();
                break;
            }
            super::KVM_EXIT_INTERNAL_ERROR => {
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

    Ok(super::KvmRunResult {
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
pub fn run_long_mode_payload(
    _payload: &[u8],
    _opts: &super::KvmRunOptions,
) -> Result<super::KvmRunResult> {
    bail!("native x86-64 KVM Long Mode runner is only supported on x86_64 Linux hosts")
}

#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "x86_64")]
pub(crate) fn write_u64_le(mem: &mut [u8], offset: usize, val: u64) {
    mem[offset..offset + 8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(target_arch = "x86_64")]
pub(crate) fn setup_vcpu_cpuid(kvm_fd: i32, vcpu_fd: i32) -> Result<()> {
    // Allocate a flexible `kvm_cpuid2` buffer holding up to 128 `kvm_cpuid_entry2` (40 bytes each).
    const MAX_ENTRIES: usize = 128;
    let total_u32s = 2 + MAX_ENTRIES * 10;
    let mut buf = vec![0u32; total_u32s];
    buf[0] = MAX_ENTRIES as u32; // nent

    let rc = unsafe { libc::ioctl(kvm_fd, super::KVM_GET_SUPPORTED_CPUID, buf.as_mut_ptr()) };
    if rc < 0 {
        bail!(
            "KVM_GET_SUPPORTED_CPUID failed: {}",
            io::Error::last_os_error()
        );
    }
    let rc2 = unsafe { libc::ioctl(vcpu_fd, super::KVM_SET_CPUID2, buf.as_ptr()) };
    if rc2 < 0 {
        bail!("KVM_SET_CPUID2 failed: {}", io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
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
