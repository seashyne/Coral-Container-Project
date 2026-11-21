//! Named-volume listing and per-container volume usage mapping.

use super::model::VolumeSummary;
use crate::images;
use crate::service::CoralDaemonService;
use coral_runtime::rootfs::{default_volumes_dir, list_named_volumes};
use std::collections::{HashMap, HashSet};
use tonic::Status;

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn list_volumes_summary(&self) -> Result<Vec<VolumeSummary>, Status> {
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

pub(super) fn container_volume_names(ctr: &crate::service::ManagedContainer) -> HashSet<String> {
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
