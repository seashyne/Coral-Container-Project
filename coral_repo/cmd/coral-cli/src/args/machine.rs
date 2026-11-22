//! `coral machine` CLI arguments for cross-platform VM / daemon environment management
//! (Linux native/KVM, macOS Lima + VirtioFS, Windows WSL2).

use clap::{Args, Subcommand};

/// Arguments for `coral machine`.
#[derive(Debug, Args)]
pub struct MachineArgs {
    #[command(subcommand)]
    pub action: MachineAction,
}

#[derive(Debug, Subcommand)]
pub enum MachineAction {
    /// Initialize a Coral machine profile (auto-detects Linux native, macOS Lima+VirtioFS, or Windows WSL2)
    Init {
        /// Machine profile name
        #[arg(default_value = "default")]
        name: String,

        /// Number of vCPUs allocated to the VM
        #[arg(long, default_value_t = 4)]
        cpus: u32,

        /// Memory in MiB allocated to the VM
        #[arg(long, default_value_t = 4096)]
        memory: u32,

        /// Root disk size in GiB
        #[arg(long, default_value_t = 40)]
        disk: u32,

        /// Backend override (`auto`, `native`, `lima`, `wsl2`)
        #[arg(long, default_value = "auto")]
        backend: String,
    },

    /// Start the Coral machine / daemon backend
    Start {
        /// Machine profile name
        #[arg(default_value = "default")]
        name: String,
    },

    /// Stop the Coral machine / daemon backend
    Stop {
        /// Machine profile name
        #[arg(default_value = "default")]
        name: String,
    },

    /// Show status, endpoints, and VirtioFS / backend configuration for a machine
    Status {
        /// Machine profile name
        #[arg(default_value = "default")]
        name: String,

        /// Output machine status as JSON
        #[arg(long)]
        json: bool,
    },

    /// Print shell environment exports (`CORAL_ADDR`, `DOCKER_HOST`) for `eval "$(coral machine env)"`
    Env {
        /// Machine profile name
        #[arg(default_value = "default")]
        name: String,

        /// Target shell format (`sh`, `fish`, `powershell`)
        #[arg(long, default_value = "sh")]
        shell: String,
    },
}
