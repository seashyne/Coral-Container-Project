//! `coral vm` — native Linux `/dev/kvm` MicroVM hypervisor subcommands.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Arguments for `coral vm`.
#[derive(Debug, Args)]
pub struct VmArgs {
    #[command(subcommand)]
    pub action: VmAction,
}

#[derive(Debug, Subcommand)]
pub enum VmAction {
    /// Probe host `/dev/kvm` capabilities and run a live 64-bit Long Mode hardware MicroVM self-test
    Probe {
        /// Output structured JSON capabilities and self-test telemetry
        #[arg(long)]
        json: bool,
    },
    /// Download and install an Alpine `linux-virt` guest kernel (or register a local bzImage) for pure-Rust KVM boot
    SetupKernel {
        /// Optional local bzImage path to copy instead of downloading Alpine `linux-virt`
        #[arg(long)]
        from: Option<PathBuf>,

        /// Destination path (defaults to `~/.local/state/coral/vmm/vmlinuz-virt`)
        #[arg(long)]
        dest: Option<PathBuf>,

        /// Output structured JSON installation report
        #[arg(long)]
        json: bool,
    },
    /// Boot an OCI image inside a real separate Linux guest kernel on Coral's pure-Rust `/dev/kvm` hypervisor (Zero QEMU)
    Boot {
        /// Base image tag or rootfs directory (e.g. `alpine:3.20`)
        #[arg(long, default_value = "alpine:3.20")]
        image: String,

        /// Guest Linux bzImage kernel path (defaults to `~/.local/state/coral/vmm/vmlinuz-virt`)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Guest RAM in MB (64..=1024)
        #[arg(short = 'm', long = "memory", default_value_t = 256)]
        memory_mb: usize,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 15)]
        timeout: u64,

        /// Extra kernel command line arguments
        #[arg(long)]
        cmdline: Option<String>,

        /// Attach persistent raw/ext4 disk image via VirtIO-Blk (`/dev/vda`; append `:ro` for read-only)
        #[arg(long)]
        disk: Option<String>,

        /// Auto-mount `/dev/vda` onto this guest path inside the MicroVM (requires `--disk`)
        #[arg(long)]
        disk_mount: Option<String>,

        /// Enable pure-Rust VirtIO-Net networking (`eth0` 10.0.2.15/24, gateway 10.0.2.2)
        #[arg(long)]
        net: bool,

        /// Forward host TCP port into MicroVM (`HOST_PORT:GUEST_PORT`, implies `--net`)
        #[arg(short = 'p', long = "publish")]
        ports: Vec<String>,

        /// Output structured JSON report
        #[arg(long)]
        json: bool,

        /// Command to run as PID 1 inside the separate Linux kernel MicroVM (pass after `--`)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Manage persistent VirtIO-Blk raw/ext4 disk images for Coral KVM MicroVMs
    Disk {
        #[command(subcommand)]
        action: VmDiskAction,
    },
    /// Boot a raw 64-bit guest payload or Linux bzImage kernel + initramfs inside `/dev/kvm`
    Run {
        /// Path to raw x86-64 Long Mode payload binary (ignored when `--kernel` is set)
        payload: Option<PathBuf>,

        /// Guest Linux kernel bzImage path (requires `--initrd`)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Guest compressed CPIO initramfs path (paired with `--kernel`)
        #[arg(long)]
        initrd: Option<PathBuf>,

        /// Extra Linux kernel cmdline parameters
        #[arg(long)]
        cmdline: Option<String>,

        /// Guest RAM in MB
        #[arg(short = 'm', long = "memory", default_value_t = 16)]
        memory_mb: usize,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 5)]
        timeout: u64,

        /// Maximum KVM VM-exits before terminating runaway guest
        #[arg(long, default_value_t = 100_000)]
        max_exits: u64,

        /// Output structured JSON execution report
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum VmDiskAction {
    /// Create a sparse VirtIO-Blk raw (or ext4-formatted) disk image for `coral vm boot --disk`
    Create {
        /// Output disk image path (e.g. `./data.img`)
        path: PathBuf,
        /// Disk capacity in MiB
        #[arg(long, default_value_t = 64)]
        size_mb: u64,
        /// Format the new disk image with ext4 via host `mkfs.ext4`
        #[arg(long)]
        ext4: bool,
    },
}
