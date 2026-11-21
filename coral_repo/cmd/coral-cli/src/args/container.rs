//! `coral` container lifecycle subcommands (`diff`, `commit`, `inspect`, `cp`, `logs`,
//! `exec`, `stop`, `rm`, `stats`).

use clap::Args;

/// Arguments for `coral diff`.
#[derive(Debug, Args)]
pub struct DiffArgs {
    /// Container ID
    pub id: String,

    /// Inspect local container state directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral commit`.
#[derive(Debug, Args)]
pub struct CommitArgs {
    /// Source container ID
    pub container: String,

    /// Target image repository:tag (e.g. `my-custom-alpine:v1`)
    pub image: String,

    /// Commit directly into local `--images-dir` without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral inspect`.
#[derive(Debug, Args)]
pub struct InspectArgs {
    /// Container ID
    pub id: String,

    /// Read local OCI state & config.json directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral cp`.
#[derive(Debug, Args)]
pub struct CpArgs {
    /// Source path (`HOST_PATH` or `CONTAINER_ID:/path`)
    pub src: String,

    /// Destination path (`CONTAINER_ID:/path` or `HOST_PATH`)
    pub dst: String,
}

/// Arguments for `coral logs`.
#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Container ID
    pub id: String,

    /// Include container state, exit code, health, restart count, and timestamps header
    #[arg(short, long)]
    pub status: bool,

    /// Stream live log output until the container exits (like `docker logs -f`)
    #[arg(short = 'f', long)]
    pub follow: bool,

    /// Number of trailing lines to show from the end of the logs
    #[arg(long)]
    pub tail: Option<usize>,
}

/// Arguments for `coral exec`.
#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Container ID
    pub id: String,

    /// Enter a local OCI container's live namespaces directly via `setns()` (no daemon)
    #[arg(long)]
    pub local: bool,

    /// Keep STDIN open for interactive `--local` exec (pair with `-t`)
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY for interactive `--local` exec (pair with `-i`)
    #[arg(short = 't', long)]
    pub tty: bool,

    /// Additional environment variables (`KEY=VALUE`) for `--local` exec
    #[arg(short = 'e', long = "env")]
    pub envs: Vec<String>,

    /// Working directory override for `--local` exec
    #[arg(short = 'w', long)]
    pub workdir: Option<String>,

    /// Command timeout in seconds (daemon mode)
    #[arg(long, default_value_t = 30)]
    pub timeout: i64,

    /// Command and arguments to execute
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
}

/// Arguments for `coral stop`.
#[derive(Debug, Args)]
pub struct StopArgs {
    /// Container ID
    pub id: String,

    /// Stop timeout in seconds
    #[arg(short, long, default_value_t = 10)]
    pub timeout: i64,
}

/// Arguments for `coral rm`.
#[derive(Debug, Args)]
pub struct RmArgs {
    /// Container ID
    pub id: String,
}

/// Arguments for `coral stats`.
#[derive(Debug, Args)]
pub struct StatsArgs {
    /// Container ID
    pub id: String,

    /// Continuously watch live CPU, Memory, PIDs, and Health metrics
    #[arg(short = 'w', long)]
    pub watch: bool,

    /// Polling interval in milliseconds when `--watch` is enabled
    #[arg(long, default_value_t = 1000)]
    pub interval: u64,
}
