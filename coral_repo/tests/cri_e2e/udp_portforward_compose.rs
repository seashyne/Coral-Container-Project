//! Daemon UDP port forwarding and full compose lifecycle.

use super::*;

#[tokio::test]
async fn daemon_udp_port_forward_and_full_compose_lifecycle_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();
    let images_dir = daemon_dir.path().join("images");
    fs::create_dir_all(&images_dir).unwrap();
    // Seed `alpine:3.20` into the temporary daemon image store via `coral pull --local`.
    let seed_bin = coral_bin.clone();
    let seed_images = images_dir.clone();
    let seed_rootfs = rootfs.clone();
    tokio::task::spawn_blocking(move || {
        let st = std::process::Command::new(&seed_bin)
            .arg("--images-dir")
            .arg(&seed_images)
            .args(["pull", "alpine:3.20", "--local", "--rootfs"])
            .arg(&seed_rootfs)
            .status()
            .unwrap();
        assert!(st.success());
    })
    .await
    .unwrap();

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: Some(images_dir.clone()),
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });
    let svc_ctrl = svc.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let tcp_addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc.clone()))
            .add_service(ImageServiceServer::new(svc))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let daemon_url = format!("http://{tcp_addr}");
    let mut rt = RuntimeServiceClient::connect(daemon_url.clone())
        .await
        .unwrap();

    // 1. Daemon Tokio UDP port forwarder across `--network bridge` (`CLONE_NEWNET`).
    let udp_host_port = {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.local_addr().unwrap().port()
    };
    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "udp-daemon-pod".into(),
                    uid: "udp-daemon-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "udp-daemon-box".into(),
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

    let udp_cid = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "udp-daemon-ctr".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:3.20".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec![
                    "-c".into(),
                    "printf 'DAEMON_UDP_PONG\\n' | nc -u -l -p 9998 -w 3".into(),
                ],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.network".into(), "bridge".into()),
                    (
                        "coral.ports".into(),
                        format!("127.0.0.1:{udp_host_port}:9998/udp"),
                    ),
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
        container_id: udp_cid.clone(),
    })
    .await
    .unwrap();

    let mut udp_reply = String::new();
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target: std::net::SocketAddr = format!("127.0.0.1:{udp_host_port}").parse().unwrap();
        let _ = client.send_to(b"DAEMON_UDP_PING\n", target).await;
        let mut buf = [0u8; 128];
        if let Ok(Ok((n, _))) =
            tokio::time::timeout(Duration::from_millis(200), client.recv_from(&mut buf)).await
        {
            udp_reply = String::from_utf8_lossy(&buf[..n]).into_owned();
            if udp_reply.contains("DAEMON_UDP_PONG") {
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    }
    assert!(
        udp_reply.contains("DAEMON_UDP_PONG"),
        "expected DAEMON_UDP_PONG from daemon UDP port forwarder, got '{udp_reply}'"
    );
    let _ = rt
        .stop_container(StopContainerRequest {
            container_id: udp_cid.clone(),
            timeout: 2,
        })
        .await;
    let _ = rt
        .remove_container(RemoveContainerRequest {
            container_id: udp_cid,
        })
        .await;
    let _ = rt
        .remove_pod_sandbox(RemovePodSandboxRequest {
            pod_sandbox_id: pod_id,
        })
        .await;

    // 2. Full `coral compose` CLI stack lifecycle against the daemon:
    // `.env` interpolation, `env_file`, Multi-Stage `build.target` + `build.args`,
    // `depends_on` (`service_completed_successfully` + `service_healthy`), `entrypoint` + `command`,
    // `profiles`, `exec`, `ps`, `logs`, `stop`/`start`/`restart`, and `down -v`.
    let stack_dir = daemon_dir.path().join("stack");
    let app_ctx = stack_dir.join("app");
    fs::create_dir_all(&app_ctx).unwrap();
    fs::write(
        app_ctx.join("Dockerfile"),
        r#"
FROM alpine:3.20 AS builder
ARG STAGE_TAG=unset
RUN mkdir -p /out && echo "$STAGE_TAG" > /out/built.txt

FROM alpine:3.20 AS runtime
COPY --from=builder /out/built.txt /app/built.txt
"#,
    )
    .unwrap();

    fs::write(stack_dir.join(".env"), "ALPINE_VER=3.20\nSTAGE_VAL=COMPOSE_STAGE_V2\n").unwrap();
    fs::write(
        stack_dir.join("svc.env"),
        "FROM_FILE=ENV_FILE_OK\nOVERRIDE_ME=from_file\n",
    )
    .unwrap();
    let compose_file = stack_dir.join("docker-compose.yml");
    fs::write(
        &compose_file,
        r#"
name: fullcompose
services:
  init-step:
    image: alpine:${ALPINE_VER}
    entrypoint: ["/bin/sh", "-c"]
    command: ["echo INIT_DONE_OK"]
  db:
    image: alpine:${ALPINE_VER}
    command: ["/bin/sh", "-c", "touch /tmp/db_ready && sleep 30"]
    healthcheck:
      test: ["CMD-SHELL", "test -f /tmp/db_ready"]
      interval: 500ms
      retries: 3
  app:
    build:
      context: ./app
      dockerfile: Dockerfile
      target: runtime
      args:
        STAGE_TAG: ${STAGE_VAL}
    env_file:
      - ./svc.env
    environment:
      OVERRIDE_ME: from_compose
    depends_on:
      init-step:
        condition: service_completed_successfully
      db:
        condition: service_healthy
    command: ["/bin/sh", "-c", "sleep 30"]
  optional-tool:
    image: alpine:3.20
    profiles: ["debug"]
    command: ["/bin/sleep", "30"]
"#,
    )
    .unwrap();

    let run_cli = |extra: &[&str]| {
        let bin = coral_bin.clone();
        let addr = daemon_url.clone();
        let sr = daemon_dir.path().to_path_buf();
        let img = images_dir.clone();
        let cf = compose_file.clone();
        let args: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
        tokio::task::spawn_blocking(move || {
            std::process::Command::new(&bin)
                .arg("--addr")
                .arg(&addr)
                .arg("--root")
                .arg(&sr)
                .arg("--images-dir")
                .arg(&img)
                .arg("compose")
                .arg("-f")
                .arg(&cf)
                .args(&args)
                .output()
                .unwrap()
        })
    };

    let cfg_out = run_cli(&["config"]).await.unwrap();
    assert!(cfg_out.status.success());
    let cfg_txt = String::from_utf8_lossy(&cfg_out.stdout);
    assert!(cfg_txt.contains("COMPOSE_STAGE_V2"), "{cfg_txt}");

    let up_out = run_cli(&["up", "-d", "--build"]).await.unwrap();
    let up_err = String::from_utf8_lossy(&up_out.stderr);
    let up_stdout = String::from_utf8_lossy(&up_out.stdout);
    assert!(
        up_out.status.success(),
        "compose up failed:\nstdout:\n{up_stdout}\nstderr:\n{up_err}"
    );
    assert!(up_stdout.contains("app"), "{up_stdout}");
    assert!(!up_stdout.contains("optional-tool"), "{up_stdout}");

    let ps_out = run_cli(&["ps", "-a"]).await.unwrap();
    let ps_txt = String::from_utf8_lossy(&ps_out.stdout);
    assert!(ps_out.status.success());
    assert!(ps_txt.contains("init-step"), "{ps_txt}");
    assert!(ps_txt.contains("db"), "{ps_txt}");
    assert!(ps_txt.contains("app"), "{ps_txt}");

    let exec_out = run_cli(&[
        "exec",
        "app",
        "--",
        "/bin/sh",
        "-c",
        "echo BUILT=$(cat /app/built.txt) F=$FROM_FILE O=$OVERRIDE_ME",
    ])
    .await
    .unwrap();
    let exec_txt = String::from_utf8_lossy(&exec_out.stdout);
    assert!(
        exec_out.status.success(),
        "compose exec failed: {}",
        String::from_utf8_lossy(&exec_out.stderr)
    );
    assert!(
        exec_txt.contains("BUILT=COMPOSE_STAGE_V2 F=ENV_FILE_OK O=from_compose"),
        "unexpected exec output: {exec_txt}"
    );

    let logs_out = run_cli(&["logs", "init-step"]).await.unwrap();
    let logs_txt = String::from_utf8_lossy(&logs_out.stdout);
    assert!(logs_txt.contains("INIT_DONE_OK"), "logs={logs_txt}");

    assert!(run_cli(&["stop", "-t", "2", "app"]).await.unwrap().status.success());
    assert!(run_cli(&["start", "app"]).await.unwrap().status.success());
    assert!(run_cli(&["restart", "-t", "2", "app"]).await.unwrap().status.success());

    let down_out = run_cli(&["down", "-v"]).await.unwrap();
    assert!(down_out.status.success());

    svc_ctrl.shutdown().await;
    server_handle.abort();
}
