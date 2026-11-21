//! Serialized report types returned by the housekeeping commands.

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoGcReport {
    pub files_removed: usize,
    pub bytes_reclaimed: u64,
    /// Orphaned pod sandboxes reclaimed this pass.
    pub pods_removed: usize,
    /// Per-sandbox teardown dirs (`sandboxes/<id>`) reclaimed with their sandbox record.
    pub sandbox_dirs_removed: usize,
}
