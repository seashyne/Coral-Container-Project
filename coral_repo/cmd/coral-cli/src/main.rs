//! Entry point for the `coral` CLI: parses arguments and dispatches to the command handlers.

mod args;
mod catalog;
mod cp;
mod daemon_client;
mod dispatch;
mod local_run;
mod nextgen;
mod port_proxy;
mod progress_bar;
mod run_resolve;
mod units;

use args::Cli;
use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match dispatch::dispatch(cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("coral error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
