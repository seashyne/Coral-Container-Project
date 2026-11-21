//! `coral network` — built-in and user-defined container network subcommands.

use clap::{Args, Subcommand};

/// Arguments for `coral network`.
#[derive(Debug, Args)]
pub struct NetworkArgs {
    #[command(subcommand)]
    pub action: Option<NetworkAction>,
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    /// Create a user-defined isolated bridge network
    Create {
        /// Network name (e.g. `backend`)
        name: String,

        /// Network driver (`bridge`)
        #[arg(long, default_value = "bridge")]
        driver: String,

        /// Optional IPv4 CIDR subnet (e.g. `10.89.10.0/24`; auto-allocated if omitted)
        #[arg(long)]
        subnet: Option<String>,
    },
    /// List built-in and user-defined networks
    Ls,
    /// Print JSON configuration for a network
    Inspect {
        /// Network name
        name: String,
    },
    /// Remove a user-defined network
    Rm {
        /// Network name
        name: String,
    },
}
