//! Background Auto-GC: stale artifact reclamation and orphaned pod-sandbox reaping.

use super::model::AutoGcReport;
use crate::images;
use crate::service::CoralDaemonService;
use chrono::Utc;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tracing::{info, warn};

impl CoralDaemonService {
    /// Spawns a periodic background garbage collector if `interval_secs > 0`.
    pub(crate) fn spawn_auto_gc_loop(&self, interval_secs: u64) -> Option<tokio::task::JoinHandle<()>> {
        if interval_secs == 0 {
            return None;
        }
        let svc = self.clone();
        Some(tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(interval_secs.max(5));
            loop {
                tokio::time::sleep(interval).await;
                let report = svc.run_auto_gc_once().await;
                if report.files_removed > 0 || report.pods_removed > 0 {
                    info!(
                        files_removed = report.files_removed,
                        pods_removed = report.pods_removed,
                        bytes_reclaimed = report.bytes_reclaimed,
                        "coral-daemon background Auto-GC reclaimed artifacts"
                    );
                }
            }
        }))
    }

    /// Runs a single safe garbage-collection pass over orphaned staging dirs, stale exec-bundles,
    /// orphaned pod sandboxes, excess rotated log archives, and oversized build-cache entries
    /// (> 512 MiB).
    pub(crate) async fn run_auto_gc_once(&self) -> AutoGcReport {
        self.run_auto_gc_with_age(std::time::Duration::from_secs(3600), 512 * 1024 * 1024)
            .await
    }

    /// Sweeps pod sandboxes that own no container and have been that way for `min_idle`.
    ///
    /// This is the automatic counterpart to step 2 of `prune_system`: without it, every
    /// `coral run`/`compose up` invocation leaves its sandbox record behind forever, because
    /// `prune` only runs when a user explicitly asks for it.
    ///
    /// Safety rails, in order:
    /// 1. Only sandboxes whose `created_at` predates `now - min_idle` are candidates, so a
    ///    sandbox whose `CreateContainer` RPC is still in flight is never reaped.
    /// 2. Sandboxes with any container record are skipped — even an exited one, because
    ///    `coral start` can revive it.
    /// 3. `cni_raw_result` is only populated for sandboxes that actually attached a network
    ///    namespace, so air-gapped pods never trigger a bogus CNI `DEL`.
    ///
    /// Returns `(pods_removed, sandbox_dirs_removed)`.
    async fn sweep_orphaned_sandboxes(
        &self,
        min_idle: std::time::Duration,
        report: &mut AutoGcReport,
    ) -> (usize, usize) {
        let now_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let idle_ns = i64::try_from(min_idle.as_nanos()).unwrap_or(i64::MAX);

        // Snapshot candidates, then drop the store lock before touching CNI or the filesystem.
        let candidates: Vec<(String, Option<serde_json::Value>, coral_api::PodSandboxConfig)> = {
            let store = self.store.read().await;
            let active_pods: HashSet<&String> =
                store.containers.values().map(|c| &c.pod_sandbox_id).collect();
            store
                .sandboxes
                .values()
                .filter(|sb| !active_pods.contains(&sb.id))
                .filter(|sb| sb.created_at <= now_ns.saturating_sub(idle_ns))
                .map(|sb| {
                    (
                        sb.id.clone(),
                        sb.cni_raw_result.clone(),
                        sb.config.clone(),
                    )
                })
                .collect()
        };

        if candidates.is_empty() {
            return (0, 0);
        }

        let sandbox_root = self.cfg.state_dir.join("sandboxes");
        let mut pods_removed = 0usize;
        let mut dirs_removed = 0usize;

        for (pod_id, cni_raw, config) in candidates {
            // Release the CNI IP allocation held by a pod nobody will ever reference again.
            if let Some(prev_res) = cni_raw {
                let cni = self.cni.clone();
                let cni_req = crate::service::runtime_rpc::build_cni_exec_request(&pod_id, &config);
                let label = config
                    .metadata
                    .as_ref()
                    .map(|m| m.name.clone())
                    .unwrap_or_default();
                match tokio::task::spawn_blocking(move || {
                    cni.del_pod_network(&cni_req, Some(&prev_res))
                })
                .await
                {
                    // A failing CNI DEL must not block the record from being reclaimed; leaked
                    // IPAM entries are logged so an operator can reconcile them.
                    Ok(Err(e)) => warn!(
                        pod_sandbox_id = %pod_id,
                        pod = %label,
                        err = %e,
                        "CNI DEL failed while auto-GC reaping orphaned pod sandbox"
                    ),
                    Err(e) => warn!(
                        pod_sandbox_id = %pod_id,
                        err = %e,
                        "CNI DEL task panicked while auto-GC reaping orphaned pod sandbox"
                    ),
                    Ok(Ok(())) => {}
                }
            }

            let dir = sandbox_root.join(&pod_id);
            if dir.is_dir() {
                let bytes = images::dir_size(&dir);
                if coral_runtime::remove_dir_all_rootless_safe(&dir).is_ok() {
                    report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
                    dirs_removed += 1;
                }
            }

            // Re-check under the write lock: a container may have been created for this pod
            // between the candidate snapshot and now, which makes the pod live again.
            let mut store = self.store.write().await;
            let still_orphaned = !store
                .containers
                .values()
                .any(|c| c.pod_sandbox_id == pod_id);
            if still_orphaned && store.sandboxes.remove(&pod_id).is_some() {
                pods_removed += 1;
                self.persist_state(&store);
            }
            drop(store);
        }

        (pods_removed, dirs_removed)
    }

    /// Rewinds every sandbox's `created_at` by `age`, so tests can drive the
    /// idle-threshold branch of [`Self::sweep_orphaned_sandboxes`] without sleeping.
    ///
    /// Not compiled into release builds: `coral-daemon` is a library as well as a binary, so
    /// an ungated `pub` helper here would become part of the shipped API surface.
    #[doc(hidden)]
    #[cfg(any(test, feature = "test-helpers"))]
    pub async fn age_sandboxes_for_test(&self, age: std::time::Duration) {
        let delta = i64::try_from(age.as_nanos()).unwrap_or(i64::MAX);
        let mut store = self.store.write().await;
        for sb in store.sandboxes.values_mut() {
            sb.created_at = sb.created_at.saturating_sub(delta);
        }
        self.persist_state(&store);
    }

    /// Runs one Auto-GC sweep with explicit thresholds (see [`AutoGcReport`]).
    ///
    /// Public so integration tests can drive a deterministic pass without waiting for the
    /// background timer.
    pub async fn run_auto_gc_with_age(
        &self,
        orphan_min_age: std::time::Duration,
        build_cache_cap_bytes: u64,
    ) -> AutoGcReport {
        let mut report = AutoGcReport::default();
        let now = std::time::SystemTime::now();
        let max_files = self.current_log_policy().max_files;

        let (active_cids, log_paths) = {
            let store = self.store.read().await;
            let cids: HashSet<String> = store.containers.keys().cloned().collect();
            let logs: Vec<std::path::PathBuf> =
                store.containers.values().map(|c| c.log_path.clone()).collect();
            (cids, logs)
        };

        // 1. Reclaim expired `.tmp*` / `.staging*` artifacts in images_dir.
        if let Ok(rd) = fs::read_dir(&self.images_dir) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".tmp") || name.starts_with(".staging") || name.ends_with(".tmp")
                {
                    let path = entry.path();
                    if is_older_than(&path, now, orphan_min_age) {
                        reclaim_path(&path, &mut report);
                    }
                }
            }
        }

        // 2. Reclaim orphaned `exec-bundles/<id>` dirs not mapped to any known container.
        let exec_bundles = self.cfg.state_dir.join("exec-bundles");
        if let Ok(rd) = fs::read_dir(&exec_bundles) {
            for entry in rd.flatten() {
                let path = entry.path();
                let cid = entry.file_name().to_string_lossy().into_owned();
                if !active_cids.contains(&cid) && is_older_than(&path, now, orphan_min_age) {
                    reclaim_path(&path, &mut report);
                }
            }
        }

        // 3. Enforce max rotated log files (`.1`..`.N`) per active container log.
        for log_path in log_paths {
            let Some(parent) = log_path.parent() else {
                continue;
            };
            let Some(base_name) = log_path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let prefix = format!("{base_name}.");
            if let Ok(rd) = fs::read_dir(parent) {
                for entry in rd.flatten() {
                    let fname = entry.file_name().to_string_lossy().into_owned();
                    if let Some(idx_str) = fname.strip_prefix(&prefix)
                        && let Ok(idx) = idx_str.parse::<usize>()
                        && idx > max_files
                    {
                        reclaim_path(&entry.path(), &mut report);
                    }
                }
            }
        }

        // 4. Trim oldest `build-cache/*` entries if build-cache exceeds threshold.
        let cache_dir = self.cfg.state_dir.join("build-cache");        if build_cache_cap_bytes > 0
            && let Ok(rd) = fs::read_dir(&cache_dir)
        {
            let mut entries: Vec<(std::time::SystemTime, u64, std::path::PathBuf)> = Vec::new();
            let mut total = 0u64;
            for entry in rd.flatten() {
                let p = entry.path();
                let size = if p.is_dir() {
                    images::dir_size(&p)
                } else {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                };
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                total = total.saturating_add(size);
                entries.push((mtime, size, p));
            }
            if total > build_cache_cap_bytes {
                entries.sort_by_key(|(t, _, _)| *t);
                for (_, size, p) in entries {
                    if total <= build_cache_cap_bytes {
                        break;
                    }
                    reclaim_path(&p, &mut report);
                    total = total.saturating_sub(size);
                }
            }
        }

        // 5. Reap pod sandboxes that own no container (the automatic counterpart of
        //    `coral system prune`, which a user must invoke explicitly).
        let (pods_removed, sandbox_dirs_removed) =
            self.sweep_orphaned_sandboxes(orphan_min_age, &mut report)
                .await;
        report.pods_removed = pods_removed;
        report.sandbox_dirs_removed = sandbox_dirs_removed;

        // 6. Drop `progress/*.json` snapshots left behind by a CLI that died mid-pull.
        let stale_progress = crate::images::progress::PullProgressReporter::sweep_stale(
            &self.cfg.state_dir,
            orphan_min_age,
        );
        report.files_removed = report.files_removed.saturating_add(stale_progress);

        let mut store = self.store.write().await;
        store.auto_gc_runs = store.auto_gc_runs.saturating_add(1);
        store.auto_gc_pods_reaped = store.auto_gc_pods_reaped.saturating_add(pods_removed as u64);
        store.auto_gc_bytes_reclaimed = store
            .auto_gc_bytes_reclaimed
            .saturating_add(report.bytes_reclaimed);
        report
    }
}

fn is_older_than(path: &Path, now: std::time::SystemTime, age: std::time::Duration) -> bool {
    if age.is_zero() {
        return true;
    }
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| now.duration_since(m).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

fn reclaim_path(path: &Path, report: &mut AutoGcReport) {
    if path.is_dir() {
        let bytes = images::dir_size(path);
        if coral_runtime::remove_dir_all_rootless_safe(path).is_ok() {
            report.files_removed += 1;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
        }
    } else if path.is_file() {
        let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if fs::remove_file(path).is_ok() {
            report.files_removed += 1;
            report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(bytes);
        }
    }
}
