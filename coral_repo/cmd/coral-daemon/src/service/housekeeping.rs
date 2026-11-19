//! Daemon-level housekeeping (`system df`, `system prune`), remote/WSL2-compatible
//! Named Volume management (`volume ls/create/rm`), and Boot Auto-Resume for
//! `--restart always` / `--restart unless-stopped` containers.

use super::port_forward::spawn_port_forwarders;
use super::supervisor::{RestartPolicy, SupervisorLaunch, spawn_container_supervisor};
use super::{CoralDaemonService, DaemonStore};
use crate::images;
use chrono::Utc;
use coral_api::ContainerState as CriContainerState;
use coral_runtime::rootfs::{
    default_volumes_dir, ensure_named_volume, list_named_volumes, remove_named_volume,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use tokio::process::Command;
use tonic::Status;
use tracing::{info, warn};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDiskUsage {
    pub images_count: usize,
    pub images_bytes: u64,
    pub cache_entries: usize,
    pub cache_bytes: u64,
    pub containers_count: usize,
    pub containers_running: usize,
    pub containers_exited: usize,
    pub containers_bytes: u64,
    pub volumes_count: usize,
    pub volumes_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneReport {
    pub containers_removed: usize,
    pub pods_removed: usize,
    pub cache_entries_removed: usize,
    pub volumes_removed: usize,
    pub images_removed: usize,
    pub bytes_reclaimed: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSummary {
    pub name: String,
    pub mountpoint: String,
    pub size: u64,
    pub used_by: Vec<String>,
}

impl CoralDaemonService {
    /// Resumes containers configured with `--restart always` or `--restart unless-stopped`
    /// that were previously running (not stopped by user) before a daemon restart or reboot.
    pub(crate) fn spawn_boot_auto_resume(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let svc = self.clone();
        handle.spawn(async move {
            let candidates: Vec<(String, std::path::PathBuf, std::path::PathBuf, Option<String>, bool)> = {
                let mut store = svc.store.write().await;
                let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
                let mut list = Vec::new();
                for ctr in store.containers.values_mut() {
                    if ctr.stopped_by_user || ctr.started_at <= 0 {
                        continue;
                    }
                    if ctr.state == CriContainerState::ContainerRunning {
                        continue;
                    }
                    let policy = RestartPolicy::parse(&ctr.restart_policy);
                    if !matches!(policy, RestartPolicy::Always | RestartPolicy::UnlessStopped) {
                        continue;
                    }
                    ctr.state = CriContainerState::ContainerRunning;
                    ctr.started_at = now;
                    ctr.restart_count = ctr.restart_count.saturating_add(1);
                    ctr.reason = "BootAutoResume".into();
                    let net_mode = ctr
                        .config
                        .annotations
                        .get("coral.network")
                        .map(String::as_str)
                        .unwrap_or("host");
                    let is_bridge = !net_mode.eq_ignore_ascii_case("host")
                        && !net_mode.eq_ignore_ascii_case("none");
                    list.push((
                        ctr.id.clone(),
                        ctr.bundle_dir.clone(),
                        ctr.log_path.clone(),
                        ctr.config.annotations.get("coral.ports").cloned(),
                        is_bridge,
                    ));
                }
                if !list.is_empty() {
                    svc.persist_state(&store);
                }
                list
            };

            let runtime_root = svc.runtime_root();
            for (cid, bundle_dir, log_path, ports_spec, is_bridge) in candidates {
                if let Some(raw_ports) = ports_spec {
                    match spawn_port_forwarders(&cid, &raw_ports, is_bridge, &runtime_root).await {
                        Ok(handles) if !handles.is_empty() => {
                            svc.store
                                .write()
                                .await
                                .port_forwarders
                                .insert(cid.clone(), handles);
                        }
                        Ok(_) => {}
                        Err(e) => {
                            warn!(container_id = %cid, err = %e, "boot auto-resume port forwarder skipped");
                        }
                    }
                }
                info!(container_id = %cid, "auto-resuming container after daemon boot");
                let sup_handle = spawn_container_supervisor(SupervisorLaunch {
                    cid: cid.clone(),
                    bundle_dir,
                    log_path,
                    runtime_root: runtime_root.clone(),
                    state_dir: svc.cfg.state_dir.clone(),
                    coral_bin: svc.cfg.coral_bin.clone(),
                    store: svc.store.clone(),
                });
                if let Some(prev) = svc.store.write().await.supervisors.insert(cid, sup_handle) {
                    prev.abort();
                }
            }
        });
    }

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
            other => Err(Status::invalid_argument(format!(
                "unknown coral.system.action '{other}'"
            ))),
        }
    }

    async fn compute_system_df(&self) -> SystemDiskUsage {
        let store = self.store.read().await;

        let mut seen_rootfs = HashSet::new();
        let mut images_bytes = 0u64;
        for img in &store.images {
            if seen_rootfs.insert(img.rootfs_path.clone()) {
                images_bytes = images_bytes.saturating_add(img.size);
            }
        }
        let images_count = store.images.len();

        let cache_dir = self.cfg.state_dir.join("build-cache");
        let (cache_entries, cache_bytes) = count_and_size_subdirs(&cache_dir);

        let mut containers_running = 0usize;
        let mut containers_exited = 0usize;
        let mut containers_bytes = 0u64;
        for ctr in store.containers.values() {
            if ctr.state == CriContainerState::ContainerRunning {
                containers_running += 1;
            } else if ctr.state == CriContainerState::ContainerExited {
                containers_exited += 1;
            }
            containers_bytes = containers_bytes
                .saturating_add(images::dir_size(&ctr.bundle_dir))
                .saturating_add(
                    fs::metadata(&ctr.log_path).map(|m| m.len()).unwrap_or(0),
                );
        }
        let containers_count = store.containers.len();
        drop(store);

        let vol_root = default_volumes_dir();
        let vols = list_named_volumes(&vol_root).unwrap_or_default();
        let volumes_count = vols.len();
        let volumes_bytes = vols
            .iter()
            .fold(0u64, |acc, (_, p)| acc.saturating_add(images::dir_size(p)));

        let total_bytes = images_bytes
            .saturating_add(cache_bytes)
            .saturating_add(containers_bytes)
            .saturating_add(volumes_bytes);

        SystemDiskUsage {
            images_count,
            images_bytes,
            cache_entries,
            cache_bytes,
            containers_count,
            containers_running,
            containers_exited,
            containers_bytes,
            volumes_count,
            volumes_bytes,
            total_bytes,
        }
    }

    async fn prune_system(&self, prune_all: bool, prune_volumes: bool) -> PruneReport {
        let mut report = PruneReport::default();

        // 1. Remove all EXITED containers + their bundles, logs, and runtime state.
        let exited_items: Vec<(String, std::path::PathBuf, std::path::PathBuf)> = {
            let mut store = self.store.write().await;
            let exited_ids: Vec<String> = store
                .containers
                .values()
                .filter(|c| c.state == CriContainerState::ContainerExited)
                .map(|c| c.id.clone())
                .collect();
            let mut removed = Vec::with_capacity(exited_ids.len());
            for id in exited_ids {
                if let Some(handles) = store.port_forwarders.remove(&id) {
                    for h in handles {
                        h.abort();
                    }
                }
                if let Some(ctr) = store.containers.remove(&id) {
                    removed.push((id, ctr.bundle_dir, ctr.log_path));
                }
            }
            self.persist_state(&store);
            removed
        };

        let runtime_root = self.runtime_root();
        for (id, bundle_dir, log_path) in exited_items {
            let bytes = images::dir_size(&bundle_dir)
                .saturating_add(fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0));
            let _ = Command::new(&self.cfg.coral_bin)
                .arg("--root")
                .arg(&runtime_root)
                .arg("delete")
                .arg("--force")
                .arg(&id)
                .output()
                .await;
            let _ = images::remove_dir_force(&bundle_dir);
            let _ = fs::remove_file(&log_path);
            report.containers_removed += 1;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
        }

        // 2. Remove pod sandboxes that no longer own any containers.
        {
            let mut store = self.store.write().await;
            let active_pods: HashSet<String> = store
                .containers
                .values()
                .map(|c| c.pod_sandbox_id.clone())
                .collect();
            let dead_pods: Vec<String> = store
                .sandboxes
                .keys()
                .filter(|pid| !active_pods.contains(*pid))
                .cloned()
                .collect();
            for pid in dead_pods {
                store.sandboxes.remove(&pid);
                report.pods_removed += 1;
            }
            self.persist_state(&store);
        }

        // 3. Purge build step cache & leftover exec bundles.
        let cache_dir = self.cfg.state_dir.join("build-cache");
        let (c_entries, c_bytes) = count_and_size_subdirs(&cache_dir);
        if c_entries > 0 {
            let _ = images::remove_dir_force(&cache_dir);
            report.cache_entries_removed = c_entries;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(c_bytes);
        }
        let exec_bundles = self.cfg.state_dir.join("exec-bundles");
        if exec_bundles.is_dir() {
            let eb_bytes = images::dir_size(&exec_bundles);
            let _ = images::remove_dir_force(&exec_bundles);
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(eb_bytes);
        }

        // 4. Optional: remove unused named volumes.
        if prune_volumes {
            let used_vols = {
                let store = self.store.read().await;
                all_used_volumes(&store)
            };
            let vol_root = default_volumes_dir();
            for (vname, vpath) in list_named_volumes(&vol_root).unwrap_or_default() {
                if !used_vols.contains(&vname) {
                    let bytes = images::dir_size(&vpath);
                    if remove_named_volume(&vol_root, &vname).is_ok() {
                        report.volumes_removed += 1;
                        report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
                    }
                }
            }
        }

        // 5. Optional (`--all`): remove unused managed images.
        if prune_all {
            let mut store = self.store.write().await;
            let active_rootfs: HashSet<String> = store
                .containers
                .values()
                .map(|c| c.rootfs_path.to_string_lossy().into_owned())
                .collect();
            let mut kept = Vec::with_capacity(store.images.len());
            let mut removed_paths = Vec::new();
            for img in store.images.drain(..) {
                if img.managed && !active_rootfs.contains(&img.rootfs_path) {
                    report.images_removed += 1;
                    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(img.size);
                    removed_paths.push(img.rootfs_path);
                } else {
                    kept.push(img);
                }
            }
            store.images = kept;
            for path in removed_paths {
                self.cleanup_unused_managed_dir(&store.images, &path);
            }
            self.persist_images(&store.images);
        }

        info!(
            containers = report.containers_removed,
            pods = report.pods_removed,
            cache = report.cache_entries_removed,
            volumes = report.volumes_removed,
            images = report.images_removed,
            bytes = report.bytes_reclaimed,
            "completed coral system prune"
        );
        report
    }

    #[allow(clippy::result_large_err)]
    async fn list_volumes_summary(&self) -> Result<Vec<VolumeSummary>, Status> {
        let store = self.store.read().await;
        let mut usage_map: HashMap<String, Vec<String>> = HashMap::new();
        for ctr in store.containers.values() {
            let cname = ctr
                .config
                .metadata
                .as_ref()
                .map(|m| m.name.clone())
                .unwrap_or_else(|| ctr.id.clone());
            for vname in container_volume_names(ctr) {
                usage_map.entry(vname).or_default().push(cname.clone());
            }
        }
        drop(store);

        let vol_root = default_volumes_dir();
        let vols = list_named_volumes(&vol_root)
            .map_err(|e| Status::internal(format!("list volumes: {e}")))?;
        let mut out = Vec::with_capacity(vols.len());
        for (name, mountpoint) in vols {
            let mut used_by = usage_map.remove(&name).unwrap_or_default();
            used_by.sort();
            used_by.dedup();
            out.push(VolumeSummary {
                name,
                mountpoint: mountpoint.to_string_lossy().into_owned(),
                size: images::dir_size(&mountpoint),
                used_by,
            });
        }
        Ok(out)
    }
}

fn count_and_size_subdirs(dir: &Path) -> (usize, u64) {
    let Ok(rd) = fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut count = 0usize;
    let mut bytes = 0u64;
    for entry in rd.flatten() {
        count += 1;
        let p = entry.path();
        if p.is_dir() {
            bytes = bytes.saturating_add(images::dir_size(&p));
        } else if let Ok(meta) = entry.metadata() {
            bytes = bytes.saturating_add(meta.len());
        }
    }
    (count, bytes)
}

fn container_volume_names(ctr: &super::ManagedContainer) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(raw) = ctr.config.annotations.get("coral.mounts") else {
        return out;
    };
    for line in raw.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((src, _)) = line.split_once(':') {
            let s = src.trim();
            if !s.contains('/') && !s.is_empty() && s != "." && s != ".." {
                out.insert(s.to_string());
            }
        }
    }
    out
}

fn all_used_volumes(store: &DaemonStore) -> HashSet<String> {
    let mut used = HashSet::new();
    for ctr in store.containers.values() {
        used.extend(container_volume_names(ctr));
    }
    used
}
