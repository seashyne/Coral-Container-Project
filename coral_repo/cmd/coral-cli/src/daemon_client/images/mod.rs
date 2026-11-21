//! Image management commands (`catalog`, `pull`, `images`, `rmi`, `build`, `commit`)
//! with automatic fallback between `coral-daemon` gRPC and local image storage.

mod build;
mod list;
mod pull;
mod registry;

use clap::ValueEnum;

pub use build::cmd_build;
pub use list::{cmd_catalog, cmd_commit_via_daemon, cmd_images, cmd_rmi};
pub use pull::cmd_pull;
pub use registry::{cmd_load, cmd_save, cmd_tag};

/// How `coral pull` reports image download progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ProgressMode {
    /// Live terminal progress bar on stderr (default; degrades to periodic lines when piped).
    Auto,
    /// One JSON snapshot per line on stderr (for CI and machine consumption).
    Json,
    /// No progress reporting.
    None,
}
