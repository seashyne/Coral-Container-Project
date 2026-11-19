//! CRI v1 `CoralDaemonService` state store, configuration, and module exports.

mod compose_rpc;
pub mod housekeeping;
mod image_rpc;
pub mod metrics;
pub mod nextgen;
mod oci_spec;
mod persist;
mod port_forward;
mod pty_rpc;
mod runtime_rpc;
mod supervisor;

use crate::images::{self, StoredImage};
use chrono::Utc;
use coral_api::{ContainerConfig, ContainerState as CriContainerState, PodSandboxConfig, PodSandboxState};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::warn;

/// Configuration for `CoralDaemonService`.
#[derive(Debug, Clone)]
pub struct CoralDaemonConfig {
    /// Directory where daemon stores container bundles, logs, and runtime state.
    pub state_dir: PathBuf,
    /// Optional directory where pulled OCI base images and `images.json` are stored.
    /// Defaults to `<state_dir>/images` when omitted.
    pub images_dir: Option<PathBuf>,
    /// Path to the `coral` OCI CLI binary used to execute isolated containers.
    pub coral_bin: PathBuf,
    /// Optional default local rootfs directory (e.g., Alpine minirootfs) registered as `alpine:latest`.
    pub default_rootfs: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct SandboxRecord {
    pub id: String,
    pub config: PodSandboxConfig,
    pub state: PodSandboxState,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedContainer {
    pub id: String,
    pub pod_sandbox_id: String,
    pub config: ContainerConfig,
    pub bundle_dir: PathBuf,
    pub rootfs_path: PathBuf,
    pub log_path: PathBuf,
    pub state: CriContainerState,
    pub created_at: i64,
    pub started_at: i64,
    pub finished_at: i64,
    pub exit_code: i32,
    pub reason: String,
    pub message: String,
    pub restart_policy: String,
    pub restart_count: u32,
    pub stopped_by_user: bool,
    pub health_status: String,
    pub health_failing_streak: u32,
}

#[derive(Debug, Default)]
pub(crate) struct DaemonStore {
    pub sandboxes: HashMap<String, SandboxRecord>,
    pub containers: HashMap<String, ManagedContainer>,
    pub port_forwarders: HashMap<String, Vec<JoinHandle<()>>>,
    pub supervisors: HashMap<String, JoinHandle<()>>,
    pub images: Vec<StoredImage>,
}

/// Implements both Kubernetes CRI v1 `RuntimeService` and `ImageService`.
#[derive(Debug, Clone)]
pub struct CoralDaemonService {
    pub(crate) cfg: Arc<CoralDaemonConfig>,
    pub(crate) images_dir: PathBuf,
    pub(crate) store: Arc<RwLock<DaemonStore>>,
    pub(crate) seq: Arc<AtomicU64>,
}

impl CoralDaemonService {
    pub fn new(cfg: CoralDaemonConfig) -> Self {
        let images_dir = cfg
            .images_dir
            .clone()
            .unwrap_or_else(|| cfg.state_dir.join("images"));

        let mut initial = DaemonStore {
            images: images::load_index(&images_dir),
            ..DaemonStore::default()
        };
        let runtime_root = cfg.state_dir.join("runtime-state");
        persist::load_persisted_store(&cfg.state_dir, &runtime_root, &mut initial);

        // root.path in an OCI spec is resolved relative to the bundle dir by the
        // runtime, so registered rootfs paths must always be absolute.
        if let Some(rootfs) = &cfg.default_rootfs
            && let Ok(abs) = fs::canonicalize(rootfs)
            && abs.is_dir()
        {
            let mut default_tags = Vec::new();
            for tag in ["alpine:latest", "alpine:3.20"] {
                if images::find_image(&initial.images, tag).is_none() {
                    default_tags.push(tag.to_string());
                }
            }
            if !default_tags.is_empty() {
                let rootfs_str = abs.to_string_lossy().into_owned();
                let size = images::dir_size(&abs);
                initial.images.push(StoredImage {
                    id: "sha256:coral-alpine-local".into(),
                    repo_tags: default_tags,
                    size,
                    rootfs_path: rootfs_str,
                    managed: false,
                });
            }
        }

        let svc = Self {
            cfg: Arc::new(cfg),
            images_dir,
            store: Arc::new(RwLock::new(initial)),
            seq: Arc::new(AtomicU64::new(1)),
        };
        svc.spawn_boot_auto_resume();
        svc
    }

    pub(crate) fn next_id(&self, prefix: &str) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let ts = Utc::now().timestamp_millis();
        format!("{prefix}-{ts}-{n}")
    }

    pub(crate) fn runtime_root(&self) -> PathBuf {
        self.cfg.state_dir.join("runtime-state")
    }

    pub(crate) fn persist_images(&self, images: &[StoredImage]) {
        if let Err(e) = images::save_index(&self.images_dir, images) {
            warn!(err = %e, dir = %self.images_dir.display(), "failed to persist images.json");
        }
    }

    pub(crate) fn persist_state(&self, store: &DaemonStore) {
        persist::save_persisted_store(&self.cfg.state_dir, store);
    }

    pub(crate) fn cleanup_unused_managed_dir(&self, remaining: &[StoredImage], rootfs_path: &str) {
        if let Err(e) = images::cleanup_unused_managed_dir(&self.images_dir, remaining, rootfs_path)
        {
            warn!(err = %e, path = %rootfs_path, "failed to remove managed image directory");
        }
    }

    /// Aborts background supervisor and port-forwarding tasks managed by this daemon instance.
    pub async fn shutdown(&self) {
        let mut store = self.store.write().await;
        for (_, handle) in store.supervisors.drain() {
            handle.abort();
        }
        for (_, handles) in store.port_forwarders.drain() {
            for h in handles {
                h.abort();
            }
        }
    }
}
