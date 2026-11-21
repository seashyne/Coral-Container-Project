//! TCP and UDP port forwarding across a bridge network.

use super::*;

#[test]
fn port_forwarding_relays_tcp_traffic_to_container() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    let host_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let ctr_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine-port:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    let mut child = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-port:3.20",
            "--name",
            "port-ctr",
            "-p",
            &format!("127.0.0.1:{host_port}:{ctr_port}"),
            "--",
            "/bin/sh",
            "-c",
            &format!("echo PORT_RELAY_OK | nc -l -p {ctr_port} -w 2"),
        ])
        .spawn()
        .expect("spawn container with -p");

    let mut received = String::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", host_port)) {
            use std::io::Read;
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.read_to_string(&mut received);
            if received.contains("PORT_RELAY_OK") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(40));
    }

    let status = child.wait().expect("wait port-ctr");
    assert!(status.success());
    assert!(
        received.contains("PORT_RELAY_OK"),
        "expected PORT_RELAY_OK via forwarded port, got: {received:?}"
    );
}

#[test]
fn udp_port_forwarding_bridge_and_multistage_dockerfile_build() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Real two-way UDP port forwarding across isolated `--network bridge` (CLONE_NEWNET).
    let host_udp_port = {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.local_addr().unwrap().port()
    };
    let ctr_child = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--pull",
            "never",
            "--network",
            "bridge",
            "-p",
            &format!("127.0.0.1:{host_udp_port}:9999/udp"),
            "--name",
            "udp-bridge-ctr",
            "alpine:3.20",
            "/bin/sh",
            "-c",
            "printf 'UDP_PONG_OK\\n' | nc -u -l -p 9999 -w 2",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    let client = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(250)))
        .unwrap();
    let target: std::net::SocketAddr = format!("127.0.0.1:{host_udp_port}").parse().unwrap();
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut received = String::new();
    let mut rbuf = [0u8; 128];
    while Instant::now() < deadline {
        let _ = client.send_to(b"PING_TO_UDP\n", target);
        if let Ok((n, _)) = client.recv_from(&mut rbuf) {
            received = String::from_utf8_lossy(&rbuf[..n]).into_owned();
            if received.contains("UDP_PONG_OK") {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    let ctr_out = ctr_child.wait_with_output().unwrap();
    let ctr_stdout = String::from_utf8_lossy(&ctr_out.stdout);
    let ctr_stderr = String::from_utf8_lossy(&ctr_out.stderr);
    assert!(
        received.contains("UDP_PONG_OK"),
        "expected UDP_PONG_OK on host client, got '{received}'\nctr_stdout={ctr_stdout}\nctr_stderr={ctr_stderr}"
    );
    assert!(
        ctr_stdout.contains("PING_TO_UDP"),
        "expected PING_TO_UDP inside bridge container, got '{ctr_stdout}'"
    );

    // 2. Multi-Stage Dockerfile build (`FROM ... AS builder`, `COPY --from=builder`,
    // `--build-arg`, and `--target builder`).
    let ctx_dir = work.path().join("ms-ctx");
    std::fs::create_dir_all(&ctx_dir).unwrap();
    std::fs::write(ctx_dir.join("input.txt"), "CTX_INPUT").unwrap();
    std::fs::write(
        ctx_dir.join("Dockerfile"),
        r#"
ARG ALPINE_VER=3.20
FROM alpine:${ALPINE_VER} AS builder
ARG ARTIFACT_MSG=DEFAULT_MSG
WORKDIR /src
COPY input.txt /src/input.txt
RUN mkdir -p /out && echo "${ARTIFACT_MSG}:$(cat /src/input.txt)" > /out/artifact.txt && echo SECRET_BUILD_ONLY > /var/build_only.txt

FROM alpine:3.20 AS runtime
WORKDIR /app
COPY --from=builder /out/artifact.txt /app/artifact.txt
CMD ["/bin/cat", "/app/artifact.txt"]
"#,
    )
    .unwrap();

    let build_runtime = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "build",
            "--local",
            "--no-cache",
            "--build-arg",
            "ARTIFACT_MSG=STAGE1_TO_STAGE2_OK",
            "-t",
            "ms-app:v1",
        ])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build_runtime.status.success(),
        "multistage build failed:\n{}",
        String::from_utf8_lossy(&build_runtime.stderr)
    );

    let run_ms = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--pull",
            "never",
            "ms-app:v1",
            "/bin/sh",
            "-c",
            "cat /app/artifact.txt && test ! -e /var/build_only.txt && echo STAGE_ISOLATED_OK",
        ])
        .output()
        .unwrap();
    let run_ms_out = String::from_utf8_lossy(&run_ms.stdout);
    assert!(run_ms.status.success(), "{run_ms_out}");
    assert!(run_ms_out.contains("STAGE1_TO_STAGE2_OK:CTX_INPUT"));
    assert!(run_ms_out.contains("STAGE_ISOLATED_OK"));

    // `--target builder` stops after builder stage and preserves `/var/build_only.txt`.
    let build_target = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "build",
            "--local",
            "--target",
            "builder",
            "--build-arg",
            "ARTIFACT_MSG=TARGET_BUILDER_OK",
            "-t",
            "ms-builder:v1",
        ])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build_target.status.success(),
        "target builder failed:\n{}",
        String::from_utf8_lossy(&build_target.stderr)
    );

    let run_target = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--pull",
            "never",
            "ms-builder:v1",
            "/bin/sh",
            "-c",
            "cat /out/artifact.txt && cat /var/build_only.txt",
        ])
        .output()
        .unwrap();
    let run_target_out = String::from_utf8_lossy(&run_target.stdout);
    assert!(run_target.status.success(), "{run_target_out}");
    assert!(run_target_out.contains("TARGET_BUILDER_OK:CTX_INPUT"));
    assert!(run_target_out.contains("SECRET_BUILD_ONLY"));
}
