//! In-kernel KVM IRQChip line pulsing and the SIGUSR1 watchdog wakeup handler.

pub(super) fn pulse_irq_line(vm_fd: i32, irq: u32) {
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

extern "C" fn sigusr1_noop(_sig: libc::c_int) {}

pub(super) fn install_sigusr1_wakeup() {
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
