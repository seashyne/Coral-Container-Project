//! `coral sandbox` — hardened AI-agent / untrusted-code sandbox subcommands.

use crate::units::parse_memory_mb;
use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Arguments for `coral sandbox`.
#[derive(Debug, Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub action: SandboxAction,
}

#[derive(Debug, Subcommand)]
pub enum SandboxAction {
    /// Execute untrusted code / AI Agent workloads inside a hardened timeout-enforced sandbox
    /// (auto-upgrades to a full Linux KVM MicroVM when `--kernel` or `CORAL_VMM_KERNEL` is provided)
    Run {
        /// Base image tag or rootfs path
        #[arg(long, default_value = "alpine:3.20")]
        image: String,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 10)]
        timeout: u64,

        /// Memory ceiling (supports `64`, `64m`, `2G`)
        #[arg(short = 'm', long = "memory", default_value = "64", value_parser = parse_memory_mb)]
        memory_mb: i64,

        /// CPU quota as percentage of one core
        #[arg(long = "cpu", default_value_t = 50)]
        cpu_percent: i64,

        /// Maximum processes/threads
        #[arg(long = "pids", visible_alias = "pids-limit", default_value_t = 32)]
        pids_limit: i64,

        /// Allow isolated bridge networking instead of air-gapped `--network none`
        #[arg(long)]
        allow_network: bool,

        /// Optional Linux guest kernel bzImage for full separate-kernel KVM MicroVM execution
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Output structured JSON report (`isolationMode`, `kvmHardwareAvailable`, `stdout`, `stderr`, `timedOut`)
        #[arg(long)]
        json: bool,

        /// Untrusted command and arguments (pass after `--`)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
}
