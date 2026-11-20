//! Microsecond-startup WebAssembly (`wasm` / `wat` + WASI Preview 1) runtime for Coral.
//! Runs OCI `.wasm` workloads directly with deterministic CPU fuel metering and linear
//! memory caps — zero Linux rootfs required.

pub mod http;
pub mod wasi;

pub use http::{
    WasmHttpHandler, WasmHttpOptions, WasmHttpRequest, WasmHttpResponse, serve_wasm_http,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use wasi::{WasiContext, register_wasi_preview1};
use wasmi::{Config, Engine, Linker, Module, Store, StoreLimitsBuilder};

/// Execution parameters for running a WebAssembly workload.
#[derive(Debug, Clone)]
pub struct WasmRunOptions {
    /// Command-line arguments passed via WASI `args_get` (argv[0] defaults to `"module.wasm"`).
    pub args: Vec<String>,
    /// Environment variables (`KEY=VALUE`) passed via WASI `environ_get`.
    pub envs: Vec<String>,
    /// Optional STDIN bytes fed to WASI `fd_read(0)`.
    pub stdin: Vec<u8>,
    /// Deterministic instruction fuel budget (0 = unlimited, default 50_000_000).
    pub fuel_limit: u64,
    /// Linear memory limit in MB (default 64 MB).
    pub memory_limit_mb: usize,
    /// If true, also streams WASI stdout/stderr live to host process stdio.
    pub passthrough_stdio: bool,
}

impl Default for WasmRunOptions {
    fn default() -> Self {
        Self {
            args: vec!["module.wasm".into()],
            envs: Vec::new(),
            stdin: Vec::new(),
            fuel_limit: 50_000_000,
            memory_limit_mb: 64,
            passthrough_stdio: false,
        }
    }
}

/// Execution telemetry & captured output from a WebAssembly workload run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasmRunOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub fuel_consumed: u64,
    pub duration_usec: u64,
    pub entrypoint: String,
    pub trapped: Option<String>,
}

/// Detects whether a Coral image rootfs directory is a Wasm OCI bundle (contains `module.wasm`,
/// `module.wat`, or a single `.wasm`/`.wat` file without `/bin/sh`).
pub fn find_wasm_module_in_rootfs(rootfs: &Path) -> Option<PathBuf> {
    for candidate in ["module.wasm", "app.wasm", "main.wasm", "module.wat", "app.wat"] {
        let p = rootfs.join(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    if rootfs.join("bin/sh").exists() {
        return None;
    }
    if let Ok(rd) = fs::read_dir(rootfs) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_file()
                && let Some(ext) = p.extension().and_then(|e| e.to_str())
                && (ext.eq_ignore_ascii_case("wasm") || ext.eq_ignore_ascii_case("wat"))
            {
                return Some(p);
            }
        }
    }
    None
}

/// Loads a `.wasm` binary or `.wat` text file from disk and runs it in `coral-wasm`.
pub fn run_wasm_file(path: &Path, opts: &WasmRunOptions) -> Result<WasmRunOutput> {
    let bytes = fs::read(path)
        .with_context(|| format!("read WebAssembly module '{}'", path.display()))?;
    run_wasm_bytes(&bytes, opts)
}

/// Compiles and executes WebAssembly binary (`.wasm`) or WebAssembly Text (`.wat`) bytes
/// under strict fuel & memory caps with WASI Preview 1 support.
pub fn run_wasm_bytes(source: &[u8], opts: &WasmRunOptions) -> Result<WasmRunOutput> {
    let start = Instant::now();

    let mut cfg = Config::default();
    if opts.fuel_limit > 0 {
        cfg.consume_fuel(true);
    }
    let engine = Engine::new(&cfg);
    let module = Module::new(&engine, source).context("compile/validate WebAssembly module")?;

    let mem_bytes = opts.memory_limit_mb.max(1).saturating_mul(1024 * 1024);
    let limits = StoreLimitsBuilder::new()
        .memory_size(mem_bytes)
        .instances(4)
        .tables(8)
        .build();

    let ctx = WasiContext {
        args: if opts.args.is_empty() {
            vec!["module.wasm".into()]
        } else {
            opts.args.clone()
        },
        envs: opts.envs.clone(),
        stdin: VecDeque::from(opts.stdin.clone()),
        stdout: Vec::new(),
        stderr: Vec::new(),
        passthrough_stdio: opts.passthrough_stdio,
        exit_code: None,
        limits,
    };

    let mut store = Store::new(&engine, ctx);
    store.limiter(|s| &mut s.limits);
    if opts.fuel_limit > 0 {
        store.set_fuel(opts.fuel_limit).context("set Wasm CPU fuel")?;
    }

    let mut linker = Linker::<WasiContext>::new(&engine);
    register_wasi_preview1(&mut linker).context("register WASI Preview 1 host imports")?;

    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .context("instantiate WebAssembly module")?;

    // Resolve standard WASI `_start` (or fallback `main` / `run`).
    let (entry_name, call_res) = if let Ok(func) = instance.get_typed_func::<(), ()>(&store, "_start") {
        ("_start".to_string(), func.call(&mut store, ()).map(|()| 0))
    } else if let Ok(func) = instance.get_typed_func::<(), i32>(&store, "_start") {
        ("_start".to_string(), func.call(&mut store, ()))
    } else if let Ok(func) = instance.get_typed_func::<(), ()>(&store, "main") {
        ("main".to_string(), func.call(&mut store, ()).map(|()| 0))
    } else if let Ok(func) = instance.get_typed_func::<(), i32>(&store, "main") {
        ("main".to_string(), func.call(&mut store, ()))
    } else {
        bail!(
            "WebAssembly module exports neither `_start` (WASI) nor `main` entrypoint"
        );
    };

    let fuel_consumed = if opts.fuel_limit > 0 {
        let remaining = store.get_fuel().unwrap_or(0);
        opts.fuel_limit.saturating_sub(remaining)
    } else {
        0
    };
    let duration_usec = start.elapsed().as_micros() as u64;

    let mut exit_code = store.data().exit_code.unwrap_or(0);
    let mut trapped = None;
    match call_res {
        Ok(code) => {
            if store.data().exit_code.is_none() {
                exit_code = code;
            }
        }
        Err(err) => {
            if let Some(code) = err.i32_exit_status().or(store.data().exit_code) {
                exit_code = code;
            } else {
                exit_code = 1;
                trapped = Some(err.to_string());
            }
        }
    }

    let stdout = String::from_utf8_lossy(&store.data().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&store.data().stderr).into_owned();

    Ok(WasmRunOutput {
        exit_code,
        stdout,
        stderr,
        fuel_consumed,
        duration_usec,
        entrypoint: entry_name,
        trapped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executes_wasi_preview1_stdout_and_custom_exit_code_in_microseconds() {
        let wat = r#"
        (module
          (import "wasi_snapshot_preview1" "fd_write"
            (func $fd_write (param i32 i32 i32 i32) (result i32)))
          (import "wasi_snapshot_preview1" "proc_exit"
            (func $proc_exit (param i32)))
          (memory (export "memory") 1)
          ;; iovec at offset 0: ptr=16, len=23
          (data (i32.const 0) "\10\00\00\00\17\00\00\00")
          (data (i32.const 16) "Hello from Coral Wasm!\n")
          (func (export "_start")
            (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 64)))
            (call $proc_exit (i32.const 7))
          )
        )
        "#;
        let out = run_wasm_bytes(wat.as_bytes(), &WasmRunOptions::default()).unwrap();
        assert_eq!(out.exit_code, 7);
        assert_eq!(out.stdout, "Hello from Coral Wasm!\n");
        assert!(out.trapped.is_none());
        assert!(out.fuel_consumed > 0);
    }

    #[test]
    fn stops_infinite_loop_deterministically_with_fuel_limit() {
        let wat = r#"
        (module
          (func (export "_start")
            (loop $inf
              br $inf
            )
          )
        )
        "#;
        let opts = WasmRunOptions {
            fuel_limit: 5_000,
            ..WasmRunOptions::default()
        };
        let out = run_wasm_bytes(wat.as_bytes(), &opts).unwrap();
        assert_ne!(out.exit_code, 0);
        assert!(out.trapped.as_deref().unwrap_or("").contains("fuel"));
    }

    #[test]
    fn enforces_linear_memory_ceiling() {
        let wat = r#"
        (module
          (memory (export "memory") 1)
          (func (export "main") (result i32)
            ;; Try to grow by 100 pages (6.4 MB) under a 1 MB ceiling -> must return -1!
            (memory.grow (i32.const 100))
          )
        )
        "#;
        let opts = WasmRunOptions {
            memory_limit_mb: 1,
            ..WasmRunOptions::default()
        };
        let out = run_wasm_bytes(wat.as_bytes(), &opts).unwrap();
        assert_eq!(out.exit_code, -1);
    }
}
