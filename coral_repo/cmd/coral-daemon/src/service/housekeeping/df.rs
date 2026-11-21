//! `system df` disk-usage accounting.

use super::model::SystemDiskUsage;
use crate::images;
use crate::service::CoralDaemonService;
use coral_api::ContainerState as CriContainerState;
use coral_runtime::rootfs::{default_volumes_dir, list_named_volumes};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

impl CoralDaemonService {
    pub(crate) async fn compute_system_df(&self) -> SystemDiskUsage {
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
}

pub(super) fn count_and_size_subdirs(dir: &Path) -> (usize, u64) {
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
