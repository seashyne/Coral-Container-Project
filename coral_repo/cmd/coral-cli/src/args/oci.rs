//! Local OCI runtime subcommands (`spec`, `create`, `start`, `state`, `kill`, `delete`,
//! `cni-config`).

use clap::Args;
use std::path::PathBuf;

/// Arguments for `coral spec`.
#[derive(Debug, Args)]
pub struct SpecArgs {
    /// Generate a rootless OCI spec with UserNS UID/GID mappings
    #[arg(long)]
    pub rootless: bool,

    /// Path to the container rootfs
    #[arg(long, default_value = "rootfs")]
    pub rootfs: String,

    /// Output file path
    #[arg(short, long, default_value = "config.json")]
    pub output: PathBuf,
}

/// Arguments for `coral create`.
#[derive(Debug, Args)]
pub struct CreateArgs {
    /// Path to the OCI bundle directory containing config.json
    #[arg(short, long, default_value = ".")]
    pub bundle: PathBuf,

    /// Unique container ID
    pub id: String,
}

/// Arguments for `coral start`.
#[derive(Debug, Args)]
pub struct StartArgs {
    /// Container ID
    pub id: String,
}

/// Arguments for `coral state`.
#[derive(Debug, Args)]
pub struct StateArgs {
    /// Container ID
    pub id: String,
}

/// Arguments for `coral kill`.
#[derive(Debug, Args)]
pub struct KillArgs {
    /// Container ID
    pub id: String,

    /// Signal name or number (e.g. SIGTERM, KILL, 9)
    #[arg(default_value = "SIGTERM")]
    pub signal: String,
}

/// Arguments for `coral delete`.
#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Force-kill before deleting if still running
    #[arg(short, long)]
    pub force: bool,

    /// Container ID
    pub id: String,
}

/// Arguments for `coral cni-config`.
#[derive(Debug, Args)]
pub struct CniConfigArgs {
    /// Optional output path (prints to stdout if omitted)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}
