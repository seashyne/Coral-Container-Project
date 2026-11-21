//! `system prune` reclamation of exited containers, pods, caches, volumes and images.

use super::df::count_and_size_subdirs;
use super::volumes::container_volume_names;
use super::model::PruneReport;
use crate::images;
use crate::service::CoralDaemonService;
use crate::service::DaemonStore;
use coral_api::ContainerState as CriContainerState;
use coral_runtime::rootfs::{default_volumes_dir, list_named_volumes, remove_named_volume};
use std::collections::HashSet;
use std::fs;
use tokio::process::Command;
use tracing::info;

impl CoralDaemonService {
    pub(crate) async fn prune_system(&self, prune_all: bool, prune_volumes: bool) -> PruneReport {
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
}

fn all_used_volumes(store: &DaemonStore) -> HashSet<String> {
    let mut used = HashSet::new();
    for ctr in store.containers.values() {
        used.extend(container_volume_names(ctr));
    }
    used
}
