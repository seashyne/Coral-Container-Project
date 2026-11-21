//! `coral pods` — CRI pod sandbox lifecycle subcommands.

use clap::{Args, Subcommand};

/// Arguments for `coral pods`.
#[derive(Debug, Args)]
pub struct PodsArgs {
    #[command(subcommand)]
    pub action: Option<PodAction>,
}

#[derive(Debug, Subcommand)]
pub enum PodAction {
    /// List pod sandboxes
    Ls,
    /// Stop a pod sandbox
    Stop {
        /// Pod sandbox ID
        id: String,
    },
    /// Remove a pod sandbox
    Rm {
        /// Pod sandbox ID
        id: String,
    },
}
