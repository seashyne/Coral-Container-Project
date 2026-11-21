use crate::service::{CoralDaemonConfig, CoralDaemonService, ManagedContainer, SandboxRecord};
use chrono::Utc;
use std::fs;
use std::path::Path;

#[tokio::test]
async fn auto_gc_reclaims_stale_staging_exec_bundles_and_build_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let images_dir = tmp.path().join("images");
    fs::create_dir_all(images_dir.join(".staging-abc")).unwrap();
    fs::write(images_dir.join(".staging-abc/layer.tar"), vec![7u8; 4096]).unwrap();
    fs::create_dir_all(state_dir.join("exec-bundles/dead-ctr")).unwrap();
    fs::write(state_dir.join("exec-bundles/dead-ctr/config.json"), b"{}").unwrap();
    fs::create_dir_all(state_dir.join("build-cache/entry-1")).unwrap();
    fs::write(state_dir.join("build-cache/entry-1/blob"), vec![1u8; 8192]).unwrap();

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: state_dir.clone(),
        images_dir: Some(images_dir.clone()),
        coral_bin: std::path::PathBuf::from("coral"),
        default_rootfs: None,
    })
    .with_log_rotation(1024, 2);

    let report = svc
        .run_auto_gc_with_age(std::time::Duration::ZERO, 1024)
        .await;
    assert!(report.files_removed >= 3);
    assert!(report.bytes_reclaimed >= 12_288);
    assert!(!images_dir.join(".staging-abc").exists());
    assert!(!state_dir.join("exec-bundles/dead-ctr").exists());
}

/// Builds a sandbox record with a bare-minimum `PodSandboxConfig`.
fn sandbox(id: &str, created_at: i64) -> SandboxRecord {
    SandboxRecord {
        id: id.to_string(),
        config: coral_api::PodSandboxConfig::default(),
        state: coral_api::PodSandboxState::SandboxReady,
        created_at,
        pod_ip: None,
        cni_raw_result: None,
    }
}

fn test_service(state_dir: &Path, images_dir: &Path) -> CoralDaemonService {
    CoralDaemonService::new(CoralDaemonConfig {
        state_dir: state_dir.to_path_buf(),
        images_dir: Some(images_dir.to_path_buf()),
        coral_bin: std::path::PathBuf::from("coral"),
        default_rootfs: None,
    })
    .with_log_rotation(1024, 2)
}

#[tokio::test]
async fn auto_gc_reaps_idle_orphan_sandboxes_but_keeps_live_and_fresh_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let images_dir = tmp.path().join("images");
    fs::create_dir_all(state_dir.join("sandboxes")).unwrap();
    fs::create_dir_all(&images_dir).unwrap();

    let svc = test_service(&state_dir, &images_dir);

    // `orphan` owns no container and is old -> must be reaped.
    // `busy` owns a container -> must survive even though it is old.
    // `fresh` owns no container but is young -> must survive this pass.
    fs::create_dir_all(state_dir.join("sandboxes/orphan")).unwrap();
    fs::write(state_dir.join("sandboxes/orphan/config.json"), b"{\"a\":1}").unwrap();
    fs::create_dir_all(state_dir.join("sandboxes/busy")).unwrap();
    fs::create_dir_all(state_dir.join("sandboxes/fresh")).unwrap();

    {
        let mut store = svc.store.write().await;
        store.sandboxes.insert(
            "orphan".into(),
            sandbox("orphan", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        );
        store.sandboxes.insert(
            "busy".into(),
            sandbox("busy", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        );
        store.sandboxes.insert(
            "fresh".into(),
            sandbox("fresh", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
        );
        store.containers.insert(
            "ctr-busy".into(),
            ManagedContainer {
                id: "ctr-busy".into(),
                pod_sandbox_id: "busy".into(),
                config: coral_api::ContainerConfig::default(),
                bundle_dir: state_dir.join("exec-bundles/ctr-busy"),
                rootfs_path: state_dir.join("rootfs/ctr-busy"),
                log_path: state_dir.join("logs/ctr-busy.log"),
                state: coral_api::ContainerState::ContainerExited,
                created_at: 0,
                started_at: 0,
                finished_at: 0,
                exit_code: 0,
                reason: "Completed".into(),
                message: String::new(),
                restart_policy: "no".into(),
                restart_count: 0,
                stopped_by_user: false,
                health_status: String::new(),
                health_failing_streak: 0,
            },
        );
    }

    // Rewind every sandbox by 2h; the threshold below is 1h, so only `orphan` and
    // `busy` are eligible and `busy` is then filtered out by the container check.
    svc.age_sandboxes_for_test(std::time::Duration::from_secs(7200))
        .await;
    {
        let mut store = svc.store.write().await;
        let now_ns = Utc::now().timestamp_nanos_opt().unwrap_or(0);
        store.sandboxes.get_mut("fresh").unwrap().created_at = now_ns;
    }

    let report = svc
        .run_auto_gc_with_age(std::time::Duration::from_secs(3600), 0)
        .await;

    assert_eq!(report.pods_removed, 1, "exactly the orphan should be reaped");
    assert_eq!(report.sandbox_dirs_removed, 1);
    assert!(
        report.bytes_reclaimed >= 7,
        "the orphan's config.json bytes should be accounted for"
    );

    let store = svc.store.read().await;
    assert!(!store.sandboxes.contains_key("orphan"));
    assert!(store.sandboxes.contains_key("busy"), "live pod must survive");
    assert!(store.sandboxes.contains_key("fresh"), "young pod must survive");
    assert_eq!(store.auto_gc_pods_reaped, 1);
    drop(store);

    assert!(!state_dir.join("sandboxes/orphan").exists());
    assert!(state_dir.join("sandboxes/busy").exists());
    assert!(state_dir.join("sandboxes/fresh").exists());
}

#[tokio::test]
async fn auto_gc_sweeps_progress_snapshots_abandoned_by_a_dead_cli() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let images_dir = tmp.path().join("images");
    fs::create_dir_all(state_dir.join("progress")).unwrap();
    fs::create_dir_all(&images_dir).unwrap();
    fs::write(state_dir.join("progress/123-abandoned.json"), b"{}").unwrap();

    let svc = test_service(&state_dir, &images_dir);
    let report = svc
        .run_auto_gc_with_age(std::time::Duration::ZERO, 0)
        .await;

    assert!(report.files_removed >= 1);
    assert!(!state_dir.join("progress/123-abandoned.json").exists());
}
