//! `coral volume` — persistent named volume subcommands.

use clap::{Args, Subcommand};

/// Arguments for `coral volume`.
#[derive(Debug, Args)]
pub struct VolumeArgs {
    #[command(subcommand)]
    pub action: Option<VolumeAction>,
}

#[derive(Debug, Subcommand)]
pub enum VolumeAction {
    /// Create a persistent named volume
    Create {
        /// Volume name
        name: String,
    },
    /// List persistent named volumes
    Ls,
    /// Print filesystem path and metadata for a named volume
    Inspect {
        /// Volume name
        name: String,
    },
    /// Remove a named volume and its stored data
    Rm {
        /// Volume name
        name: String,
    },
}
