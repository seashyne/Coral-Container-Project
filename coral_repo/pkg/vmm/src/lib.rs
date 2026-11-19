//! Coral VMM — Native Linux `/dev/kvm` 64-bit MicroVM hypervisor, Linux initramfs builder,
//! and AI Agent / Untrusted Code Sandbox engine.

pub mod initramfs;
pub mod kvm;
pub mod sandbox;

pub use initramfs::{BzImageInfo, build_initramfs_cpio_gz, inspect_bzimage};
pub use kvm::{
    KvmCapabilities, KvmRunOptions, KvmRunResult, build_64bit_selftest_payload, probe_kvm,
    run_kvm_hardware_selftest, run_long_mode_payload,
};
pub use sandbox::{SandboxPolicy, SandboxReport, boot_linux_microvm, detect_external_vmm, run_ai_sandbox};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn kvm_hardware_64bit_long_mode_microvm_boots_and_outputs_serial() {
        let caps = probe_kvm();
        if !caps.available {
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
        if !caps.available {
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
        let sz = build_initramfs_cpio_gz(&rootfs, &["/bin/sh".into(), "-c".into(), "echo hi".into()], &out)
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
}
