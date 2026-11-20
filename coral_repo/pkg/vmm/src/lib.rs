//! Coral VMM — Pure-Rust Linux `/dev/kvm` 64-bit MicroVM hypervisor, Linux kernel boot loader
//! (Zero QEMU / Zero Firecracker), VirtIO-MMIO transport, initramfs builder, and AI Agent Sandbox.

pub mod initramfs;
pub mod kernel;
pub mod kvm;
pub mod linux_boot;
pub mod sandbox;

pub use initramfs::{BzImageInfo, build_initramfs_cpio_gz, inspect_bzimage};
pub use kernel::{
    KernelInstallReport, default_kernel_path, extract_bzimage_from_apk, install_kernel_from_file,
    setup_alpine_microvm_kernel,
};
pub use kvm::{
    KvmCapabilities, KvmRunOptions, KvmRunResult, build_64bit_selftest_payload, probe_kvm,
    run_kvm_hardware_selftest, run_long_mode_payload,
};
pub use linux_boot::{
    LinuxBootOptions, LinuxBootResult, Uart16550, VirtioMmioConsole, boot_linux_bzimage_kvm,
};
pub use sandbox::{
    SandboxPolicy, SandboxReport, boot_linux_microvm, detect_external_vmm, run_ai_sandbox,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn kvm_hardware_64bit_long_mode_microvm_boots_and_outputs_serial() {
        let caps = probe_kvm();
        if !caps.available || !cfg!(target_arch = "x86_64") {
            eprintln!("skipping hardware KVM test: {}", caps.detail);
            return;
        }
        let res = run_kvm_hardware_selftest().expect("64-bit KVM self-test should succeed");
        assert_eq!(res.exit_code, 0);
        assert!(!res.timed_out);
        assert_eq!(res.serial_output, "CORAL_KVM_VMM_64BIT_OK\n");
        assert!(res.vm_exits >= 20);
    }

    #[test]
    fn kvm_microvm_enforces_vm_exit_budget_on_runaway_guest() {
        let caps = probe_kvm();
        if !caps.available || !cfg!(target_arch = "x86_64") {
            return;
        }
        // Infinite serial write loop: mov dx, 0x3F8; mov al, 'X'; loop: out dx, al; jmp loop
        let runaway: &[u8] = &[0x66, 0xba, 0xf8, 0x03, 0xb0, b'X', 0xee, 0xeb, 0xfd];
        let opts = KvmRunOptions {
            memory_mb: 8,
            timeout_ms: 2_000,
            max_vm_exits: 64,
        };
        let res = run_long_mode_payload(runaway, &opts).unwrap();
        assert!(res.timed_out);
        assert_eq!(res.exit_reason, "vm-exit-budget-exceeded");
        assert_eq!(res.vm_exits, 64);
    }

    #[test]
    fn builds_valid_newc_cpio_initramfs_archive() {
        let dir = tempdir().unwrap();
        let rootfs = dir.path().join("rootfs");
        fs::create_dir_all(rootfs.join("etc")).unwrap();
        fs::write(rootfs.join("etc/hello.txt"), "CORAL_INITRD_OK\n").unwrap();

        let out = dir.path().join("initrd.cpio.gz");
        let sz = build_initramfs_cpio_gz(
            &rootfs,
            &["/bin/sh".into(), "-c".into(), "echo hi".into()],
            &out,
        )
        .unwrap();
        assert!(sz > 64);

        // Decompress and check SVR4 `070701` magic + coral-init + TRAILER!!!
        let raw = fs::read(&out).unwrap();
        let mut dec = flate2::read::GzDecoder::new(raw.as_slice());
        let mut unpacked = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut unpacked).unwrap();
        let text = String::from_utf8_lossy(&unpacked);
        assert!(text.starts_with("070701"));
        assert!(text.contains("coral-init"));
        assert!(text.contains("etc/hello.txt"));
        assert!(text.contains("TRAILER!!!"));
    }

    #[test]
    fn uart_16550a_supports_loopback_autoconfig_and_tx_irq() {
        let mut uart = Uart16550::default();
        // Enable loopback (MCR bit 4) and send probe byte
        uart.write_port(4, 0x10);
        uart.write_port(0, 0x5A);
        assert_eq!(uart.read_port(5) & 0x01, 0x01); // Data Ready set
        assert_eq!(uart.read_port(0), 0x5A);
        // Disable loopback, enable TX IRQ (IER bit 1), emit serial bytes
        uart.write_port(4, 0x03);
        assert!(uart.write_port(1, 0x02));
        assert!(uart.write_port(0, b'O'));
        assert!(uart.write_port(0, b'K'));
        assert_eq!(uart.output, b"OK");
        assert_eq!(uart.read_port(2), 0xC2); // THRI pending
        assert_eq!(uart.read_port(2), 0xC1); // Cleared after read
    }

    #[test]
    fn virtio_mmio_console_exposes_virtio_v2_registers() {
        let mut dev = VirtioMmioConsole::default();
        assert_eq!(dev.read_u32(0x000), VirtioMmioConsole::MAGIC_VALUE);
        assert_eq!(dev.read_u32(0x004), 2);
        assert_eq!(dev.read_u32(0x008), VirtioMmioConsole::DEVICE_ID_CONSOLE);
        dev.write_u32(0x014, 1);
        assert_eq!(dev.read_u32(0x010), 1); // VIRTIO_F_VERSION_1
        dev.write_u32(0x070, 0x0F);
        assert_eq!(dev.read_u32(0x070), 0x0F);
        assert!(dev.probed);
    }

    #[test]
    fn extracts_synthetic_bzimage_from_alpine_apk_stream() {
        let dir = tempdir().unwrap();
        let mut fake_bz = vec![0u8; 4096];
        fake_bz[0x1F1] = 4; // setup_sects
        fake_bz[0x202..0x206].copy_from_slice(b"HdrS");
        fake_bz[0x206..0x208].copy_from_slice(&0x020Fu16.to_le_bytes()); // protocol 2.15

        // Build a valid gzipped tar stream containing `boot/vmlinuz-virt`
        let mut gz_buf = Vec::new();
        {
            let gz = flate2::write::GzEncoder::new(&mut gz_buf, flate2::Compression::fast());
            let mut tar_builder = tar::Builder::new(gz);
            let mut hdr = tar::Header::new_gnu();
            hdr.set_size(fake_bz.len() as u64);
            hdr.set_mode(0o644);
            hdr.set_cksum();
            tar_builder
                .append_data(&mut hdr, "boot/vmlinuz-virt", fake_bz.as_slice())
                .unwrap();
            let gz = tar_builder.into_inner().unwrap();
            gz.finish().unwrap();
        }

        let dest = dir.path().join("vmlinuz-virt");
        let info = extract_bzimage_from_apk(&gz_buf, &dest).unwrap();
        assert_eq!(info.protocol_major, 2);
        assert_eq!(info.protocol_minor, 15);
        assert_eq!(info.total_size, 4096);
    }
}
