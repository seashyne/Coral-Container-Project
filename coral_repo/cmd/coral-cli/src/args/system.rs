//! `coral system` — disk usage and resource cleanup subcommands.

use clap::{Args, Subcommand};

/// Arguments for `coral system`.
#[derive(Debug, Args)]
pub struct SystemArgs {
    #[command(subcommand)]
    pub action: SystemAction,
}

#[derive(Debug, Subcommand)]
pub enum SystemAction {
    /// Show Coral disk usage (images, containers, named volumes, build cache)
    Df,
    /// Remove stopped containers, unused pod sandboxes, and build cache
    Prune {
        /// Also remove all unused managed images
        #[arg(short = 'a', long)]
        all: bool,

        /// Also remove all unused named volumes
        #[arg(long)]
        volumes: bool,
    },
}
