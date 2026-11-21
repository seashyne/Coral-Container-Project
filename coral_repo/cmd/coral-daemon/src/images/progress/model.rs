//! Progress snapshot data model: phase enum + serialized snapshot struct.

use serde::{Deserialize, Serialize};

/// Pull lifecycle phase, serialized `camelCase` for the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PullPhase {
    /// Resolving the manifest / multi-arch index.
    Resolving,
    /// Streaming layers from the registry into the staging directory.
    Downloading,
    /// Unpacking layer tarballs into the staging rootfs.
    Extracting,
    /// Atomically installing the rootfs and computing its size.
    Finalizing,
    /// Pull finished successfully.
    Done,
    /// Pull failed; `message` carries the reason.
    Failed,
}

impl PullPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Resolving => "resolving",
            Self::Downloading => "downloading",
            Self::Extracting => "extracting",
            Self::Finalizing => "finalizing",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// Serialized progress snapshot written to `<state_dir>/progress/<pull_id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub pull_id: String,
    pub image: String,
    pub phase: PullPhase,
    /// Layers fully downloaded and extracted so far.
    pub layers_done: usize,
    /// Total layers in the resolved platform manifest (`0` while resolving).
    pub layers_total: usize,
    /// 1-based index of the layer currently being streamed.
    pub current_layer: usize,
    /// `Content-Length`-derived size of the current layer, when the registry reports one.
    pub current_layer_total_bytes: u64,
    pub bytes_downloaded: u64,
    /// Sum of layer sizes from the manifest. Accurate for OCI manifests; `0` for multi-arch
    /// indexes, where the child manifest is not size-annotated before the first byte arrives.
    pub bytes_total: u64,
    /// `0..=100`, saturating at 100 while `bytes_total` is unknown.
    pub percent: u8,
    pub elapsed_ms: u64,
    pub message: String,
}

impl PullProgress {
    pub fn new(pull_id: impl Into<String>, image: impl Into<String>) -> Self {
        Self {
            pull_id: pull_id.into(),
            image: image.into(),
            phase: PullPhase::Resolving,
            layers_done: 0,
            layers_total: 0,
            current_layer: 0,
            current_layer_total_bytes: 0,
            bytes_downloaded: 0,
            bytes_total: 0,
            percent: 0,
            elapsed_ms: 0,
            message: String::new(),
        }
    }
}
