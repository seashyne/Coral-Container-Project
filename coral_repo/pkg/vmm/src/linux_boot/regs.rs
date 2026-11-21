//! vCPU creation and 64-bit Long Mode register setup for the Linux boot protocol.

use anyhow::{Result, bail};
use crate::kvm::{
    KVM_CREATE_VCPU, KVM_GET_SREGS, KVM_GET_VCPU_MMAP_SIZE, KVM_SET_REGS, KVM_SET_SREGS,
    KvmDtable, KvmRegs, KvmSegment, KvmSregs, MmapGuard, setup_vcpu_cpuid,
};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

const GDT_ADDR: u64 = 0x0500;
const ZERO_PAGE_ADDR: u64 = 0x7000;
const PML4_ADDR: u64 = 0x9000;

/// The created vCPU file descriptor together with its mapped `kvm_run` shared region.
pub(super) struct VcpuHandle {
    pub vcpu_fd: OwnedFd,
    pub run_map: MmapGuard,
}

/// Creates vCPU 0, initializes CPUID2, mmaps `kvm_run`, and programs the 64-bit startup state.
pub(super) fn create_vcpu(
    kvm_fd: RawFd,
    vm_fd: RawFd,
    load_addr: usize,
) -> Result<VcpuHandle> {
    let vcpu_raw = unsafe { libc::ioctl(vm_fd, KVM_CREATE_VCPU, 0) };
    if vcpu_raw < 0 {
        bail!("KVM_CREATE_VCPU failed: {}", std::io::Error::last_os_error());
    }
    let vcpu_fd = unsafe { OwnedFd::from_raw_fd(vcpu_raw) };
    setup_vcpu_cpuid(kvm_fd, vcpu_fd.as_raw_fd())?;

    let mmap_sz = unsafe { libc::ioctl(kvm_fd, KVM_GET_VCPU_MMAP_SIZE, 0) };
    if mmap_sz <= 0 {
        bail!(
            "KVM_GET_VCPU_MMAP_SIZE failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let run_map = MmapGuard::fd_rw(vcpu_fd.as_raw_fd(), mmap_sz as usize)?;

    let mut sregs = KvmSregs::default();
    if unsafe { libc::ioctl(vcpu_fd.as_raw_fd(), KVM_GET_SREGS, &mut sregs) } < 0 {
        bail!("KVM_GET_SREGS failed: {}", std::io::Error::last_os_error());
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
        bail!("KVM_SET_SREGS failed: {}", std::io::Error::last_os_error());
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
        bail!("KVM_SET_REGS failed: {}", std::io::Error::last_os_error());
    }

    Ok(VcpuHandle { vcpu_fd, run_map })
}
