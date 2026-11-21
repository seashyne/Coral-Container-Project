//! Boot, auto-resume, inspect, cp and system prune.

use super::*;

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
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut ready_for_cp = false;
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
            ready_for_cp = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(ready_for_cp, "timed out waiting for boot_ctr and exited_ctr before cp");

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
    let resume_deadline = Instant::now() + Duration::from_secs(25);
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
