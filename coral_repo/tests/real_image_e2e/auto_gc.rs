//! Automatic pod-sandbox garbage collection semantics.

use super::*;

#[tokio::test]
async fn auto_gc_reaps_only_orphaned_sandboxes_and_keeps_live_ones() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let images_dir = dir.path().join("images");
    let (endpoint, svc, handle) = spawn_daemon(state_dir, images_dir).await;
    let mut rt = RuntimeServiceClient::connect(endpoint.clone()).await.unwrap();

    // Register the local Alpine minirootfs so `create_container` below needs no registry
    // access. This test asserts Auto-GC semantics, not pull behaviour, so it must stay
    // runnable in an air-gapped CI job.
    let mut img = ImageServiceClient::connect(endpoint).await.unwrap();
    let local_rootfs = alpine_rootfs();
    // The fixture is committed, but create it if a fresh clone lacks it: this test only needs
    // *a* resolvable rootfs so `create_container` succeeds, never a bootable one.
    std::fs::create_dir_all(&local_rootfs).expect("create local rootfs fixture dir");
    assert!(
        local_rootfs.is_dir(),
        "expected the unpacked Alpine minirootfs at {}",
        local_rootfs.display()
    );
    let mut pull_annotations = HashMap::new();
    pull_annotations.insert(
        "coral.rootfs".to_string(),
        local_rootfs.to_string_lossy().into_owned(),
    );
    img.pull_image(PullImageRequest {
        image: Some(ImageSpec {
            image: "alpine:3.20".into(),
            annotations: pull_annotations,
        }),
    })
    .await
    .expect("register the local Alpine rootfs");

    // Two sandboxes: one stays empty (orphaned), one owns a container (live).
    let orphan_pod = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "gc-orphan".into(),
                    uid: "gc-orphan-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "gc-orphan".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    let live_pod = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "gc-live".into(),
                    uid: "gc-live-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "gc-live".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    // Give only the live pod a container, so Auto-GC has something to distinguish.
    rt.create_container(CreateContainerRequest {
        pod_sandbox_id: live_pod.clone(),
        config: Some(ContainerConfig {
            metadata: Some(ContainerMetadata {
                name: "gc-live-ctr".into(),
                attempt: 0,
            }),
            image: Some(ImageSpec {
                image: "alpine:3.20".into(),
                annotations: HashMap::new(),
            }),
            command: vec!["/bin/true".into()],
            args: Vec::new(),
            working_dir: "/".into(),
            envs: Vec::new(),
            labels: HashMap::new(),
            annotations: HashMap::new(),
            log_path: String::new(),
            linux: None,
        }),
        sandbox_config: None,
    })
    .await
    .expect("create a container owned by the live sandbox");

    // Age both sandboxes past the idle threshold so they become GC candidates.
    svc.age_sandboxes_for_test(std::time::Duration::from_secs(7200))
        .await;

    let report = svc.run_auto_gc_with_age(Duration::from_secs(3600), 0).await;
    assert_eq!(
        report.pods_removed, 1,
        "exactly the sandbox owning no container must be reaped"
    );

    let remaining = rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert!(
        remaining.iter().any(|p| p.id == live_pod),
        "the sandbox owning a container must survive Auto-GC"
    );
    assert!(
        !remaining.iter().any(|p| p.id == orphan_pod),
        "the orphaned sandbox must have been reaped"
    );

    // A fresh sandbox created just now is never reaped, even when it owns nothing.
    let young_pod = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "gc-young".into(),
                    uid: "gc-young-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "gc-young".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    let report2 = svc.run_auto_gc_with_age(Duration::from_secs(3600), 0).await;
    assert_eq!(
        report2.pods_removed, 0,
        "a sandbox created seconds ago must not be reaped"
    );
    let after = rt
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert!(after.iter().any(|p| p.id == young_pod));

    // Every reclaim pass bumps the Auto-GC counter exposed via `/metrics`.
    let metrics = coral_daemon::service::prometheus::render_prometheus_metrics(&svc).await;
    assert!(
        metrics.contains("coral_daemon_auto_gc_pods_reaped_total 1"),
        "expected the pods-reaped counter to report 1:\n{metrics}"
    );
    assert!(
        metrics.contains("coral_daemon_orphaned_pods"),
        "expected the orphaned-pods gauge to be exported:\n{metrics}"
    );

    let _ = rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: live_pod,
        })
        .await;
    svc.shutdown().await;
    handle.abort();
}
