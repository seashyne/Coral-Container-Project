//! Exec, streaming, live PTY and custom networking.

use super::*;

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
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut saw_sibling_host = false;
    let mut db_ready = false;
    let mut last_app_state = -1;
    let mut last_app_log = String::new();
    let mut last_db_debug = String::new();
    while Instant::now() < deadline {
        let s_db = rt
            .container_status(ContainerStatusRequest {
                container_id: db_ctr.clone(),
                verbose: true,
            })
            .await
            .unwrap()
            .into_inner();
        let db_st = s_db.status.as_ref().unwrap();
        last_db_debug = format!(
            "state={} exit={} reason={} msg={:?} log={:?}",
            db_st.state,
            db_st.exit_code,
            db_st.reason,
            db_st.message,
            s_db.info.get("logOutput")
        );
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
        let st = s_app.status.as_ref().unwrap();
        last_app_state = st.state;
        last_app_log = s_app.info.get("logOutput").cloned().unwrap_or_default();
        if st.state == ContainerState::ContainerExited as i32 {
            saw_sibling_host = last_app_log.contains("db-node");
        }
        if db_ready && saw_sibling_host {
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(db_ready, "db-node should be running ({last_db_debug})");
    assert!(
        saw_sibling_host,
        "app-node on custom network 'tier3-net' must resolve sibling 'db-node' in /etc/hosts (last_app_state={last_app_state}, log={last_app_log:?})"
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
    let pty_deadline = Instant::now() + Duration::from_secs(20);
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
