//! `coral compose` — multi-container stack subcommands.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Arguments for `coral compose`.
#[derive(Debug, Args)]
pub struct ComposeArgs {
    /// Path to compose YAML file (defaults to coral-compose.yml / docker-compose.yml)
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Override the Compose project name (defaults to `name:` in YAML or directory name)
    #[arg(short = 'p', long = "project-name")]
    pub project_name: Option<String>,

    /// Specify an alternate environment file for `${VAR}` variable interpolation
    #[arg(long = "env-file")]
    pub env_file: Option<PathBuf>,

    /// Enable services matching the given Compose profile, repeatable
    #[arg(long = "profile")]
    pub profiles: Vec<String>,

    #[command(subcommand)]
    pub action: ComposeAction,
}

#[derive(Debug, Subcommand)]
pub enum ComposeAction {
    /// Build (if needed), create, and start all (or selected) services in a Compose stack
    Up {
        /// Detached mode: start containers in coral-daemon and return immediately
        #[arg(short = 'd', long)]
        detach: bool,

        /// Force rebuilding images for services with a `build:` directive
        #[arg(long)]
        build: bool,

        /// Wait until services with a healthcheck become healthy before returning
        #[arg(long)]
        wait: bool,

        /// Do not start linked services listed under `depends_on`
        #[arg(long)]
        no_deps: bool,

        /// Scale a service to N instances (`--scale web=3`), repeatable
        #[arg(long = "scale")]
        scale: Vec<String>,

        /// Optional service names to start
        services: Vec<String>,
    },
    /// Stop and remove all containers and pods belonging to the Compose stack
    Down {
        /// Also delete named volumes declared in the Compose file
        #[arg(short = 'v', long)]
        volumes: bool,
    },
    /// List containers belonging to the Compose stack
    Ps {
        /// Show all containers (including stopped containers)
        #[arg(short = 'a', long)]
        all: bool,
    },
    /// Show aggregated logs from all services (or one service) in the Compose stack
    Logs {
        /// Follow live log output
        #[arg(short = 'f', long)]
        follow: bool,

        /// Number of lines to show from the end of each service's logs
        #[arg(long)]
        tail: Option<usize>,

        /// Optional service name filter
        service: Option<String>,
    },
    /// Build or rebuild services that define a `build:` directive
    Build {
        /// Do not use build layer cache
        #[arg(long)]
        no_cache: bool,

        /// Optional service names to build
        services: Vec<String>,
    },
    /// Start existing containers for Compose services
    Start {
        /// Optional service names to start
        services: Vec<String>,
    },
    /// Stop running Compose service containers without removing them
    Stop {
        /// Seconds to wait before killing containers
        #[arg(short = 't', long, default_value_t = 10)]
        timeout: i64,

        /// Optional service names to stop
        services: Vec<String>,
    },
    /// Restart Compose service containers
    Restart {
        /// Seconds to wait before killing containers
        #[arg(short = 't', long, default_value_t = 10)]
        timeout: i64,

        /// Optional service names to restart
        services: Vec<String>,
    },
    /// Execute a command inside a running Compose service container
    Exec {
        /// Keep STDIN open for interactive exec (pair with `-t`)
        #[arg(short = 'i', long)]
        interactive: bool,

        /// Allocate a pseudo-TTY for interactive exec (pair with `-i`)
        #[arg(short = 't', long)]
        tty: bool,

        /// Extra environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Working directory inside the container
        #[arg(short = 'w', long)]
        workdir: Option<String>,

        /// Sync command timeout in seconds
        #[arg(long, default_value_t = 30)]
        timeout: i64,

        /// Target Compose service name
        service: String,

        /// Command and arguments to run inside the service container
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },
    /// Watch build contexts and automatically rebuild & restart services on file changes
    Watch {
        /// Polling interval in milliseconds
        #[arg(long, default_value_t = 1500)]
        interval_ms: u64,

        /// Optional service names to watch
        services: Vec<String>,
    },
    /// Validate and render the resolved Compose configuration as JSON
    Config,
}
