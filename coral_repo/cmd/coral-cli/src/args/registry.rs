//! OCI Registry authentication subcommands (`login`, `logout`).

use clap::Args;

/// Arguments for `coral login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Registry host (defaults to `docker.io`)
    #[arg(default_value = "docker.io")]
    pub registry: String,

    /// Registry username
    #[arg(short = 'u', long)]
    pub username: String,

    /// Password or personal access token (or pass `--password-stdin`)
    #[arg(short = 'p', long)]
    pub password: Option<String>,

    /// Read password / token from STDIN
    #[arg(long)]
    pub password_stdin: bool,
}

/// Arguments for `coral logout`.
#[derive(Debug, Args)]
pub struct LogoutArgs {
    /// Registry host (defaults to `docker.io`)
    #[arg(default_value = "docker.io")]
    pub registry: String,
}
