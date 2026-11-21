//! `coral wasm` — WebAssembly (WASI Preview 1) workload commands.

use crate::catalog;
use anyhow::{Context, Result, bail};
use coral_daemon::images;
use coral_wasm::{
    WasmHttpHandler, WasmHttpOptions, WasmRunOptions, find_wasm_module_in_rootfs, run_wasm_file,
    serve_wasm_http,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

pub fn cmd_wasm_run(
    images_dir: &Path,
    target: &str,
    fuel: u64,
    memory_mb: usize,
    envs: Vec<String>,
    args: Vec<String>,
    as_json: bool,
) -> Result<ExitCode> {
    let module_path = resolve_wasm_target(images_dir, target)?;
    let mut argv = vec![
        module_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "module.wasm".into()),
    ];
    argv.extend(args);

    let out = run_wasm_file(
        &module_path,
        &WasmRunOptions {
            args: argv,
            envs,
            stdin: Vec::new(),
            fuel_limit: fuel,
            memory_limit_mb: memory_mb,
            passthrough_stdio: !as_json,
        },
    )?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else if let Some(trap) = &out.trapped {
        eprintln!("coral wasm trap: {trap}");
    }

    let code = u8::try_from(out.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

pub fn cmd_wasm_pack(images_dir: &Path, file: &Path, tag: &str) -> Result<()> {
    if !file.is_file() {
        bail!("WebAssembly source file '{}' not found", file.display());
    }
    let bytes = fs::read(file).with_context(|| format!("read '{}'", file.display()))?;
    // Validate Wasm/WAT syntax immediately before packing!
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wasm")
        .to_ascii_lowercase();
    let dest_name = if ext == "wat" {
        "module.wat"
    } else {
        "module.wasm"
    };

    let slug: String = tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let rootfs = images_dir.join(format!("wasm-{slug}")).join("rootfs");
    fs::create_dir_all(&rootfs)?;
    fs::write(rootfs.join(dest_name), &bytes)?;

    let abs_rootfs = fs::canonicalize(&rootfs).unwrap_or(rootfs);
    let size = images::dir_size(&abs_rootfs);
    let id = format!(
        "sha256:wasm-{}-{}",
        slug,
        chrono::Utc::now().timestamp_millis()
    );
    let stored = images::StoredImage {
        id: id.clone(),
        repo_tags: vec![tag.to_string()],
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    };

    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, stored);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    images::save_index(images_dir, &index)?;
    println!(
        "Packed Wasm OCI image '{}'\n  ID:     {}\n  Size:   {}\n  Module: {}",
        tag,
        id,
        catalog::format_bytes(size),
        abs_rootfs.join(dest_name).display()
    );
    Ok(())
}

pub fn resolve_wasm_target(images_dir: &Path, target: &str) -> Result<PathBuf> {
    let p = Path::new(target);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    if p.is_dir()
        && let Some(m) = find_wasm_module_in_rootfs(p)
    {
        return Ok(m);
    }
    let index = images::load_index(images_dir);
    if let Some(img) = images::find_image(&index, target)
        && let Some(m) = find_wasm_module_in_rootfs(Path::new(&img.rootfs_path))
    {
        return Ok(m);
    }
    bail!(
        "WebAssembly module or Wasm OCI image '{target}' not found (pack one with `coral wasm pack <file.wasm> -t <tag>`)"
    )
}

pub async fn cmd_wasm_serve(
    images_dir: &Path,
    target: &str,
    listen: &str,
    fuel: u64,
    memory_mb: usize,
    envs: Vec<String>,
) -> Result<()> {
    let module_path = resolve_wasm_target(images_dir, target)?;
    let bytes = fs::read(&module_path)
        .with_context(|| format!("read Wasm module '{}'", module_path.display()))?;
    let handler = Arc::new(WasmHttpHandler::from_bytes(
        &bytes,
        WasmHttpOptions {
            fuel_limit: fuel,
            memory_limit_mb: memory_mb,
            extra_envs: envs,
        },
    )?);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind Wasm HTTP listener on {listen}"))?;
    let local = listener.local_addr()?;
    println!(
        "Coral Wasm HTTP microservice listening on http://{local}\n  Module:    {}\n  Fuel/req:  {}\n  Mem/req:   {} MB\nPress Ctrl+C to stop.",
        module_path.display(),
        fuel,
        memory_mb
    );
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = tx.send(());
    });
    serve_wasm_http(listener, handler, rx).await
}
