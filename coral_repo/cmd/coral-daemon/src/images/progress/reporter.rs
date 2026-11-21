//! Reporter that atomically publishes pull progress snapshots to disk.

use super::{PROGRESS_DIR, PullPhase, PullProgress};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Atomically publishes [`PullProgress`] snapshots to `<state_dir>/progress/<pull_id>.json`.
///
/// Every counter is an atomic so the shared `Arc` can be ticked from the streaming loop without
/// taking a lock per 64 KiB chunk; only flushes take the write lock and touch the filesystem.
#[derive(Debug)]
pub struct PullProgressReporter {
    pull_id: String,
    image: String,
    dir: PathBuf,
    started: std::time::Instant,
    phase: std::sync::RwLock<PullPhase>,
    layers_done: AtomicU64,
    layers_total: AtomicU64,
    current_layer: AtomicU64,
    current_layer_total_bytes: AtomicU64,
    bytes_downloaded: AtomicU64,
    bytes_total: AtomicU64,
    message: std::sync::RwLock<String>,
}

impl PullProgressReporter {
    /// Creates a reporter that writes `<state_dir>/progress/<pull_id>.json`.
    pub fn new(
        state_dir: &Path,
        pull_id: impl Into<String>,
        image: impl Into<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pull_id: pull_id.into(),
            image: image.into(),
            dir: state_dir.join(PROGRESS_DIR),
            started: std::time::Instant::now(),
            phase: std::sync::RwLock::new(PullPhase::Resolving),
            layers_done: AtomicU64::new(0),
            layers_total: AtomicU64::new(0),
            current_layer: AtomicU64::new(0),
            current_layer_total_bytes: AtomicU64::new(0),
            bytes_downloaded: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            message: std::sync::RwLock::new(String::new()),
        })
    }

    pub fn pull_id(&self) -> &str {
        &self.pull_id
    }

    /// Absolute path of the snapshot file for this pull.
    pub fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.json", self.pull_id))
    }

    pub fn set_phase(&self, phase: PullPhase) {
        if let Ok(mut guard) = self.phase.write() {
            *guard = phase;
        }
    }

    pub fn set_layers_total(&self, total: usize) {
        self.layers_total.store(total as u64, Ordering::Relaxed);
    }

    pub fn set_bytes_total(&self, total: u64) {
        self.bytes_total.store(total, Ordering::Relaxed);
    }

    /// Marks `layer_index` (1-based) as the layer now being downloaded.
    pub fn begin_layer(&self, layer_index: usize, declared_bytes: u64) {
        self.current_layer
            .store(layer_index as u64, Ordering::Relaxed);
        self.current_layer_total_bytes
            .store(declared_bytes, Ordering::Relaxed);
        self.set_phase(PullPhase::Downloading);
    }

    /// Adds `delta` bytes to the downloaded counter without flushing a snapshot.
    pub fn add_bytes(&self, delta: u64) {
        self.bytes_downloaded.fetch_add(delta, Ordering::Relaxed);
    }

    /// Increments the completed-layer counter (called once extraction finishes).
    pub fn finish_layer(&self) {
        self.layers_done.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a failure reason; the phase becomes [`PullPhase::Failed`].
    pub fn fail(&self, message: impl Into<String>) {
        if let Ok(mut guard) = self.message.write() {
            *guard = message.into();
        }
        self.set_phase(PullPhase::Failed);
        self.flush();
    }

    /// Builds a snapshot of the current counters.
    pub fn snapshot(&self) -> PullProgress {
        let bytes_downloaded = self.bytes_downloaded.load(Ordering::Relaxed);
        let bytes_total = self.bytes_total.load(Ordering::Relaxed);
        let layers_done = self.layers_done.load(Ordering::Relaxed) as usize;
        let layers_total = self.layers_total.load(Ordering::Relaxed) as usize;
        let phase = self.phase.read().map(|g| *g).unwrap_or(PullPhase::Resolving);
        let message = self.message.read().map(|g| g.clone()).unwrap_or_default();

        // Percentage is the more reliable signal: layer counters can be known while the byte
        // total is not (multi-arch index child manifests carry no size annotation).
        let percent = if bytes_total > 0 {
            bytes_downloaded
                .saturating_mul(100)
                .checked_div(bytes_total)
                .unwrap_or(0)
                .min(100) as u8
        } else if layers_total > 0 {
            layers_done
                .saturating_mul(100)
                .checked_div(layers_total)
                .unwrap_or(0)
                .min(100) as u8
        } else {
            0
        };
        let percent = if phase == PullPhase::Done { 100 } else { percent };

        PullProgress {
            pull_id: self.pull_id.clone(),
            image: self.image.clone(),
            phase,
            layers_done,
            layers_total,
            current_layer: self.current_layer.load(Ordering::Relaxed) as usize,
            current_layer_total_bytes: self
                .current_layer_total_bytes
                .load(Ordering::Relaxed),
            bytes_downloaded,
            bytes_total,
            percent,
            elapsed_ms: self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            message,
        }
    }

    /// Atomically writes the current snapshot (`<pull_id>.json.tmp` + rename).
    ///
    /// Best-effort: a failed progress write must never fail the pull it is describing.
    pub fn flush(&self) {
        if fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let Ok(json) = serde_json::to_string(&self.snapshot()) else {
            return;
        };
        let target = self.path();
        let tmp = self.dir.join(format!("{}.json.tmp", self.pull_id));
        if fs::write(&tmp, json).is_ok() {
            let _ = fs::rename(&tmp, &target);
        }
    }

    /// Flushes a terminal snapshot and removes the file so `<state_dir>/progress/` stays clean.
    pub fn finish(self: &Arc<Self>) {
        self.flush();
        let _ = fs::remove_file(self.path());
    }

    /// Removes any leftover terminal snapshot for `pull_id` (e.g. a crash from a previous run).
    pub fn clear_stale(state_dir: &Path, pull_id: &str) {
        let dir = state_dir.join(PROGRESS_DIR);
        let _ = fs::remove_file(dir.join(format!("{pull_id}.json")));
        let _ = fs::remove_file(dir.join(format!("{pull_id}.json.tmp")));
    }

    /// Best-effort removal of snapshots that were never cleaned up by a crashed daemon.
    pub fn sweep_stale(state_dir: &Path, older_than: std::time::Duration) -> usize {
        let dir = state_dir.join(PROGRESS_DIR);
        let Ok(rd) = fs::read_dir(&dir) else {
            return 0;
        };
        let now = std::time::SystemTime::now();
        let mut removed = 0usize;
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let old = fs::metadata(&path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age >= older_than);
            if old && fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}
