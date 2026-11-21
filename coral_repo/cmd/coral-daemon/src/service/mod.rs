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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use streaming::CriStreamServer;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::warn;

/// How long the `images_dir` size walk behind `ImageFsInfo` is considered fresh.
const IMAGES_SIZE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(10);

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
    /// Orphaned pod sandboxes reaped by background Auto-GC since daemon start.
    pub auto_gc_pods_reaped: u64,
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
    /// Short-lived cache for the recursive `images_dir` size walk behind `ImageFsInfo` and
    /// `system df`. Kubelet polls `ImageFsInfo` on a fixed cadence, and the walk is O(inodes)
    /// over every image rootfs, so it must not run per request.
    pub(crate) images_size_cache: Arc<std::sync::Mutex<Option<(std::time::Instant, u64)>>>,
    /// Auto-GC interval requested via [`Self::with_auto_gc`] (`0` = disabled).
    ///
    /// [`Self::new`] returns before the CRI listeners are bound, so starting the timer there
    /// would let the very first pass land while kubelet is still doing its initial
    /// `Version`/`Status` handshake. `main` therefore calls [`Self::start_auto_gc`] once the
    /// listeners are up; the requested interval is parked here in the meantime.
    pub(crate) auto_gc_interval_secs: Arc<AtomicU64>,
    /// One-shot guard so Boot Auto-Resume runs at most once per daemon instance.
    pub(crate) boot_auto_resume_started: Arc<AtomicBool>,
    /// One-shot guard so the background Auto-GC loop starts at most once per daemon instance.
    pub(crate) auto_gc_started: Arc<AtomicBool>,
}

/// Delay before the in-process fallback kicks in Boot Auto-Resume / Auto-GC.
///
/// The `coral-daemon` binary triggers both explicitly once its CRI listeners are bound
/// (see `main`), so this fallback never fires there. It exists for embedders that build
/// [`CoralDaemonService`] in-process and serve it from their own gRPC server: without it
/// such a daemon would silently never resume `--restart always|unless-stopped` containers
/// and never reclaim stale state.
const INPROCESS_STARTUP_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

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
            images_size_cache: Arc::new(std::sync::Mutex::new(None)),
            auto_gc_interval_secs: Arc::new(AtomicU64::new(0)),
            boot_auto_resume_started: Arc::new(AtomicBool::new(false)),
            auto_gc_started: Arc::new(AtomicBool::new(false)),
        };
        // Boot Auto-Resume and Auto-GC are normally started by `main` once the CRI listeners
        // are bound (see `start_boot_auto_resume`). In-process embedders that serve the
        // service from their own gRPC server never reach that code path, so schedule a
        // short-grace fallback here; both starts are one-shot, so an explicit call from
        // `main` always wins and this fallback becomes a no-op.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let fallback = svc.clone();
            handle.spawn(async move {
                tokio::time::sleep(INPROCESS_STARTUP_GRACE).await;
                fallback.start_boot_auto_resume();
                fallback.start_auto_gc();
            });
        }
        svc
    }

    /// Overrides the CNI configuration and plugin binary directories (e.g., `/etc/cni/net.d`
    /// and `/opt/cni/bin`) for Kubernetes pod network attachment.
    pub fn with_cni(mut self, conf_dir: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> Self {
        self.cni = Arc::new(CniPluginChain::new(conf_dir, bin_dir));
        self
    }

    /// Starts Boot Auto-Resume for `--restart always|unless-stopped` containers.
    ///
    /// The `coral-daemon` binary calls this right after its CRI listeners are bound, so
    /// kubelet's first `Status`/`ListPodSandbox` is answered immediately instead of waiting
    /// behind the resume replay. In-process embedders may call it at any time; the
    /// one-shot guard makes repeated calls (e.g. the in-process fallback scheduled by
    /// [`Self::new`]) harmless.
    pub fn start_boot_auto_resume(&self) {
        if self.boot_auto_resume_started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.spawn_boot_auto_resume();
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

    /// Requests periodic background Auto-GC (`interval_secs == 0` disables).
    ///
    /// Only records the interval; the loop itself does not start until
    /// [`Self::start_auto_gc`] is called, so the first sweep cannot land before the CRI
    /// listeners are bound.
    pub fn with_auto_gc(self, interval_secs: u64) -> Self {
        self.auto_gc_interval_secs
            .store(interval_secs, Ordering::Relaxed);
        self
    }

    /// Starts the background Auto-GC loop if [`Self::with_auto_gc`] requested one.
    ///
    /// Called by `main` after the listeners are bound. Kept out of the synchronous part of
    /// [`Self::new`] for the same reason as [`Self::start_boot_auto_resume`]: the first pass
    /// walks `images_dir`, `exec-bundles/`, and every sandbox, which must not compete with
    /// kubelet's initial `ListPodSandbox`/`Status` calls. The one-shot guard keeps repeated
    /// calls (e.g. the in-process fallback scheduled by [`Self::new`]) harmless.
    pub fn start_auto_gc(&self) {
        let interval_secs = self.auto_gc_interval_secs.load(Ordering::Relaxed);
        if interval_secs > 0 && !self.auto_gc_started.swap(true, Ordering::SeqCst) {
            self.spawn_auto_gc_loop(interval_secs);
        }
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

    /// Returns the total size of `images_dir` (hardlink-deduplicated), caching the O(inodes)
    /// walk for [`IMAGES_SIZE_CACHE_TTL`].
    ///
    /// `ImageFsInfo` is on kubelet's polling path and `system df` is interactive; both previously
    /// rescanned every image rootfs per call, which on a host with several images dominated
    /// request latency. The cache is invalidated explicitly after pull/remove via
    /// [`Self::invalidate_images_size_cache`].
    pub(crate) fn cached_images_dir_size(&self) -> u64 {
        if let Ok(guard) = self.images_size_cache.lock()
            && let Some((at, bytes)) = *guard
            && at.elapsed() < IMAGES_SIZE_CACHE_TTL
        {
            return bytes;
        }
        let bytes = images::dir_size(&self.images_dir);
        if let Ok(mut guard) = self.images_size_cache.lock() {
            *guard = Some((std::time::Instant::now(), bytes));
        }
        bytes
    }

    /// Drops the cached `images_dir` size after a pull, build, commit, or image removal.
    pub(crate) fn invalidate_images_size_cache(&self) {
        if let Ok(mut guard) = self.images_size_cache.lock() {
            *guard = None;
        }
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
