//! Hardware `KVM_RUN` execution loop and VM-exit dispatch for the Linux MicroVM boot.

use super::irq::{install_sigusr1_wakeup, pulse_irq_line};
use super::{LinuxBootOptions, Uart16550, VirtioMmioConsole, parse_coral_vmm_exit_marker};
use crate::kvm::{
    COM1_DATA, DEBUG_EXIT_PORT, KVM_EXIT_HLT, KVM_EXIT_INTERNAL_ERROR, KVM_EXIT_IO, KVM_EXIT_IO_IN,
    KVM_EXIT_IO_OUT, KVM_EXIT_MMIO, KVM_EXIT_SHUTDOWN, KVM_EXIT_SYSTEM_EVENT, KVM_RUN, MmapGuard,
};
use anyhow::{Result, bail};
use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Outcome of the hardware execution loop.
pub(super) struct BootOutcome {
    pub exit_code: i32,
    pub exit_reason: String,
    pub vm_exits: u64,
    pub timed_out: bool,
}

/// Runs the guest until it exits, halts, reboots, or the wall-clock/VM-exit budget is hit.
#[allow(clippy::too_many_arguments)]
pub(super) fn run_guest(
    opts: &LinuxBootOptions,
    deadline: Instant,
    vm_fd: RawFd,
    vcpu_fd: RawFd,
    run_map: &MmapGuard,
    mem: &mut [u8],
    uart: &mut Uart16550,
    virtio_console: &mut VirtioMmioConsole,
    virtio_blk: &mut Option<crate::virtio_blk::VirtioBlkDevice>,
    virtio_net: &mut Option<crate::virtio_net::VirtioNetDevice>,
) -> Result<BootOutcome> {
    // Install no-op SIGUSR1 handler + watchdog thread so idle HLT never hangs past timeout_ms
    // (and wakes every 15 ms when VirtIO-Net is active so inbound RX packets arrive promptly).
    install_sigusr1_wakeup();
    let finished = Arc::new(AtomicBool::new(false));
    let finished_wd = finished.clone();
    let net_tick = opts.enable_net;
    let vcpu_tid = unsafe { libc::pthread_self() };
    let watchdog = std::thread::spawn(move || {
        while !finished_wd.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                unsafe {
                    libc::pthread_kill(vcpu_tid, libc::SIGUSR1);
                }
                std::thread::sleep(Duration::from_millis(20));
            } else if net_tick {
                std::thread::sleep(Duration::from_millis(15));
                unsafe {
                    libc::pthread_kill(vcpu_tid, libc::SIGUSR1);
                }
            } else {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    });

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

        if let Some(net) = virtio_net.as_mut()
            && net.poll(mem)
        {
            pulse_irq_line(vm_fd, crate::virtio_net::VIRTIO_NET_IRQ);
        }

        let rc = unsafe { libc::ioctl(vcpu_fd, KVM_RUN, 0) };
        if rc < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
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
                                pulse_irq_line(vm_fd, 4);
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
                } else if (crate::virtio_blk::VIRTIO_BLK_MMIO_BASE
                    ..crate::virtio_blk::VIRTIO_BLK_MMIO_BASE
                        + crate::virtio_blk::VIRTIO_BLK_MMIO_SIZE)
                    .contains(&phys_addr)
                {
                    let reg_off = phys_addr - crate::virtio_blk::VIRTIO_BLK_MMIO_BASE;
                    let safe_len = len.min(8);
                    if let Some(blk) = virtio_blk.as_mut() {
                        if is_write {
                            let slice = unsafe { std::slice::from_raw_parts(data_ptr, safe_len) };
                            if blk.write_mmio(reg_off, slice, mem) {
                                pulse_irq_line(vm_fd, crate::virtio_blk::VIRTIO_BLK_IRQ);
                            }
                        } else {
                            let slice =
                                unsafe { std::slice::from_raw_parts_mut(data_ptr, safe_len) };
                            blk.read_mmio(reg_off, slice);
                        }
                    } else if !is_write {
                        unsafe {
                            std::slice::from_raw_parts_mut(data_ptr, safe_len).fill(0);
                        }
                    }
                } else if (crate::virtio_net::VIRTIO_NET_MMIO_BASE
                    ..crate::virtio_net::VIRTIO_NET_MMIO_BASE
                        + crate::virtio_net::VIRTIO_NET_MMIO_SIZE)
                    .contains(&phys_addr)
                {
                    let reg_off = phys_addr - crate::virtio_net::VIRTIO_NET_MMIO_BASE;
                    let safe_len = len.min(8);
                    if let Some(net) = virtio_net.as_mut() {
                        if is_write {
                            let slice = unsafe { std::slice::from_raw_parts(data_ptr, safe_len) };
                            if net.write_mmio(reg_off, slice, mem) {
                                pulse_irq_line(vm_fd, crate::virtio_net::VIRTIO_NET_IRQ);
                            }
                        } else {
                            let slice =
                                unsafe { std::slice::from_raw_parts_mut(data_ptr, safe_len) };
                            net.read_mmio(reg_off, slice);
                        }
                    } else if !is_write {
                        unsafe {
                            std::slice::from_raw_parts_mut(data_ptr, safe_len).fill(0);
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

    Ok(BootOutcome {
        exit_code,
        exit_reason: exit_reason_label,
        vm_exits,
        timed_out,
    })
}
