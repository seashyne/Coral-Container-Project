//! `coral wasm` — WebAssembly (WASI Preview 1) workload subcommands.

use clap::{Args, Subcommand};
use std::path::PathBuf;

/// Arguments for `coral wasm`.
#[derive(Debug, Args)]
pub struct WasmArgs {
    #[command(subcommand)]
    pub action: WasmAction,
}

#[derive(Debug, Subcommand)]
pub enum WasmAction {
    /// Run a `.wasm` binary, `.wat` text file, or packed Wasm OCI image in microseconds
    Run {
        /// Path to `.wasm`/`.wat` file or installed Wasm image tag
        target: String,

        /// Deterministic CPU fuel budget (0 = unlimited)
        #[arg(long, default_value_t = 50_000_000)]
        fuel: u64,

        /// Linear memory ceiling in MB
        #[arg(short = 'm', long = "memory", default_value_t = 64)]
        memory_mb: usize,

        /// Environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Output structured JSON telemetry (exit code, fuel consumed, duration_usec)
        #[arg(long)]
        json: bool,

        /// Arguments passed to WASI `args_get` (pass after `--`)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Package a `.wasm` or `.wat` file as a runnable Coral OCI Wasm image
    Pack {
        /// Source `.wasm` or `.wat` file
        file: PathBuf,

        /// Target image repository:tag (e.g. `hello-wasm:v1`)
        #[arg(short = 't', long = "tag")]
        tag: String,
    },
    /// Serve a `.wasm`/`.wat` or Wasm OCI image as a per-request isolated WASI HTTP/1.1 microservice
    Serve {
        /// Path to `.wasm`/`.wat` file or installed Wasm image tag
        target: String,

        /// TCP address to listen on (e.g. `127.0.0.1:8080`)
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,

        /// Deterministic per-request CPU fuel budget (0 = unlimited)
        #[arg(long, default_value_t = 50_000_000)]
        fuel: u64,

        /// Per-request linear memory ceiling in MB
        #[arg(short = 'm', long = "memory", default_value_t = 32)]
        memory_mb: usize,

        /// Extra environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,
    },
}
