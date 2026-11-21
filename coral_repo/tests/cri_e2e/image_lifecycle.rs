//! Image pull, scratch-image persistence and cleanup.

use super::*;

#[tokio::test]
async fn cri_image_scratch_persistence_and_cleanup() {
    let coral_bin = ensure_coral_binary().clone();
    let daemon_dir = tempdir().unwrap();
    let images_dir = daemon_dir.path().join("custom-images");

    // 1. Start daemon #1 and pull `scratch`
    let svc1 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: Some(images_dir.clone()),
        coral_bin: coral_bin.clone(),
        default_rootfs: None,
    });
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let h1 = tokio::spawn(async move {
        Server::builder()
            .add_service(ImageServiceServer::new(svc1))
            .serve_with_incoming(TcpListenerStream::new(listener1))
            .await
            .unwrap();
    });

    let mut client1 = ImageServiceClient::connect(format!("http://{addr1}"))
        .await
        .unwrap();
    let pulled = client1
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: "scratch".into(),
                annotations: HashMap::new(),
            }),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(pulled.image_ref, "sha256:coral-scratch");
    h1.abort();

    // 2. Restart daemon #2 pointing at the same images_dir -> `scratch` must persist
    let svc2 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: Some(images_dir.clone()),
        coral_bin,
        default_rootfs: None,
    });
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let h2 = tokio::spawn(async move {
        Server::builder()
            .add_service(ImageServiceServer::new(svc2))
            .serve_with_incoming(TcpListenerStream::new(listener2))
            .await
            .unwrap();
    });

    let mut client2 = ImageServiceClient::connect(format!("http://{addr2}"))
        .await
        .unwrap();
    let list = client2
        .list_images(ListImagesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.images.len(), 1);
    assert!(list.images[0].repo_tags.iter().any(|t| t == "scratch"));
    let scratch_path = std::path::PathBuf::from(&list.images[0].rootfs_path);
    assert!(scratch_path.is_dir());

    // 3. Remove `scratch` -> managed directory is cleaned up
    client2
        .remove_image(RemoveImageRequest {
            image: Some(ImageSpec {
                image: "scratch".into(),
                annotations: HashMap::new(),
            }),
        })
        .await
        .unwrap();
    assert!(!scratch_path.exists());
    let list_after = client2
        .list_images(ListImagesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(list_after.images.is_empty());
    h2.abort();
}

#[tokio::test]
#[ignore = "requires outbound network access to Docker Hub"]
async fn cri_live_pull_and_run_linux_base_images() {
    let coral_bin = ensure_coral_binary().clone();
    let daemon_dir = tempdir().unwrap();
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin,
        default_rootfs: None,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc.clone()))
            .add_service(ImageServiceServer::new(svc))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let endpoint = format!("http://{addr}");
    let mut rt = RuntimeServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut img = ImageServiceClient::connect(endpoint).await.unwrap();

    for (image_ref, cmd, args, expected_substr) in [
        (
            "busybox:musl",
            vec!["/bin/sh".into(), "-c".into()],
            vec!["busybox | head -n 1".into()],
            "BusyBox",
        ),
        (
            "debian:bookworm-slim",
            vec!["/bin/sh".into(), "-c".into()],
            vec!["cat /etc/os-release".into()],
            "Debian GNU/Linux 12 (bookworm)",
        ),
    ] {
        img.pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: image_ref.into(),
                annotations: HashMap::new(),
            }),
        })
        .await
        .unwrap();

        let pod_id = rt
            .run_pod_sandbox(RunPodSandboxRequest {
                config: Some(PodSandboxConfig {
                    metadata: Some(PodSandboxMetadata {
                        name: format!("pod-{}", image_ref.replace(':', "-")),
                        uid: image_ref.into(),
                        namespace: "test".into(),
                        attempt: 0,
                    }),
                    hostname: "coral-base-test".into(),
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
                pod_sandbox_id: pod_id,
                config: Some(ContainerConfig {
                    metadata: Some(ContainerMetadata {
                        name: "base-check".into(),
                        attempt: 0,
                    }),
                    image: Some(ImageSpec {
                        image: image_ref.into(),
                        annotations: HashMap::new(),
                    }),
                    command: cmd,
                    args,
                    working_dir: "/".into(),
                    envs: vec![],
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
        let resp = final_resp.expect("container did not finish");
        let st = resp.status.unwrap();
        let log_out = resp.info.get("logOutput").cloned().unwrap_or_default();
        assert_eq!(st.exit_code, 0, "{image_ref} failed: {} | {log_out}", st.message);
        assert!(
            log_out.contains(expected_substr),
            "{image_ref} unexpected output: {log_out}"
        );
        println!("[OK] {image_ref} -> {}", log_out.lines().next().unwrap_or(""));
    }

    handle.abort();
}
