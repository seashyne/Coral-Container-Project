//! Bind/named volumes, custom DNS, interactive PTY and security hardening.

use super::*;

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
