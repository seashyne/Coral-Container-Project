//! CRI v1 gRPC sandbox, container and exec lifecycle.

use super::*;

#[tokio::test]
async fn cri_v1_grpc_full_sandbox_container_and_exec_lifecycle() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists(), "Alpine rootfs must exist");

    let daemon_dir = tempdir().unwrap();
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    });

    // Bind ephemeral local port to avoid port conflicts.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = TcpListenerStream::new(listener);

    let server_handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc.clone()))
            .add_service(ImageServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    let endpoint = format!("http://{addr}");
    let mut rt_client = RuntimeServiceClient::connect(endpoint.clone()).await.unwrap();
    let mut img_client = ImageServiceClient::connect(endpoint).await.unwrap();

    // 1. CRI Version
    let ver = rt_client
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(ver.runtime_name, "coral");
    assert_eq!(ver.runtime_api_version, "v1");

    // 2. CRI ImageService
    let images = img_client
        .list_images(ListImagesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(
        images
            .images
            .iter()
            .any(|i| i.repo_tags.iter().any(|t| t == "alpine:latest"))
    );

    // 3. RunPodSandbox
    let pod_id = rt_client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-e2e-pod".into(),
                    uid: "uid-001".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "coral-cri-pod".into(),
                log_directory: String::new(),
                labels: HashMap::from([("app".into(), "bluecoral".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    let pods = rt_client
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(pods.items.len(), 1);
    assert_eq!(pods.items[0].id, pod_id);

    // 4. CreateContainer
    let ctr_id = rt_client
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "alpine-worker".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into(), "-c".into()],
                args: vec!["echo CRI_OK host=$(hostname) pid=$$ mode=$CORAL_MODE".into()],
                working_dir: "/".into(),
                envs: vec![KeyValue {
                    key: "CORAL_MODE".into(),
                    value: "cri-v1".into(),
                }],
                labels: HashMap::from([("tier".into(), "backend".into())]),
                annotations: HashMap::new(),
                log_path: String::new(),
                linux: Some(LinuxContainerConfig {
                    resources: Some(LinuxContainerResources {
                        cpu_period: 100_000,
                        cpu_quota: 50_000,
                        cpu_shares: 1024,
                        memory_limit_in_bytes: 128 * 1024 * 1024,
                        pids_limit: 64,
                    }),
                }),
            }),
            sandbox_config: None,
        })
        .await
        .unwrap()
        .into_inner()
        .container_id;

    // 5. StartContainer
    rt_client
        .start_container(StartContainerRequest {
            container_id: ctr_id.clone(),
        })
        .await
        .unwrap();

    // 6. Poll ContainerStatus until Exited
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut final_resp = None;
    while Instant::now() < deadline {
        let resp = rt_client
            .container_status(ContainerStatusRequest {
                container_id: ctr_id.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        let st = resp.status.as_ref().unwrap();
        if st.state == ContainerState::ContainerExited as i32 {
            final_resp = Some(resp);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let resp = final_resp.expect("container did not finish in time");
    let status = resp.status.unwrap();
    assert_eq!(status.exit_code, 0, "container failed: {}", status.message);
    let log_out = resp.info.get("logOutput").cloned().unwrap_or_default();
    assert!(
        log_out.contains("CRI_OK host=coral-cri-pod pid=1 mode=cri-v1"),
        "unexpected CRI container log output:\n{log_out}"
    );

    // 7. ExecSync on exited container preserves OverlayFS CoW upper layer!
    let exec_write = rt_client
        .exec_sync(ExecSyncRequest {
            container_id: ctr_id.clone(),
            cmd: vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo EXEC_COW_PERSISTED > /etc/cri-cow-test && cat /etc/alpine-release".into(),
            ],
            timeout: 10,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(exec_write.exit_code, 0);
    assert!(String::from_utf8_lossy(&exec_write.stdout).starts_with("3.20"));

    let exec_read = rt_client
        .exec_sync(ExecSyncRequest {
            container_id: ctr_id.clone(),
            cmd: vec!["/bin/cat".into(), "/etc/cri-cow-test".into()],
            timeout: 10,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(exec_read.exit_code, 0);
    assert_eq!(
        String::from_utf8_lossy(&exec_read.stdout).trim(),
        "EXEC_COW_PERSISTED"
    );

    let diff_resp = rt_client
        .container_status(ContainerStatusRequest {
            container_id: ctr_id.clone(),
            verbose: true,
        })
        .await
        .unwrap()
        .into_inner();
    let overlay_diff = diff_resp
        .info
        .get("overlayDiff")
        .cloned()
        .unwrap_or_default();
    assert!(
        overlay_diff.contains("A /etc/cri-cow-test"),
        "expected overlayDiff to contain A /etc/cri-cow-test, got:\n{overlay_diff}"
    );

    // 8. ListContainers & Cleanup
    let listed = rt_client
        .list_containers(ListContainersRequest { filter: None })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(listed.containers.len(), 1);

    rt_client
        .remove_container(RemoveContainerRequest {
            container_id: ctr_id,
        })
        .await
        .unwrap();
    rt_client
        .stop_pod_sandbox(StopPodSandboxRequest {
            pod_sandbox_id: pod_id.clone(),
        })
        .await
        .unwrap();
    rt_client
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: pod_id,
        })
        .await
        .unwrap();

    server_handle.abort();
}
