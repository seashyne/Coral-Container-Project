use coral_api::image_service_client::ImageServiceClient;
use coral_api::image_service_server::ImageServiceServer;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::runtime_service_server::RuntimeServiceServer;
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ExecSyncRequest, ImageSpec, KeyValue, LinuxContainerConfig,
    LinuxContainerResources, ListContainersRequest, ListImagesRequest, ListPodSandboxRequest,
    PodSandboxConfig, PodSandboxMetadata, PullImageRequest, RemoveContainerRequest,
    RemoveImageRequest, RemovePodSandboxRequest, RunPodSandboxRequest, StartContainerRequest,
    StopPodSandboxRequest, VersionRequest,
};
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

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
async fn cri_compose_up_ps_logs_and_down_e2e() {
    let rootfs = alpine_rootfs();
    let coral_bin = ensure_coral_binary().clone();
    let daemon_dir = tempdir().unwrap();
    let volumes_dir = daemon_dir.path().join("volumes");

    let service = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(service.clone()))
            .add_service(ImageServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let compose_file = daemon_dir.path().join("coral-compose.yml");
    std::fs::write(
        &compose_file,
        r#"
name: e2e-stack
services:
  app:
    image: alpine:3.20
    depends_on:
      - db
    environment:
      DB_HOST: db
    volumes:
      - stack-shared:/shared
    command: /bin/sh -c "echo APP_SEES_DB=$(grep -c db /etc/hosts) ENV=$DB_HOST"
  db:
    image: alpine:3.20
    volumes:
      - stack-shared:/shared
    command: /bin/sh -c "echo DB_BOOTED > /shared/db.flag && echo DB_READY"
volumes:
  stack-shared:
"#,
    )
    .unwrap();

    let endpoint = format!("http://{addr}");

    // 1. `coral compose up`
    let up = tokio::process::Command::new(&coral_bin)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("up")
        .output()
        .await
        .unwrap();
    let up_stdout = String::from_utf8_lossy(&up.stdout);
    let up_stderr = String::from_utf8_lossy(&up.stderr);
    assert!(
        up.status.success(),
        "coral compose up failed:\nstdout:\n{up_stdout}\nstderr:\n{up_stderr}"
    );
    assert!(up_stdout.contains("[db] DB_READY"), "stdout:\n{up_stdout}");
    assert!(
        up_stdout.contains("APP_SEES_DB=1 ENV=db"),
        "expected sibling service host + env in compose up output:\n{up_stdout}"
    );

    // 2. `coral compose ps`
    let ps = tokio::process::Command::new(&coral_bin)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("ps")
        .output()
        .await
        .unwrap();
    assert!(ps.status.success());
    let ps_stdout = String::from_utf8_lossy(&ps.stdout);
    assert!(ps_stdout.contains("db"), "ps:\n{ps_stdout}");
    assert!(ps_stdout.contains("app"), "ps:\n{ps_stdout}");

    // 3. `coral compose logs`
    let logs = tokio::process::Command::new(&coral_bin)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("logs")
        .output()
        .await
        .unwrap();
    assert!(logs.status.success());
    let logs_stdout = String::from_utf8_lossy(&logs.stdout);
    assert!(logs_stdout.contains("DB_READY"), "logs:\n{logs_stdout}");

    // 4. `coral compose down -v`
    let down = tokio::process::Command::new(&coral_bin)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .args(["down", "-v"])
        .output()
        .await
        .unwrap();
    assert!(
        down.status.success(),
        "coral compose down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    let down_stdout = String::from_utf8_lossy(&down.stdout);
    assert!(
        down_stdout.contains("2 container(s), 2 pod(s)"),
        "down:\n{down_stdout}"
    );
    assert!(!volumes_dir.join("stack-shared").exists());

    server_handle.abort();
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

#[tokio::test]
async fn cri_persistence_restart_policy_healthcheck_live_logs_and_metrics_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();

    // --- Daemon Instance #1 ---
    let svc1 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs.clone()),
    });
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let handle1 = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc1.clone()))
            .add_service(ImageServiceServer::new(svc1))
            .serve_with_incoming(TcpListenerStream::new(listener1))
            .await
            .unwrap();
    });

    let mut rt = RuntimeServiceClient::connect(format!("http://{addr1}"))
        .await
        .unwrap();

    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "tier2-pod".into(),
                    uid: "tier2-uid".into(),
                    namespace: "ops".into(),
                    attempt: 0,
                }),
                hostname: "tier2-node".into(),
                log_directory: String::new(),
                labels: HashMap::from([("tier".into(), "2".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    // 1. Live streaming logs + Live Healthcheck + Live Metrics while STILL RUNNING
    let live_ctr = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "live-worker".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec![
                    "-c".into(),
                    "echo STREAM_TICK_1; touch /tmp/ready; sleep 4; echo STREAM_TICK_2".into(),
                ],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.health.cmd".into(), "test -f /tmp/ready".into()),
                    ("coral.health.interval_ms".into(), "200".into()),
                    ("coral.health.retries".into(), "3".into()),
                ]),
                log_path: String::new(),
                linux: Some(LinuxContainerConfig {
                    resources: Some(LinuxContainerResources {
                        cpu_period: 100_000,
                        cpu_quota: 0,
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

    rt.start_container(StartContainerRequest {
        container_id: live_ctr.clone(),
    })
    .await
    .unwrap();

    let mut saw_live_stream = false;
    let mut saw_healthy = false;
    let mut saw_metrics = false;
    let live_deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < live_deadline {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: live_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        let st = resp.status.as_ref().unwrap();
        if st.state == ContainerState::ContainerRunning as i32 {
            if resp
                .info
                .get("logOutput")
                .is_some_and(|l| l.contains("STREAM_TICK_1"))
            {
                saw_live_stream = true;
            }
            if resp.info.get("healthStatus").map(String::as_str) == Some("healthy")
                && st.labels.get("coral.health.status").map(String::as_str) == Some("healthy")
            {
                saw_healthy = true;
            }
            if let Some(mjson) = resp.info.get("metricsJson")
                && let Ok(m) = serde_json::from_str::<serde_json::Value>(mjson)
                && m["pid"].as_i64().unwrap_or(0) > 0
                && m["memoryBytes"].as_u64().unwrap_or(0) > 0
            {
                saw_metrics = true;
            }
            if saw_live_stream && saw_healthy && saw_metrics {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert!(saw_live_stream, "expected STREAM_TICK_1 streamed to disk while still running");
    assert!(saw_healthy, "expected live healthcheck to transition to healthy");
    assert!(saw_metrics, "expected live CPU/Memory/PID metricsJson while running");

    // 2. Auto-Restart Policy (`on-failure:2`)
    let restart_ctr = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "flaky-worker".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "echo FAIL_ATTEMPT; exit 1".into()],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.restart".into(), "on-failure:2".into()),
                    ("coral.restart.backoff_ms".into(), "40".into()),
                ]),
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
        container_id: restart_ctr.clone(),
    })
    .await
    .unwrap();

    let restart_deadline = Instant::now() + Duration::from_secs(15);
    let mut restart_final = None;
    while Instant::now() < restart_deadline {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: restart_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if resp.status.as_ref().unwrap().state == ContainerState::ContainerExited as i32 {
            restart_final = Some(resp);
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    let rf = restart_final.expect("on-failure:2 container did not finish after retries");
    assert_eq!(rf.status.as_ref().unwrap().exit_code, 1);
    assert_eq!(rf.info.get("restartPolicy").map(String::as_str), Some("on-failure:2"));
    assert_eq!(rf.info.get("restartCount").map(String::as_str), Some("2"));
    let attempts = rf
        .info
        .get("logOutput")
        .map(|l| l.matches("FAIL_ATTEMPT").count())
        .unwrap_or(0);
    assert_eq!(attempts, 3, "expected 1 initial run + 2 automatic restarts");

    // Wait for live_ctr to finish cleanly too so we can verify full state persistence.
    let finish_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < finish_deadline {
        let resp = rt
            .container_status(ContainerStatusRequest {
                container_id: live_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if resp.status.as_ref().unwrap().state == ContainerState::ContainerExited as i32 {
            assert!(
                resp.info
                    .get("logOutput")
                    .is_some_and(|l| l.contains("STREAM_TICK_2"))
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }

    // --- 3. Restart coral-daemon on the EXACT SAME state_dir and verify state persisted! ---
    drop(rt);
    handle1.abort();

    let svc2 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    });
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let handle2 = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc2.clone()))
            .add_service(ImageServiceServer::new(svc2))
            .serve_with_incoming(TcpListenerStream::new(listener2))
            .await
            .unwrap();
    });

    let mut rt2 = RuntimeServiceClient::connect(format!("http://{addr2}"))
        .await
        .unwrap();
    let pods = rt2
        .list_pod_sandbox(ListPodSandboxRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .items;
    assert!(pods.iter().any(|p| p.id == pod_id), "pod sandbox must survive daemon restart");

    let ctrs = rt2
        .list_containers(ListContainersRequest { filter: None })
        .await
        .unwrap()
        .into_inner()
        .containers;
    assert!(ctrs.iter().any(|c| c.id == live_ctr), "live_ctr must survive daemon restart");
    assert!(ctrs.iter().any(|c| c.id == restart_ctr), "restart_ctr must survive daemon restart");

    let persisted_status = rt2
        .container_status(ContainerStatusRequest {
            container_id: restart_ctr.clone(),
            verbose: true,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        persisted_status.info.get("restartCount").map(String::as_str),
        Some("2")
    );
    assert!(
        persisted_status
            .info
            .get("logOutput")
            .is_some_and(|l| l.contains("FAIL_ATTEMPT"))
    );

    handle2.abort();
}

#[tokio::test]
async fn cri_boot_auto_resume_inspect_cp_and_system_prune_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();
    let host_work = tempdir().unwrap();

    // --- Daemon #1 ---
    let svc1 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs.clone()),
    });
    let svc1_ctrl = svc1.clone();
    let listener1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr1 = listener1.local_addr().unwrap();
    let handle1 = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc1.clone()))
            .add_service(ImageServiceServer::new(svc1))
            .serve_with_incoming(TcpListenerStream::new(listener1))
            .await
            .unwrap();
    });

    let ep1 = format!("http://{addr1}");
    let mut rt1 = RuntimeServiceClient::connect(ep1.clone()).await.unwrap();

    let pod_id = rt1
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "prod-pod".into(),
                    uid: "prod-uid".into(),
                    namespace: "prod".into(),
                    attempt: 0,
                }),
                hostname: "prod-box".into(),
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

    // 1. Start a long-running container with `unless-stopped`
    let boot_ctr = rt1
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "always-on".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "echo BOOT_MARKER; sleep 30".into()],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.restart".into(), "unless-stopped".into())]),
                log_path: String::new(),
                linux: None,
            }),
            sandbox_config: None,
        })
        .await
        .unwrap()
        .into_inner()
        .container_id;
    rt1.start_container(StartContainerRequest {
        container_id: boot_ctr.clone(),
    })
    .await
    .unwrap();

    // 2. Also start a short container that exits cleanly (for offline cp + prune testing)
    let exited_ctr = rt1
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "one-shot".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "echo DONE_ONCE > /etc/done.txt".into()],
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
        .unwrap()
        .into_inner()
        .container_id;
    rt1.start_container(StartContainerRequest {
        container_id: exited_ctr.clone(),
    })
    .await
    .unwrap();

    // Wait until boot_ctr prints BOOT_MARKER and exited_ctr finishes.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let s1 = rt1
            .container_status(ContainerStatusRequest {
                container_id: boot_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        let s2 = rt1
            .container_status(ContainerStatusRequest {
                container_id: exited_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if s1.info.get("logOutput").is_some_and(|l| l.contains("BOOT_MARKER"))
            && s2.status.as_ref().unwrap().state == ContainerState::ContainerExited as i32
        {
            // Verify inspectJson contains rich metadata
            let inspect: serde_json::Value =
                serde_json::from_str(s1.info.get("inspectJson").expect("inspectJson present"))
                    .unwrap();
            assert_eq!(inspect["restartPolicy"], "unless-stopped");
            assert_eq!(inspect["name"], "always-on");
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }

    // 3. Test `coral cp` both directions on LIVE container AND on EXITED container!
    let src_file = host_work.path().join("payload.txt");
    std::fs::write(&src_file, "HELLO_CORAL_CP_LIVE\n").unwrap();
    let live_out = host_work.path().join("live-out.txt");

    let st_in = tokio::process::Command::new(&coral_bin)
        .arg("--addr")
        .arg(&ep1)
        .arg("cp")
        .arg(&src_file)
        .arg(format!("{boot_ctr}:/tmp/copied.txt"))
        .output()
        .await
        .unwrap();
    assert!(
        st_in.status.success(),
        "live cp host->ctr failed: {}",
        String::from_utf8_lossy(&st_in.stderr)
    );

    let st_out = tokio::process::Command::new(&coral_bin)
        .arg("--addr")
        .arg(&ep1)
        .arg("cp")
        .arg(format!("{boot_ctr}:/tmp/copied.txt"))
        .arg(&live_out)
        .output()
        .await
        .unwrap();
    assert!(
        st_out.status.success(),
        "live cp ctr->host failed: {}",
        String::from_utf8_lossy(&st_out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&live_out).unwrap(),
        "HELLO_CORAL_CP_LIVE\n"
    );

    let offline_out = host_work.path().join("offline-done.txt");
    let st_off = tokio::process::Command::new(&coral_bin)
        .arg("--addr")
        .arg(&ep1)
        .arg("cp")
        .arg(format!("{exited_ctr}:/etc/done.txt"))
        .arg(&offline_out)
        .output()
        .await
        .unwrap();
    assert!(
        st_off.status.success(),
        "offline cp ctr->host failed: {}",
        String::from_utf8_lossy(&st_off.stderr)
    );
    assert_eq!(std::fs::read_to_string(&offline_out).unwrap(), "DONE_ONCE\n");

    // 4. Test `coral system df` and `coral system prune` (must prune exited_ctr, keep boot_ctr!)
    let df_json = rt1
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.system.action".into(), "df".into())]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let df_val: serde_json::Value = serde_json::from_str(&df_json).unwrap();
    assert_eq!(df_val["containersCount"].as_u64(), Some(2));
    assert_eq!(df_val["containersRunning"].as_u64(), Some(1));
    assert_eq!(df_val["containersExited"].as_u64(), Some(1));

    let prune_json = rt1
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.system.action".into(), "prune".into())]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let prune_val: serde_json::Value = serde_json::from_str(&prune_json).unwrap();
    assert_eq!(prune_val["containersRemoved"].as_u64(), Some(1));

    // 5. Simulate unclean host reboot / daemon crash:
    //    Shut down daemon #1 supervisors WITHOUT calling stop_container (so stopped_by_user stays false),
    //    then SIGKILL the underlying container PID, and start Daemon #2 on the SAME state_dir!
    drop(rt1);
    svc1_ctrl.shutdown().await;
    handle1.abort();
    let _ = tokio::process::Command::new(&coral_bin)
        .arg("--root")
        .arg(daemon_dir.path().join("runtime-state"))
        .arg("kill")
        .arg(&boot_ctr)
        .arg("SIGKILL")
        .output()
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let svc2 = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });
    let listener2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let handle2 = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc2.clone()))
            .add_service(ImageServiceServer::new(svc2))
            .serve_with_incoming(TcpListenerStream::new(listener2))
            .await
            .unwrap();
    });

    let mut rt2 = RuntimeServiceClient::connect(format!("http://{addr2}"))
        .await
        .unwrap();

    // Daemon #2 must auto-resume `boot_ctr` (`unless-stopped`) and append a 2nd BOOT_MARKER!
    let resume_deadline = Instant::now() + Duration::from_secs(10);
    let mut resumed_ok = false;
    while Instant::now() < resume_deadline {
        let resp = rt2
            .container_status(ContainerStatusRequest {
                container_id: boot_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        let st = resp.status.as_ref().unwrap();
        let ticks = resp
            .info
            .get("logOutput")
            .map(|l| l.matches("BOOT_MARKER").count())
            .unwrap_or(0);
        let restarts: u32 = resp
            .info
            .get("restartCount")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        if st.state == ContainerState::ContainerRunning as i32 && ticks >= 2 && restarts >= 1 {
            resumed_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert!(
        resumed_ok,
        "expected Daemon #2 to auto-resume 'unless-stopped' container on startup"
    );

    // Cleanly stop the auto-resumed container before exiting test
    let _ = rt2
        .stop_container(coral_api::StopContainerRequest {
            container_id: boot_ctr,
            timeout: 5,
        })
        .await;
    handle2.abort();
}

#[tokio::test]
async fn cri_exec_stream_live_pty_and_custom_network_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();
    let net_file = daemon_dir.path().join("networks.json");
    // SAFETY: isolated per-test network registry file
    unsafe { std::env::set_var("CORAL_NETWORKS_FILE", &net_file) };

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    });
    let svc_ctrl = svc.clone();
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

    let mut rt = RuntimeServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    // 1. Create a user-defined custom network via CRI system action.
    let net_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "network_create".into()),
                    ("coral.network.name".into(), "tier3-net".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let net_val: serde_json::Value = serde_json::from_str(&net_json).unwrap();
    assert_eq!(net_val["name"], "tier3-net");
    assert_eq!(net_val["subnet"], "10.89.1.0/24");

    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "tier3-pod".into(),
                    uid: "tier3-uid".into(),
                    namespace: "tier3".into(),
                    attempt: 0,
                }),
                hostname: "tier3-box".into(),
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

    // 2. Start container A (`db-node`) on `tier3-net`.
    let db_ctr = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "db-node".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "echo DB_UP; sleep 30".into()],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.network".into(), "tier3-net".into())]),
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
        container_id: db_ctr.clone(),
    })
    .await
    .unwrap();

    // 3. Start container B (`app-node`) on the same `tier3-net` and verify `/etc/hosts` resolves `db-node`!
    let app_ctr = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id,
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "app-node".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "cat /etc/hosts".into()],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.network".into(), "tier3-net".into())]),
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
        container_id: app_ctr.clone(),
    })
    .await
    .unwrap();

    // Wait for `db-node` to be alive and `app-node` to exit with `/etc/hosts` printed.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_sibling_host = false;
    let mut db_ready = false;
    while Instant::now() < deadline {
        let s_db = rt
            .container_status(ContainerStatusRequest {
                container_id: db_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if s_db.info.get("logOutput").is_some_and(|l| l.contains("DB_UP")) {
            db_ready = true;
        }
        let s_app = rt
            .container_status(ContainerStatusRequest {
                container_id: app_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        if s_app.status.as_ref().unwrap().state == ContainerState::ContainerExited as i32 {
            let log = s_app.info.get("logOutput").cloned().unwrap_or_default();
            saw_sibling_host = log.contains("db-node");
        }
        if db_ready && saw_sibling_host {
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(db_ready, "db-node should be running");
    assert!(
        saw_sibling_host,
        "app-node on custom network 'tier3-net' must resolve sibling 'db-node' in /etc/hosts"
    );

    // 4. Open a TRUE full-duplex streaming Linux PTY session over CRI gRPC `ExecStream`!
    let (in_tx, in_rx) = tokio::sync::mpsc::channel::<coral_api::ExecStreamRequest>(16);
    in_tx
        .send(coral_api::ExecStreamRequest {
            container_id: db_ctr.clone(),
            cmd: vec!["/bin/sh".into()],
            cols: 100,
            rows: 30,
            stdin: Vec::new(),
            resize: false,
        })
        .await
        .unwrap();

    let mut out_stream = rt
        .exec_stream(tokio_stream::wrappers::ReceiverStream::new(in_rx))
        .await
        .unwrap()
        .into_inner();

    // Send resize + interactive shell commands + clean exit!
    in_tx
        .send(coral_api::ExecStreamRequest {
            container_id: String::new(),
            cmd: Vec::new(),
            cols: 120,
            rows: 40,
            stdin: b"echo PTY_STREAM_LIVE_OK\nexit 0\n".to_vec(),
            resize: true,
        })
        .await
        .unwrap();

    let mut collected = String::new();
    let mut exited = false;
    let mut exit_code = -1;
    let pty_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < pty_deadline {
        match tokio::time::timeout(Duration::from_secs(5), out_stream.message()).await {
            Ok(Ok(Some(frame))) => {
                if !frame.data.is_empty() {
                    collected.push_str(&String::from_utf8_lossy(&frame.data));
                }
                if frame.exited {
                    exited = true;
                    exit_code = frame.exit_code;
                    break;
                }
            }
            _ => break,
        }
    }
    drop(in_tx);
    assert!(exited, "PTY stream should exit cleanly (collected: {collected:?})");
    assert_eq!(exit_code, 0, "PTY shell should exit 0");
    assert!(
        collected.contains("PTY_STREAM_LIVE_OK"),
        "expected PTY_STREAM_LIVE_OK over live ExecStream PTY, got: {collected:?}"
    );

    // Clean up custom network and container.
    let _ = rt
        .stop_container(coral_api::StopContainerRequest {
            container_id: db_ctr,
            timeout: 5,
        })
        .await;
    let _ = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "network_rm".into()),
                    ("coral.network.name".into(), "tier3-net".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap();

    svc_ctrl.shutdown().await;
    handle.abort();
    unsafe { std::env::remove_var("CORAL_NETWORKS_FILE") };
}

#[tokio::test]
async fn cri_v2_wasm_oci_workload_kvm_probe_and_ai_sandbox_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();
    let images_dir = daemon_dir.path().join("images");
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: Some(images_dir.clone()),
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });
    let svc_ctrl = svc.clone();
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

    let mut rt = RuntimeServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    // 1. Probe host /dev/kvm via CRI (`coral.system.action = "vmm_probe"`).
    let probe_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.system.action".into(), "vmm_probe".into())]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let probe_val: serde_json::Value = serde_json::from_str(&probe_json).unwrap();
    assert!(probe_val["capabilities"]["arch"].is_string());
    if probe_val["capabilities"]["available"].as_bool() == Some(true) {
        assert_eq!(
            probe_val["selftest"]["serialOutput"].as_str(),
            Some("CORAL_KVM_VMM_64BIT_OK\n")
        );
        assert_eq!(probe_val["selftest"]["exitCode"].as_i64(), Some(0));
    }

    // 2. Run inline WASI Preview 1 WebAssembly workload via CRI (`coral.system.action = "wasm_run"`).
    let wat_src = r#"
    (module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "\10\00\00\00\13\00\00\00")
      (data (i32.const 16) "CORAL_WASM_CRI_OK!\n")
      (func (export "_start")
        (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 64)))))
    "#;
    let wasm_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "wasm_run".into()),
                    ("coral.wasm.source".into(), wat_src.into()),
                    ("coral.wasm.fuel".into(), "1000000".into()),
                    ("coral.wasm.memory_mb".into(), "8".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let wasm_val: serde_json::Value = serde_json::from_str(&wasm_json).unwrap();
    assert_eq!(wasm_val["exitCode"].as_i64(), Some(0));
    assert_eq!(wasm_val["stdout"].as_str(), Some("CORAL_WASM_CRI_OK!\n"));
    assert!(wasm_val["fuelConsumed"].as_u64().unwrap_or(0) > 0);

    // 3. Pack a `.wat` file as a Coral OCI Wasm image (`coral wasm pack`) and run it as a CRI container!
    let wat_file = daemon_dir.path().join("hello.wat");
    std::fs::write(&wat_file, wat_src).unwrap();
    let pack_out = tokio::process::Command::new(&coral_bin)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("wasm")
        .arg("pack")
        .arg(&wat_file)
        .arg("-t")
        .arg("hello-wasm:v1")
        .output()
        .await
        .unwrap();
    assert!(
        pack_out.status.success(),
        "coral wasm pack failed: {}",
        String::from_utf8_lossy(&pack_out.stderr)
    );
    let wasm_run_cli = tokio::process::Command::new(&coral_bin)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("run")
        .arg("--runtime")
        .arg("wasm")
        .arg("--image")
        .arg("hello-wasm:v1")
        .output()
        .await
        .unwrap();
    assert!(wasm_run_cli.status.success());
    assert_eq!(
        String::from_utf8_lossy(&wasm_run_cli.stdout),
        "CORAL_WASM_CRI_OK!\n"
    );

    // 4. Test AI Agent Untrusted Code Sandbox (`coral.system.action = "sandbox_run"`):
    //    4a. Normal command runs air-gapped and returns structured JSON.
    let sbx_ok_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "sandbox_run".into()),
                    ("coral.sandbox.image".into(), "alpine:latest".into()),
                    ("coral.sandbox.cmd".into(), "echo AGENT_SANDBOX_SAFE".into()),
                    ("coral.sandbox.timeout".into(), "5".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let sbx_ok: serde_json::Value = serde_json::from_str(&sbx_ok_json).unwrap();
    assert_eq!(sbx_ok["exitCode"].as_i64(), Some(0));
    assert_eq!(sbx_ok["timedOut"].as_bool(), Some(false));
    assert!(
        sbx_ok["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("AGENT_SANDBOX_SAFE")
    );

    //    4b. Runaway command (`sleep 30`) is deterministically killed by the 1s sandbox timeout!
    let sbx_to_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "sandbox_run".into()),
                    ("coral.sandbox.image".into(), "alpine:latest".into()),
                    ("coral.sandbox.cmd".into(), "sleep 30".into()),
                    ("coral.sandbox.timeout".into(), "1".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let sbx_to: serde_json::Value = serde_json::from_str(&sbx_to_json).unwrap();
    assert_eq!(sbx_to["timedOut"].as_bool(), Some(true));
    assert_eq!(sbx_to["exitCode"].as_i64(), Some(124));

    svc_ctrl.shutdown().await;
    handle.abort();
}
