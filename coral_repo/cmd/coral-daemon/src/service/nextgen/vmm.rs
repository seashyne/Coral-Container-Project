//! Native Linux `/dev/kvm` MicroVM hardware probe and boot hooks.

use crate::service::CoralDaemonService;
use crate::images;
use chrono::Utc;
use coral_vmm::{
    InitramfsOptions, KvmCapabilities, KvmRunResult, boot_linux_microvm_with_devices,
    build_initramfs_with_options, default_kernel_path, default_modules_dir, probe_kvm,
    run_kvm_hardware_selftest, setup_alpine_microvm_kernel,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tonic::Status;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VmmProbePayload {
    pub capabilities: KvmCapabilities,
    pub selftest: Option<KvmRunResult>,
}

impl CoralDaemonService {
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
