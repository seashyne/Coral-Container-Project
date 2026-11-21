//! WebAssembly (WASI Preview 1) container and playground execution hooks.

use crate::service::CoralDaemonService;
use crate::images;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use coral_wasm::{WasmRunOptions, find_wasm_module_in_rootfs, run_wasm_bytes, run_wasm_file};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tonic::Status;
use tracing::info;

impl CoralDaemonService {
    /// Checks if container `id` targets the `coral-wasm` runtime (explicit `coral.runtime = "wasm"`
    /// annotation or an OCI Wasm image rootfs containing `module.wasm` / `*.wat`).
    pub(crate) fn is_wasm_container(
        annotations: &HashMap<String, String>,
        rootfs_path: &std::path::Path,
    ) -> bool {
        if annotations
            .get("coral.runtime")
            .is_some_and(|v| v.eq_ignore_ascii_case("wasm"))
        {
            return true;
        }
        find_wasm_module_in_rootfs(rootfs_path).is_some()
    }

    /// Launches a WebAssembly CRI container asynchronously without spawning any Linux rootfs/fork.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_wasm_container(
        &self,
        cid: String,
        rootfs_path: PathBuf,
        log_path: PathBuf,
        args: Vec<String>,
        envs: Vec<String>,
        fuel_limit: u64,
        memory_limit_mb: usize,
    ) {
        let store = self.store.clone();
        let state_dir = self.cfg.state_dir.clone();

        tokio::spawn(async move {
            let module_path = find_wasm_module_in_rootfs(&rootfs_path);
            let run_outcome = tokio::task::spawn_blocking(move || {
                let Some(path) = module_path else {
                    return Err(anyhow::anyhow!(
                        "no .wasm or .wat module found in image rootfs '{}'",
                        rootfs_path.display()
                    ));
                };
                let mut argv = vec![
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "module.wasm".into()),
                ];
                argv.extend(args);
                run_wasm_file(
                    &path,
                    &WasmRunOptions {
                        args: argv,
                        envs,
                        stdin: Vec::new(),
                        fuel_limit,
                        memory_limit_mb,
                        passthrough_stdio: false,
                    },
                )
            })
            .await;

            let finished_at = Utc::now().timestamp_nanos_opt().unwrap_or(0);
            let mut guard = store.write().await;
            if let Some(ctr) = guard.containers.get_mut(&cid) {
                match run_outcome {
                    Ok(Ok(out)) => {
                        let mut combined = out.stdout;
                        if !out.stderr.is_empty() {
                            combined.push_str(&out.stderr);
                        }
                        if let Some(trap) = &out.trapped {
                            combined.push_str(&format!("\n[wasm trap] {trap}\n"));
                        }
                        let _ = fs::write(&log_path, combined.as_bytes());
                        ctr.exit_code = out.exit_code;
                        ctr.finished_at = finished_at;
                        ctr.state = CriContainerState::ContainerExited;
                        ctr.reason = format!(
                            "WasmCompleted ({}us, {} fuel)",
                            out.duration_usec, out.fuel_consumed
                        );
                        ctr.message = out.trapped.unwrap_or_default();
                        info!(
                            container_id = %cid,
                            exit_code = out.exit_code,
                            duration_usec = out.duration_usec,
                            fuel = out.fuel_consumed,
                            "Wasm container completed"
                        );
                    }
                    Ok(Err(e)) => {
                        let msg = format!("{e:#}");
                        let _ = fs::write(&log_path, format!("wasm error: {msg}\n"));
                        ctr.exit_code = 1;
                        ctr.finished_at = finished_at;
                        ctr.state = CriContainerState::ContainerExited;
                        ctr.reason = "WasmError".into();
                        ctr.message = msg;
                    }
                    Err(e) => {
                        ctr.exit_code = 1;
                        ctr.finished_at = finished_at;
                        ctr.state = CriContainerState::ContainerExited;
                        ctr.reason = "WasmJoinError".into();
                        ctr.message = e.to_string();
                    }
                }
                crate::service::persist::save_persisted_store(&state_dir, &guard);
            }
        });
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_wasm_run(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let source_wat = annotations.get("coral.wasm.source").cloned();
        let image_ref = annotations
            .get("coral.wasm.image")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let fuel_limit = annotations
            .get("coral.wasm.fuel")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(50_000_000);
        let memory_limit_mb = annotations
            .get("coral.wasm.memory_mb")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(64);
        let args: Vec<String> = annotations
            .get("coral.wasm.args")
            .map(|raw| raw.split_whitespace().map(ToOwned::to_owned).collect())
            .unwrap_or_default();
        let envs: Vec<String> = annotations
            .get("coral.wasm.envs")
            .map(|raw| {
                raw.lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let rootfs_opt = if let Some(img) = image_ref {
            let store = self.store.read().await;
            images::find_image(&store.images, &img).map(|i| PathBuf::from(&i.rootfs_path))
        } else {
            None
        };

        let out = tokio::task::spawn_blocking(move || {
            let opts = WasmRunOptions {
                args,
                envs,
                stdin: Vec::new(),
                fuel_limit,
                memory_limit_mb,
                passthrough_stdio: false,
            };
            if let Some(src) = source_wat
                && !src.trim().is_empty()
            {
                return run_wasm_bytes(src.as_bytes(), &opts);
            }
            if let Some(rootfs) = rootfs_opt
                && let Some(mod_path) = find_wasm_module_in_rootfs(&rootfs)
            {
                return run_wasm_file(&mod_path, &opts);
            }
            Err(anyhow::anyhow!(
                "provide either coral.wasm.source (WAT text) or a Wasm image in coral.wasm.image"
            ))
        })
        .await
        .map_err(|e| Status::internal(format!("join wasm_run: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("wasm execution failed: {e:#}")))?;

        serde_json::to_string(&out)
            .map_err(|e| Status::internal(format!("serialize wasm output: {e}")))
    }
}
