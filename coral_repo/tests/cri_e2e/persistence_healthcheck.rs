//! Restart policies, health checks, live logs and metrics.

use super::*;

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
    let mut last_live_debug = String::new();
    let live_deadline = Instant::now() + Duration::from_secs(15);
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
        last_live_debug = format!(
            "state={} exit={} reason={} msg={:?} log={:?} health={:?} metrics={:?}",
            st.state,
            st.exit_code,
            st.reason,
            st.message,
            resp.info.get("logOutput"),
            resp.info.get("healthStatus"),
            resp.info.get("metricsJson")
        );
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
    assert!(
        saw_live_stream,
        "expected STREAM_TICK_1 streamed to disk while still running ({last_live_debug})"
    );
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
