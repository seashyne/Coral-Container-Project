//! Production Docker-parity E2E tests: Bind & Named Volumes (`coral volume`), DNS (`--dns`, `--add-host`),
//! Interactive PTY (`-t`), Seccomp-BPF + Cap Drop, TCP Port Forwarding (`-p`), Isolated Bridge NetNS
//! (`--network bridge` Cross-NetNS multi-container port proxy), `coral diff` + `coral commit`, and `coral build`.

use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

#[test]
fn volumes_dns_pty_and_security_hardening() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");
    let host_rw = work.path().join("vol-rw");
    let host_ro = work.path().join("vol-ro");
    std::fs::create_dir_all(&host_rw).unwrap();
    std::fs::create_dir_all(&host_ro).unwrap();
    std::fs::write(host_ro.join("seed.txt"), "READONLY_SEED\n").unwrap();

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine-sec:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Bind volumes (rw + ro), DNS resolv.conf, Seccomp (mode 2), and CAP_SYS_ADMIN dropped.
    let out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-sec:3.20",
            "--name",
            "sec-vol-ctr",
            "-v",
            &format!("{}:/mnt/rw", host_rw.display()),
            "-v",
            &format!("{}:/mnt/ro:ro", host_ro.display()),
            "--",
            "/bin/sh",
            "-c",
            concat!(
                "echo FROM_CTR > /mnt/rw/out.txt; ",
                "echo SEED=$(cat /mnt/ro/seed.txt); ",
                "if touch /mnt/ro/nope 2>/dev/null; then echo RO_FAIL; else echo RO_OK; fi; ",
                "echo DNS=$(test -s /etc/resolv.conf && echo yes || echo no); ",
                "grep -E '^(Seccomp|CapEff):' /proc/self/status"
            ),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "sec-vol-ctr failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(host_rw.join("out.txt")).unwrap().trim(),
        "FROM_CTR"
    );
    assert!(stdout.contains("SEED=READONLY_SEED"), "stdout:\n{stdout}");
    assert!(stdout.contains("RO_OK"), "ro mount was writable!\n{stdout}");
    assert!(stdout.contains("DNS=yes"), "DNS resolv.conf missing!\n{stdout}");
    assert!(
        stdout.contains("Seccomp:\t2"),
        "expected Seccomp mode 2 (BPF), got:\n{stdout}"
    );
    // Verify CAP_SYS_ADMIN (bit 21 = 0x200000) is cleared in CapEff.
    let cap_line = stdout
        .lines()
        .find(|l| l.starts_with("CapEff:"))
        .expect("CapEff line missing");
    let cap_hex = cap_line.split_whitespace().nth(1).unwrap();
    let cap_bits = u64::from_str_radix(cap_hex, 16).unwrap();
    assert_eq!(
        cap_bits & (1u64 << 21),
        0,
        "CAP_SYS_ADMIN must be dropped, got CapEff={cap_hex}"
    );

    // 2. PTY (`-t`) gives real TTY file descriptors inside container.
    let pty_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "-t",
            "--image",
            "alpine-sec:3.20",
            "--name",
            "pty-ctr",
            "--",
            "/bin/sh",
            "-c",
            "test -t 0 && test -t 1 && echo PTY_ACTIVE",
        ])
        .output()
        .unwrap();
    assert!(
        pty_out.status.success(),
        "pty-ctr failed: {}",
        String::from_utf8_lossy(&pty_out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&pty_out.stdout).contains("PTY_ACTIVE"),
        "expected PTY_ACTIVE, got: {}",
        String::from_utf8_lossy(&pty_out.stdout)
    );

    // 3. `--network none` isolates container into loopback-only NetNS.
    let net_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--network",
            "none",
            "--image",
            "alpine-sec:3.20",
            "--name",
            "netnone-ctr",
            "--",
            "/bin/sh",
            "-c",
            "ls /sys/class/net 2>/dev/null || cat /proc/net/dev",
        ])
        .output()
        .unwrap();
    assert!(net_out.status.success());
    let net_str = String::from_utf8_lossy(&net_out.stdout);
    assert!(
        net_str.contains("lo"),
        "expected loopback in --network none:\n{net_str}"
    );
    assert!(
        !net_str.contains("eth0") && !net_str.contains("enp"),
        "host NIC leaked into --network none:\n{net_str}"
    );
}

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
fn overlayfs_diff_and_commit_creates_new_runnable_image() {
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

    // 1. Run container with --keep that adds, modifies, and deletes files in its CoW upper layer.
    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--keep",
            "--image",
            "alpine:3.20",
            "--name",
            "cow-commit-ctr",
            "--",
            "/bin/sh",
            "-c",
            "echo COMMITTED_V1 > /etc/coral-committed.txt && echo MODIFIED > /etc/issue && rm -f /etc/motd",
        ])
        .output()
        .unwrap();
    assert!(
        run_out.status.success(),
        "cow-commit-ctr failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    // 2. Inspect `coral diff --local cow-commit-ctr`.
    let diff_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["diff", "--local", "cow-commit-ctr"])
        .output()
        .unwrap();
    assert!(diff_out.status.success());
    let diff_txt = String::from_utf8_lossy(&diff_out.stdout);
    assert!(
        diff_txt.contains("A /etc/coral-committed.txt"),
        "expected added file in diff:\n{diff_txt}"
    );
    assert!(
        diff_txt.contains("C /etc/issue"),
        "expected modified file in diff:\n{diff_txt}"
    );
    assert!(
        diff_txt.contains("D /etc/motd"),
        "expected deleted whiteout in diff:\n{diff_txt}"
    );

    // 3. Commit the container into a new image `committed-alpine:v1`.
    let commit_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["commit", "--local", "cow-commit-ctr", "committed-alpine:v1"])
        .output()
        .unwrap();
    assert!(
        commit_out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&commit_out.stderr)
    );

    // 4. Run a fresh container from `committed-alpine:v1` and verify changes persisted while base rootfs stayed untouched.
    let verify_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "committed-alpine:v1",
            "--name",
            "from-commit-ctr",
            "--",
            "/bin/sh",
            "-c",
            "cat /etc/coral-committed.txt && (test ! -e /etc/motd && echo MOTD_GONE)",
        ])
        .output()
        .unwrap();
    assert!(verify_out.status.success());
    let v_stdout = String::from_utf8_lossy(&verify_out.stdout);
    assert!(v_stdout.contains("COMMITTED_V1"), "stdout:\n{v_stdout}");
    assert!(v_stdout.contains("MOTD_GONE"), "stdout:\n{v_stdout}");
    assert!(!rootfs.join("etc/coral-committed.txt").exists());
    assert!(rootfs.join("etc/motd").exists());
}

#[test]
fn named_volumes_and_custom_dns_add_host() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");
    let volumes_dir = work.path().join("volumes");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Create named volume `coral-db-vol`.
    let create_vol = Command::new(coral)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["volume", "create", "coral-db-vol"])
        .output()
        .unwrap();
    assert!(create_vol.status.success());
    assert_eq!(
        String::from_utf8_lossy(&create_vol.stdout).trim(),
        "coral-db-vol"
    );

    // 2. Write data to named volume in Container #1 + verify `--dns` and `--add-host`.
    let c1 = Command::new(coral)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine:3.20",
            "--name",
            "vol-writer",
            "--hostname",
            "writer-node",
            "-v",
            "coral-db-vol:/var/lib/db",
            "--dns",
            "1.1.1.1",
            "--dns",
            "9.9.9.9",
            "--add-host",
            "cache.internal:10.77.0.5",
            "--",
            "/bin/sh",
            "-c",
            "echo PERSISTED_ROW_42 > /var/lib/db/rows.txt && cat /etc/resolv.conf && cat /etc/hosts",
        ])
        .output()
        .unwrap();
    assert!(
        c1.status.success(),
        "vol-writer failed: {}",
        String::from_utf8_lossy(&c1.stderr)
    );
    let c1_out = String::from_utf8_lossy(&c1.stdout);
    assert!(c1_out.contains("nameserver 1.1.1.1"), "out:\n{c1_out}");
    assert!(c1_out.contains("nameserver 9.9.9.9"), "out:\n{c1_out}");
    assert!(
        c1_out.contains("127.0.0.1\tlocalhost writer-node"),
        "out:\n{c1_out}"
    );
    assert!(
        c1_out.contains("10.77.0.5\tcache.internal"),
        "out:\n{c1_out}"
    );

    // 3. Read persisted named volume in Container #2 as read-only.
    let c2 = Command::new(coral)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine:3.20",
            "--name",
            "vol-reader",
            "-v",
            "coral-db-vol:/data:ro",
            "--",
            "/bin/cat",
            "/data/rows.txt",
        ])
        .output()
        .unwrap();
    assert!(c2.status.success());
    assert_eq!(
        String::from_utf8_lossy(&c2.stdout).trim(),
        "PERSISTED_ROW_42"
    );

    // 4. Remove named volume.
    let rm_vol = Command::new(coral)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["volume", "rm", "coral-db-vol"])
        .output()
        .unwrap();
    assert!(rm_vol.status.success());
}

#[test]
fn bridge_network_isolates_netns_and_forwards_same_internal_port() {
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

    let host_port_1 = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let host_port_2 = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };

    // Both containers bind the EXACT SAME internal container port (8080) inside their own
    // isolated `--network bridge` (CLONE_NEWNET) namespaces at the same time!
    let mut ctr1 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--network",
            "bridge",
            "--image",
            "alpine:3.20",
            "--name",
            "bridge-web-1",
            "-p",
            &format!("127.0.0.1:{host_port_1}:8080"),
            "--",
            "/bin/sh",
            "-c",
            "echo BRIDGE_NODE_ONE | nc -l -p 8080 -w 3",
        ])
        .spawn()
        .unwrap();

    let mut ctr2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--network",
            "bridge",
            "--image",
            "alpine:3.20",
            "--name",
            "bridge-web-2",
            "-p",
            &format!("127.0.0.1:{host_port_2}:8080"),
            "--",
            "/bin/sh",
            "-c",
            "echo BRIDGE_NODE_TWO | nc -l -p 8080 -w 3",
        ])
        .spawn()
        .unwrap();

    let read_from_port = |port: u16, expected: &str| -> String {
        let mut buf = String::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
                use std::io::Read;
                let _ = s.set_read_timeout(Some(Duration::from_millis(500)));
                let _ = s.read_to_string(&mut buf);
                if buf.contains(expected) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        buf
    };

    let out1 = read_from_port(host_port_1, "BRIDGE_NODE_ONE");
    let out2 = read_from_port(host_port_2, "BRIDGE_NODE_TWO");

    let _ = ctr1.wait();
    let _ = ctr2.wait();

    assert!(out1.contains("BRIDGE_NODE_ONE"), "ctr1 output: {out1:?}");
    assert!(out2.contains("BRIDGE_NODE_TWO"), "ctr2 output: {out2:?}");
}

#[test]
fn dockerfile_build_creates_custom_image_with_from_env_copy_and_run() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");
    let ctx_dir = work.path().join("build-ctx");
    std::fs::create_dir_all(&ctx_dir).unwrap();

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    std::fs::write(ctx_dir.join("greeting.txt"), "from-build-context\n").unwrap();
    std::fs::write(
        ctx_dir.join("Dockerfile"),
        r#"
FROM alpine:3.20
ARG BUILD_TAG=v1.0
ENV SERVICE_VERSION=$BUILD_TAG
WORKDIR /opt/service
COPY greeting.txt /opt/service/greeting.txt
RUN echo "version=$SERVICE_VERSION msg=$(cat /opt/service/greeting.txt)" > /opt/service/manifest.txt
"#,
    )
    .unwrap();

    let build_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v1"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(
        build_out.status.success(),
        "coral build failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr)
    );

    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v1",
            "--name",
            "built-svc-ctr",
            "--",
            "/bin/cat",
            "/opt/service/manifest.txt",
        ])
        .output()
        .unwrap();
    assert!(run_out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "version=v1.0 msg=from-build-context"
    );

    // 2. Second build with identical context must hit step cache (`[CACHED]`).
    let build2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v1"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(build2.status.success());
    let build2_err = String::from_utf8_lossy(&build2.stderr);
    assert!(
        build2_err.contains("[CACHED]"),
        "expected second build to hit cache, stderr:\n{build2_err}"
    );

    // 3. Changing a copied context file invalidates COPY & RUN cache steps.
    std::fs::write(ctx_dir.join("greeting.txt"), "updated-context-v2\n").unwrap();
    let build3 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["build", "--local", "-t", "coral-service:v2"])
        .arg(&ctx_dir)
        .output()
        .unwrap();
    assert!(build3.status.success());
    let run_v2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "coral-service:v2",
            "--name",
            "built-svc-v2",
            "--",
            "/bin/cat",
            "/opt/service/manifest.txt",
        ])
        .output()
        .unwrap();
    assert!(run_v2.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run_v2.stdout).trim(),
        "version=v1.0 msg=updated-context-v2"
    );
}

#[test]
fn image_tag_save_zstd_and_load_roundtrip_cli() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let src_images = work.path().join("src-images");
    let dst_images = work.path().join("dst-images");
    let archive_path = work.path().join("exported-alpine.tar.zst");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Tag image
    let tag_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["tag", "--local", "alpine:3.20", "my-corp/alpine:prod"])
        .output()
        .unwrap();
    assert!(
        tag_out.status.success(),
        "coral tag failed: {}",
        String::from_utf8_lossy(&tag_out.stderr)
    );

    // 2. Save image to `.tar.zst`
    let save_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&src_images)
        .args(["save", "--local", "-o"])
        .arg(&archive_path)
        .arg("my-corp/alpine:prod")
        .output()
        .unwrap();
    assert!(
        save_out.status.success(),
        "coral save failed: {}",
        String::from_utf8_lossy(&save_out.stderr)
    );
    assert!(archive_path.is_file());

    // 3. Load archive into a completely empty `dst_images` store
    let load_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&dst_images)
        .args(["load", "--local", "-i"])
        .arg(&archive_path)
        .args(["-t", "offline-alpine:v1"])
        .output()
        .unwrap();
    assert!(
        load_out.status.success(),
        "coral load failed: {}",
        String::from_utf8_lossy(&load_out.stderr)
    );

    // 4. Run a container from the imported image
    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&dst_images)
        .args([
            "run",
            "--image",
            "offline-alpine:v1",
            "--name",
            "loaded-ctr",
            "--",
            "/bin/cat",
            "/etc/alpine-release",
        ])
        .output()
        .unwrap();
    assert!(run_out.status.success());
    assert!(
        String::from_utf8_lossy(&run_out.stdout)
            .trim()
            .starts_with("3.20")
    );
}
