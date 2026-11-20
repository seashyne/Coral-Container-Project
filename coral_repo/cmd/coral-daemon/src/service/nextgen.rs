//! Coral v2.0 Next-Gen runtime hooks in `CoralDaemonService`:
//! - Direct microsecond WebAssembly (WASI Preview 1) container & playground execution (`coral-wasm`)
//! - Native Linux `/dev/kvm` MicroVM hardware probe & self-test (`coral-vmm`)
//! - AI Agent / Untrusted Code hardened sandbox runner (`coral-vmm::sandbox`)

use super::CoralDaemonService;
use crate::images;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use coral_vmm::{
    InitramfsOptions, KvmCapabilities, KvmRunResult, SandboxPolicy, boot_linux_microvm_with_devices,
    build_initramfs_with_options, default_kernel_path, default_modules_dir, probe_kvm,
    run_ai_sandbox, run_kvm_hardware_selftest, setup_alpine_microvm_kernel,
};
use coral_wasm::{WasmRunOptions, find_wasm_module_in_rootfs, run_wasm_bytes, run_wasm_file};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tonic::Status;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmmProbePayload {
    pub capabilities: KvmCapabilities,
    pub selftest: Option<KvmRunResult>,
}

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
                super::persist::save_persisted_store(&state_dir, &guard);
            }
        });
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_vmm_probe(&self) -> Result<String, Status> {
        let payload = tokio::task::spawn_blocking(|| {
            let capabilities = probe_kvm();
            let selftest = if capabilities.available {
                run_kvm_hardware_selftest().ok()
            } else {
                None
            };
            VmmProbePayload {
                capabilities,
                selftest,
            }
        })
        .await
        .map_err(|e| Status::internal(format!("join vmm_probe: {e}")))?;
        serde_json::to_string(&payload)
            .map_err(|e| Status::internal(format!("serialize vmm_probe: {e}")))
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

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_sandbox_run(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image = annotations
            .get("coral.sandbox.image")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "alpine:latest".into());
        let cmd_str = annotations
            .get("coral.sandbox.cmd")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Status::invalid_argument("coral.sandbox.cmd is required"))?;
        let timeout_secs = annotations
            .get("coral.sandbox.timeout")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10)
            .clamp(1, 300);
        let memory_mb = annotations
            .get("coral.sandbox.memory_mb")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(64);
        let cpu_percent = annotations
            .get("coral.sandbox.cpu_percent")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(50);
        let pids_limit = annotations
            .get("coral.sandbox.pids_limit")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(32);
        let allow_network = annotations
            .get("coral.sandbox.allow_network")
            .is_some_and(|v| v == "true" || v == "1");
        let kernel_path = annotations
            .get("coral.sandbox.kernel")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let rootfs_override = {
            let store = self.store.read().await;
            images::find_image(&store.images, &image).map(|i| PathBuf::from(&i.rootfs_path))
        };
        let target_image = rootfs_override
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(image);

        let policy = SandboxPolicy {
            coral_bin: self.cfg.coral_bin.clone(),
            images_dir: self.images_dir.clone(),
            state_root: self.runtime_root(),
            image: target_image,
            cmd: vec!["/bin/sh".into(), "-c".into(), cmd_str],
            envs: Vec::new(),
            memory_mb,
            cpu_percent,
            pids_limit,
            timeout_secs,
            allow_network,
            kernel_path,
            rootfs_override,
        };

        let report = tokio::task::spawn_blocking(move || run_ai_sandbox(&policy))
            .await
            .map_err(|e| Status::internal(format!("join sandbox_run: {e}")))?
            .map_err(|e| Status::internal(format!("sandbox execution failed: {e:#}")))?;

        serde_json::to_string(&report)
            .map_err(|e| Status::internal(format!("serialize sandbox report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_sign(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let custom_priv = annotations
            .get("coral.image.key")
            .map(PathBuf::from);
        let custom_pub = annotations
            .get("coral.image.pub_key")
            .map(PathBuf::from);
        let images_dir = self.images_dir.clone();
        let out = tokio::task::spawn_blocking(move || {
            let (def_priv, def_pub) = images::security::default_key_paths();
            let priv_key = custom_priv.unwrap_or(def_priv);
            let pub_key = custom_pub.unwrap_or(def_pub);
            if !priv_key.exists() {
                images::security::generate_keypair(&priv_key, &pub_key, false)?;
            }
            images::security::sign_image(&images_dir, &image_ref, &priv_key)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_sign: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("sign image failed: {e:#}")))?;
        serde_json::to_string(&out)
            .map_err(|e| Status::internal(format!("serialize sign report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_verify(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let custom_pub = annotations
            .get("coral.image.pub_key")
            .map(PathBuf::from);
        let images_dir = self.images_dir.clone();
        let out = tokio::task::spawn_blocking(move || {
            let (_def_priv, def_pub) = images::security::default_key_paths();
            let pub_key = custom_pub.unwrap_or(def_pub);
            images::security::verify_image(&images_dir, &image_ref, &pub_key)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_verify: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("verify image failed: {e:#}")))?;
        serde_json::to_string(&out)
            .map_err(|e| Status::internal(format!("serialize verify report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_sbom(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let images_dir = self.images_dir.clone();
        let sbom = tokio::task::spawn_blocking(move || {
            images::security::generate_spdx_sbom(&images_dir, &image_ref)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_sbom: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("generate SBOM failed: {e:#}")))?;
        serde_json::to_string(&sbom)
            .map_err(|e| Status::internal(format!("serialize SBOM: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_vm_setup_kernel(&self) -> Result<String, Status> {
        let report = tokio::task::spawn_blocking(|| {
            setup_alpine_microvm_kernel(&default_kernel_path())
        })
        .await
        .map_err(|e| Status::internal(format!("join vm_setup_kernel: {e}")))?
        .map_err(|e| Status::internal(format!("setup Alpine kernel failed: {e:#}")))?;
        serde_json::to_string(&report)
            .map_err(|e| Status::internal(format!("serialize kernel report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_vm_boot(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image = annotations
            .get("coral.vm.image")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "alpine:3.20".into());
        let cmd_str = annotations
            .get("coral.vm.cmd")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "uname -a".into());
        let memory_mb = annotations
            .get("coral.vm.memory_mb")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(256)
            .clamp(128, 1024);
        let timeout_secs = annotations
            .get("coral.vm.timeout")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(15)
            .clamp(1, 120);
        let disk_raw = annotations
            .get("coral.vm.disk")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let enable_net = annotations
            .get("coral.vm.net")
            .is_some_and(|v| v == "true" || v == "1");

        let rootfs = {
            let store = self.store.read().await;
            images::find_image(&store.images, &image)
                .map(|i| PathBuf::from(&i.rootfs_path))
                .ok_or_else(|| Status::not_found(format!("image '{image}' not found in daemon")))?
        };
        let state_dir = self.runtime_root();
        let report = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let kernel_path = default_kernel_path();
            if !kernel_path.is_file() {
                anyhow::bail!(
                    "guest Linux kernel '{}' not found — click 'Setup Alpine 6.6 Kernel' first",
                    kernel_path.display()
                );
            }
            let (disk_path, disk_ro) = match disk_raw {
                Some(spec) => {
                    if let Some(p) = spec.strip_suffix(":ro") {
                        (Some(PathBuf::from(p)), true)
                    } else {
                        (Some(PathBuf::from(spec)), false)
                    }
                }
                None => (None, false),
            };
            let mod_dir = default_modules_dir(&kernel_path);
            let initrd_path = state_dir.join(format!("desktop-vm-initrd-{}.cpio.gz", Utc::now().timestamp_millis()));
            build_initramfs_with_options(
                &rootfs,
                &["/bin/sh".into(), "-c".into(), cmd_str],
                &initrd_path,
                &InitramfsOptions {
                    modules_dir: mod_dir.is_dir().then_some(mod_dir.as_path()),
                    enable_blk: disk_path.is_some(),
                    enable_net,
                    disk_mount: disk_path.as_ref().map(|_| "/mnt/data"),
                },
            )?;
            let res = boot_linux_microvm_with_devices(
                &kernel_path,
                &initrd_path,
                memory_mb,
                timeout_secs,
                None,
                disk_path.as_deref(),
                disk_ro,
                enable_net,
                &[],
            );
            let _ = fs::remove_file(&initrd_path);
            res
        })
        .await
        .map_err(|e| Status::internal(format!("join vm_boot: {e}")))?
        .map_err(|e| Status::failed_precondition(format!("MicroVM boot failed: {e:#}")))?;

        serde_json::to_string(&report)
            .map_err(|e| Status::internal(format!("serialize vm_boot report: {e}")))
    }
}

#[allow(clippy::result_large_err)]
fn require_image_ref(annotations: &HashMap<String, String>) -> Result<String, Status> {
    annotations
        .get("coral.image.ref")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Status::invalid_argument("coral.image.ref is required"))
}
