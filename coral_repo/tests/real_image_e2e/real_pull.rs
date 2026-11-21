//! Live registry pulls with download-progress reporting.

use super::*;

#[tokio::test]
#[ignore = "requires outbound network access to Docker Hub"]
async fn real_pull_reports_download_progress_and_runs_container() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let images_dir = dir.path().join("images");
    let (endpoint, svc, handle) = spawn_daemon(state_dir.clone(), images_dir.clone()).await;

    let img_endpoint = endpoint.clone();
    let mut img = ImageServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut rt = RuntimeServiceClient::connect(endpoint).await.unwrap();

    let pull_id = "e2e-progress-1";
    // The reporter the daemon publishes to; the CLI reads the identical file path.
    let reporter = PullProgressReporter::new(&state_dir, pull_id, "alpine:3.20");
    let snapshot_path = reporter.path();

    // Spawn the pull with `coral.progress_id`, then observe the snapshot transition to `done`.
    // A second client is used for the pull so `img` stays available for the `list_images`
    // assertion below (the spawned task takes ownership of its client).
    let mut annotations = HashMap::new();
    annotations.insert("coral.progress_id".into(), pull_id.to_string());
    let pull_endpoint = img_endpoint.clone();
    let pull = tokio::spawn(async move {
        let mut pull_client = ImageServiceClient::connect(pull_endpoint).await.unwrap();
        pull_client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: "alpine:3.20".into(),
                    annotations,
                }),
            })
            .await
            .unwrap()
            .into_inner()
    });

    let terminal = wait_for_terminal(&snapshot_path, Duration::from_secs(120)).await;
    let pulled = pull.await.unwrap();
    assert!(
        pulled.image_ref.starts_with("sha256:"),
        "expected a digest image ref, got {}",
        pulled.image_ref
    );

    // A successful pull removes its snapshot and reports 100% in the final state it published.
    // Re-read the reporter's in-memory snapshot to assert the counters reached completion.
    let final_snapshot = reporter.snapshot();
    assert_eq!(
        final_snapshot.phase,
        PullPhase::Done,
        "reporter must end in the `done` phase ({terminal:?})"
    );
    assert_eq!(final_snapshot.percent, 100);
    assert!(
        final_snapshot.layers_total > 0,
        "alpine:3.20 resolves to at least one layer"
    );
    assert!(
        final_snapshot.bytes_downloaded >= final_snapshot.bytes_total / 2,
        "expected the byte counter to track real streamed bytes \
         (downloaded={}, total={})",
        final_snapshot.bytes_downloaded,
        final_snapshot.bytes_total
    );

    // The image is registered and its rootfs really exists on disk.
    let images = img
        .list_images(ListImagesRequest {})
        .await
        .unwrap()
        .into_inner()
        .images;
    let alpine = images
        .iter()
        .find(|i| i.repo_tags.iter().any(|t| t == "alpine:3.20"))
        .expect("alpine:3.20 must be registered after a successful pull");
    assert!(PathBuf::from(&alpine.rootfs_path).is_dir());
    assert!(
        PathBuf::from(&alpine.rootfs_path).join("etc/alpine-release").is_file(),
        "extracted rootfs must contain /etc/alpine-release"
    );

    // Run a container from the freshly pulled image and check its real output.
    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "real-image-pod".into(),
                    uid: "real-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "real-image-box".into(),
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

    let ctr_id = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "real-alpine".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:3.20".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into(), "-c".into()],
                args: vec!["cat /etc/alpine-release".into()],
                working_dir: "/".into(),
                envs: vec![KeyValue {
                    key: "CORAL_E2E".into(),
                    value: "real-image".into(),
                }],
                labels: HashMap::new(),
                annotations: HashMap::new(),
                log_path: String::new(),
                linux: None,
            }),
            sandbox_config: None,
        })
        .await
        .unwrap()
        .into_inner()
        .container_id;

    rt.start_container(StartContainerRequest {
        container_id: ctr_id.clone(),
    })
    .await
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut final_resp = None;
    while Instant::now() < deadline {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: ctr_id.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if resp.status.as_ref().unwrap().state == ContainerState::ContainerExited as i32 {
            final_resp = Some(resp);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let resp = final_resp.expect("container did not finish in time");
    let status = resp.status.unwrap();
    let log_out = resp.info.get("logOutput").cloned().unwrap_or_default();
    assert_eq!(
        status.exit_code, 0,
        "container from real image failed: {} | {log_out}",
        status.message
    );
    assert!(
        log_out.trim().starts_with("3.20"),
        "expected /etc/alpine-release to report 3.20, got: {log_out:?}"
    );

    svc.shutdown().await;
    handle.abort();
}

#[tokio::test]
#[ignore = "requires outbound network access to Docker Hub"]
async fn real_pull_of_failing_image_reports_failed_phase_with_reason() {
    let dir = tempdir().unwrap();
    let state_dir = dir.path().join("state");
    let images_dir = dir.path().join("images");
    let (endpoint, svc, handle) = spawn_daemon(state_dir.clone(), images_dir).await;

    let mut img = ImageServiceClient::connect(endpoint).await.unwrap();
    let pull_id = "e2e-progress-fail";
    let reporter = PullProgressReporter::new(&state_dir, pull_id, "coral-nonexistent/never:1");
    let snapshot_path = reporter.path();

    let err = img
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: "coral-nonexistent/never:1".into(),
                annotations: HashMap::from([("coral.progress_id".into(), pull_id.to_string())]),
            }),
        })
        .await
        .expect_err("pulling a nonexistent repository must fail");
    assert!(
        err.message().contains("pull image"),
        "unexpected gRPC error: {}",
        err.message()
    );

    // A failed pull leaves a `failed` snapshot behind so the CLI can print the root cause.
    let snap = wait_for_terminal(&snapshot_path, Duration::from_secs(10))
        .await
        .expect("a failed pull must publish a terminal snapshot");
    assert_eq!(snap.phase, PullPhase::Failed);
    assert!(
        !snap.message.trim().is_empty(),
        "the failed snapshot must carry the daemon-side reason"
    );

    // The CLI is responsible for removing the file; verify the shape it would read.
    let raw = std::fs::read_to_string(&snapshot_path).unwrap();
    let parsed: PullProgress = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.pull_id, pull_id);
    assert_eq!(parsed.image, "coral-nonexistent/never:1");

    svc.shutdown().await;
    handle.abort();
}
