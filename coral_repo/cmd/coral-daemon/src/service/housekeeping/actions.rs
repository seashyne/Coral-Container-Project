//! Dispatch of CRI annotation-based system actions.

use super::model::VolumeSummary;
use super::volumes::container_volume_names;
use crate::images;
use crate::service::CoralDaemonService;
use coral_api::ContainerState as CriContainerState;
use coral_runtime::rootfs::{
    default_volumes_dir, ensure_named_volume, remove_named_volume,
};
use std::collections::HashMap;
use tonic::Status;

impl CoralDaemonService {
    /// Dispatches CRI annotation-based system actions (`df`, `prune`, `volume_ls`, `volume_create`, `volume_rm`).
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_system_action(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let action = annotations
            .get("coral.system.action")
            .map(String::as_str)
            .unwrap_or("");
        match action {
            "df" => {
                let usage = self.compute_system_df().await;
                serde_json::to_string(&usage)
                    .map_err(|e| Status::internal(format!("serialize system df: {e}")))
            }
            "prune" => {
                let prune_all = annotations
                    .get("coral.system.prune_all")
                    .is_some_and(|v| v == "true" || v == "1");
                let prune_volumes = annotations
                    .get("coral.system.prune_volumes")
                    .is_some_and(|v| v == "true" || v == "1");
                let report = self.prune_system(prune_all, prune_volumes).await;
                serde_json::to_string(&report)
                    .map_err(|e| Status::internal(format!("serialize prune report: {e}")))
            }
            "volume_ls" => {
                let vols = self.list_volumes_summary().await?;
                serde_json::to_string(&vols)
                    .map_err(|e| Status::internal(format!("serialize volumes: {e}")))
            }
            "volume_create" => {
                let name = annotations
                    .get("coral.volume.name")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Status::invalid_argument("volume name is required"))?;
                let vol_root = default_volumes_dir();
                let mountpoint = ensure_named_volume(&vol_root, name)
                    .map_err(|e| Status::invalid_argument(format!("create volume '{name}': {e}")))?;
                let summary = VolumeSummary {
                    name: name.to_string(),
                    mountpoint: mountpoint.to_string_lossy().into_owned(),
                    size: images::dir_size(&mountpoint),
                    used_by: Vec::new(),
                };
                serde_json::to_string(&summary)
                    .map_err(|e| Status::internal(format!("serialize created volume: {e}")))
            }
            "volume_rm" => {
                let name = annotations
                    .get("coral.volume.name")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Status::invalid_argument("volume name is required"))?;
                let in_use_running: Vec<String> = {
                    let store = self.store.read().await;
                    store
                        .containers
                        .values()
                        .filter(|c| {
                            c.state == CriContainerState::ContainerRunning
                                && container_volume_names(c).contains(name)
                        })
                        .map(|c| c.id.clone())
                        .collect()
                };
                if !in_use_running.is_empty() {
                    return Err(Status::failed_precondition(format!(
                        "volume '{name}' is in use by running container(s): {}",
                        in_use_running.join(", ")
                    )));
                }
                let vol_root = default_volumes_dir();
                remove_named_volume(&vol_root, name)
                    .map_err(|e| Status::not_found(format!("remove volume '{name}': {e}")))?;
                Ok(name.to_string())
            }
            "login" => {
                let registry = annotations
                    .get("coral.auth.registry")
                    .map(String::as_str)
                    .unwrap_or("docker.io");
                let username = annotations
                    .get("coral.auth.username")
                    .map(String::as_str)
                    .unwrap_or("");
                let password = annotations
                    .get("coral.auth.password")
                    .map(String::as_str)
                    .unwrap_or("");
                let key = images::login_registry(registry, username, password)
                    .map_err(|e| Status::invalid_argument(format!("registry login failed: {e}")))?;
                Ok(key)
            }
            "logout" => {
                let registry = annotations
                    .get("coral.auth.registry")
                    .map(String::as_str)
                    .unwrap_or("docker.io");
                let removed = images::logout_registry(registry)
                    .map_err(|e| Status::internal(format!("registry logout failed: {e}")))?;
                Ok(if removed { "removed".into() } else { "not_logged_in".into() })
            }
            "push" => {
                let image_ref = annotations
                    .get("coral.image.ref")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Status::invalid_argument("image reference is required for push"))?;
                let stored = {
                    let store = self.store.read().await;
                    images::find_image(&store.images, image_ref).cloned()
                }
                .ok_or_else(|| {
                    Status::not_found(format!(
                        "image '{image_ref}' is not installed locally — tag or build it first"
                    ))
                })?;
                let pushed = images::push_image(image_ref, &stored)
                    .await
                    .map_err(|e| Status::internal(format!("push '{image_ref}': {e:#}")))?;
                serde_json::to_string(&pushed)
                    .map_err(|e| Status::internal(format!("serialize push result: {e}")))
            }
            "network_ls" => {
                let nets = coral_net::list_networks();
                serde_json::to_string(&nets)
                    .map_err(|e| Status::internal(format!("serialize networks: {e}")))
            }
            "network_create" => {
                let name = annotations
                    .get("coral.network.name")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Status::invalid_argument("network name is required"))?;
                let driver = annotations.get("coral.network.driver").map(String::as_str);
                let subnet = annotations.get("coral.network.subnet").map(String::as_str);
                let net = coral_net::create_network(name, driver, subnet)
                    .map_err(|e| Status::invalid_argument(format!("create network '{name}': {e}")))?;
                serde_json::to_string(&net)
                    .map_err(|e| Status::internal(format!("serialize network: {e}")))
            }
            "network_rm" => {
                let name = annotations
                    .get("coral.network.name")
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Status::invalid_argument("network name is required"))?;
                coral_net::remove_network(name)
                    .map_err(|e| Status::failed_precondition(format!("remove network '{name}': {e}")))?;
                Ok(name.to_string())
            }
            "vmm_probe" => self.handle_vmm_probe().await,
            "wasm_run" => self.handle_wasm_run(annotations).await,
            "sandbox_run" => self.handle_sandbox_run(annotations).await,
            "image_sign" => self.handle_image_sign(annotations).await,
            "image_verify" => self.handle_image_verify(annotations).await,
            "image_sbom" => self.handle_image_sbom(annotations).await,
            "image_scan" => self.handle_image_scan(annotations).await,
            "vm_setup_kernel" => self.handle_vm_setup_kernel().await,
            "vm_boot" => self.handle_vm_boot(annotations).await,
            "container_pause" => {
                let cid = annotations
                    .get("coral.container.id")
                    .map(String::as_str)
                    .unwrap_or("");
                self.pause_container(cid).await
            }
            "container_unpause" => {
                let cid = annotations
                    .get("coral.container.id")
                    .map(String::as_str)
                    .unwrap_or("");
                self.unpause_container(cid).await
            }
            "container_rename" => {
                let cid = annotations
                    .get("coral.container.id")
                    .map(String::as_str)
                    .unwrap_or("");
                let new_name = annotations
                    .get("coral.container.new_name")
                    .map(String::as_str)
                    .unwrap_or("");
                self.rename_container(cid, new_name).await
            }
            "container_update" => {
                let cid = annotations
                    .get("coral.container.id")
                    .map(String::as_str)
                    .unwrap_or("");
                let mem = annotations
                    .get("coral.update.memory_bytes")
                    .and_then(|v| v.parse::<i64>().ok());
                let quota = annotations
                    .get("coral.update.cpu_quota")
                    .and_then(|v| v.parse::<i64>().ok());
                let period = annotations
                    .get("coral.update.cpu_period")
                    .and_then(|v| v.parse::<i64>().ok());
                let pids = annotations
                    .get("coral.update.pids_limit")
                    .and_then(|v| v.parse::<i64>().ok());
                self.update_container_resources(cid, mem, quota, period, pids)
                    .await
            }
            "container_top" => {
                let cid = annotations
                    .get("coral.container.id")
                    .map(String::as_str)
                    .unwrap_or("");
                self.container_top(cid).await
            }
            "auto_gc" => {
                let report = self.run_auto_gc_once().await;
                serde_json::to_string(&report)
                    .map_err(|e| Status::internal(format!("serialize auto_gc report: {e}")))
            }
            other => Err(Status::invalid_argument(format!(
                "unknown coral.system.action '{other}'"
            ))),
        }
    }
}
