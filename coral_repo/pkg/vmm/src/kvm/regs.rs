//! `libkvm` ABI-compatible register and memory-region structures.

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KvmUserspaceMemoryRegion {
    pub(crate) slot: u32,
    pub(crate) flags: u32,
    pub(crate) guest_phys_addr: u64,
    pub(crate) memory_size: u64,
    pub(crate) userspace_addr: u64,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KvmRegs {
    pub(crate) rax: u64,
    pub(crate) rbx: u64,
    pub(crate) rcx: u64,
    pub(crate) rdx: u64,
    pub(crate) rsi: u64,
    pub(crate) rdi: u64,
    pub(crate) rsp: u64,
    pub(crate) rbp: u64,
    pub(crate) r8: u64,
    pub(crate) r9: u64,
    pub(crate) r10: u64,
    pub(crate) r11: u64,
    pub(crate) r12: u64,
    pub(crate) r13: u64,
    pub(crate) r14: u64,
    pub(crate) r15: u64,
    pub(crate) rip: u64,
    pub(crate) rflags: u64,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KvmSegment {
    pub(crate) base: u64,
    pub(crate) limit: u32,
    pub(crate) selector: u16,
    pub(crate) type_: u8,
    pub(crate) present: u8,
    pub(crate) dpl: u8,
    pub(crate) db: u8,
    pub(crate) s: u8,
    pub(crate) l: u8,
    pub(crate) g: u8,
    pub(crate) avl: u8,
    pub(crate) unusable: u8,
    pub(crate) padding: u8,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KvmDtable {
    pub(crate) base: u64,
    pub(crate) limit: u16,
    pub(crate) padding: [u16; 3],
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct KvmSregs {
    pub(crate) cs: KvmSegment,
    pub(crate) ds: KvmSegment,
    pub(crate) es: KvmSegment,
    pub(crate) fs: KvmSegment,
    pub(crate) gs: KvmSegment,
    pub(crate) ss: KvmSegment,
    pub(crate) tr: KvmSegment,
    pub(crate) ldt: KvmSegment,
    pub(crate) gdt: KvmDtable,
    pub(crate) idt: KvmDtable,
    pub(crate) cr0: u64,
    pub(crate) cr2: u64,
    pub(crate) cr3: u64,
    pub(crate) cr4: u64,
    pub(crate) cr8: u64,
    pub(crate) efer: u64,
    pub(crate) apic_base: u64,
    pub(crate) interrupt_bitmap: [u64; 4],
}

// Compile-time ABI size assertions matching Linux <linux/kvm.h>.
const _: () = assert!(std::mem::size_of::<KvmUserspaceMemoryRegion>() == 32);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(std::mem::size_of::<KvmRegs>() == 144);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(std::mem::size_of::<KvmSregs>() == 312);
