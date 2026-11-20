//! CRI v1 `CoralDaemonService` state store, configuration, and module exports.

mod compose_rpc;
pub mod housekeeping;
mod image_rpc;
pub mod metrics;
pub mod nextgen;
mod oci_spec;
mod persist;
mod port_forward;
pub mod prometheus;
mod pty_rpc;
mod runtime_rpc;
mod streaming;
mod supervisor;

use crate::images::{self, StoredImage};
use chrono::Utc;
use coral_api::{ContainerConfig, ContainerState as CriContainerState, PodSandboxConfig, PodSandboxState};
use coral_net::CniPluginChain;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use streaming::CriStreamServer;
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
    pub pod_ip: Option<String>,
    pub cni_raw_result: Option<serde_json::Value>,
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
    pub pod_cidr: Option<String>,
    pub auto_gc_runs: u64,
    pub auto_gc_bytes_reclaimed: u64,
}

impl DaemonStore {
    /// Resolves a CRI container ID, container `metadata.name` (e.g. `coral run --name foo`),
    /// or unique container ID prefix into the canonical container ID stored in `self.containers`.
    pub(crate) fn resolve_container_id(&self, query: &str) -> Option<String> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        if self.containers.contains_key(q) {
            return Some(q.to_string());
        }
        let mut exact_name: Vec<&ManagedContainer> = self
            .containers
            .values()
            .filter(|c| c.config.metadata.as_ref().is_some_and(|m| m.name == q))
            .collect();
        if !exact_name.is_empty() {
            exact_name.sort_by_key(|c| std::cmp::Reverse(c.created_at));
            if let Some(running) = exact_name
                .iter()
                .find(|c| c.state == CriContainerState::ContainerRunning)
            {
                return Some(running.id.clone());
            }
            return Some(exact_name[0].id.clone());
        }
        if q.len() >= 4 {
            let mut prefix_matches = self.containers.keys().filter(|id| id.starts_with(q));
            if let (Some(first), None) = (prefix_matches.next(), prefix_matches.next()) {
                return Some(first.clone());
            }
        }
        None
    }

    /// Resolves a CRI pod sandbox ID, sandbox `metadata.name`, or unique ID prefix.
    pub(crate) fn resolve_sandbox_id(&self, query: &str) -> Option<String> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        if self.sandboxes.contains_key(q) {
            return Some(q.to_string());
        }
        let mut exact_name: Vec<&SandboxRecord> = self
            .sandboxes
            .values()
            .filter(|s| s.config.metadata.as_ref().is_some_and(|m| m.name == q))
            .collect();
        if !exact_name.is_empty() {
            exact_name.sort_by_key(|s| std::cmp::Reverse(s.created_at));
            return Some(exact_name[0].id.clone());
        }
        if q.len() >= 4 {
            let mut prefix_matches = self.sandboxes.keys().filter(|id| id.starts_with(q));
            if let (Some(first), None) = (prefix_matches.next(), prefix_matches.next()) {
                return Some(first.clone());
            }
        }
        None
    }
}

/// Implements both Kubernetes CRI v1 `RuntimeService` and `ImageService`.
#[derive(Debug, Clone)]
pub struct CoralDaemonService {
    pub(crate) cfg: Arc<CoralDaemonConfig>,
    pub(crate) images_dir: PathBuf,
    pub(crate) store: Arc<RwLock<DaemonStore>>,
    pub(crate) seq: Arc<AtomicU64>,
    pub(crate) cni: Arc<CniPluginChain>,
    pub(crate) stream_server: Arc<CriStreamServer>,
    pub(crate) log_policy: Arc<std::sync::RwLock<supervisor::LogRotationPolicy>>,
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
            cni: Arc::new(CniPluginChain::default()),
            stream_server: Arc::new(CriStreamServer::default()),
            log_policy: Arc::new(std::sync::RwLock::new(
                supervisor::LogRotationPolicy::default(),
            )),
        };
        svc.spawn_boot_auto_resume();
        svc
    }

    /// Overrides the CNI configuration and plugin binary directories (e.g., `/etc/cni/net.d`
    /// and `/opt/cni/bin`) for Kubernetes pod network attachment.
    pub fn with_cni(mut self, conf_dir: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> Self {
        self.cni = Arc::new(CniPluginChain::new(conf_dir, bin_dir));
        self
    }

    /// Configures container log rotation limits (`max_size_bytes == 0` disables rotation).
    pub fn with_log_rotation(self, max_size_bytes: u64, max_files: usize) -> Self {
        if let Ok(mut guard) = self.log_policy.write() {
            *guard = supervisor::LogRotationPolicy {
                max_size_bytes,
                max_files,
            };
        }
        self
    }

    /// Enables periodic background Auto-GC (`interval_secs == 0` disables).
    pub fn with_auto_gc(self, interval_secs: u64) -> Self {
        self.spawn_auto_gc_loop(interval_secs);
        self
    }

    pub(crate) fn current_log_policy(&self) -> supervisor::LogRotationPolicy {
        self.log_policy
            .read()
            .map(|g| *g)
            .unwrap_or_default()
    }

    pub(crate) fn next_id(&self, prefix: &str) -> String {
        static GLOBAL_ID_SEQ: AtomicU64 = AtomicU64::new(1);
        let _ = &self.seq;
        let n = GLOBAL_ID_SEQ.fetch_add(1, Ordering::Relaxed);
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

    /// Aborts background supervisor, streaming server, and port-forwarding tasks managed by this daemon instance.
    pub async fn shutdown(&self) {
        self.stream_server.shutdown().await;
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
