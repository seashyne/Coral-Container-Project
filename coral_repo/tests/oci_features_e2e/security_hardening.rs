//! Live E2E security hardening tests (v2.9.0):
//! - Per-container `--cap-drop ALL` & `--cap-add NET_BIND_SERVICE`
//! - OCI `maskedPaths` & `readonlyPaths` kernel path defense
//! - Default-Deny Seccomp-BPF Allowlist (`Seccomp: 2`, `NoNewPrivs: 1`)
//! - Rootless Linux Landlock LSM (`--security-opt landlock=enforce`)
//! - `coral security audit --json` CIS posture output

use super::*;

#[test]
fn cap_drop_all_masked_paths_seccomp_allowlist_and_landlock_work_end_to_end() {
    let coral = ensure_coral_binary();
    let rootfs = alpine_rootfs();
    let tmp = tempdir().unwrap();
    let state_root = tmp.path().join("state");
    let images_dir = tmp.path().join("images");

    // 1. `--cap-drop ALL` + `--security-opt landlock=enforce` + default-deny Seccomp + maskedPaths
    let out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("run")
        .arg("--image")
        .arg(&rootfs)
        .args([
            "--name",
            "sec-hardened",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "landlock=enforce",
            "--",
            "/bin/sh",
            "-c",
            "grep -E '^(NoNewPrivs|Seccomp|CapEff):' /proc/self/status; \
             if [ -e /proc/kcore ]; then echo KCORE_BYTES=$(wc -c < /proc/kcore); else echo KCORE_BYTES=0; fi",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "hardened container failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("NoNewPrivs:\t1"), "out:\n{stdout}");
    assert!(stdout.contains("Seccomp:\t2"), "out:\n{stdout}");
    assert!(
        stdout.contains("CapEff:\t0000000000000000"),
        "expected zero effective caps with --cap-drop ALL, got:\n{stdout}"
    );
    assert!(
        stdout.contains("KCORE_BYTES=0"),
        "expected /proc/kcore to be masked to /dev/null (0 bytes), got:\n{stdout}"
    );

    // 2. `--cap-drop ALL --cap-add NET_BIND_SERVICE` grants only bit 10 (0x400)
    let out_cap = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("run")
        .arg("--image")
        .arg(&rootfs)
        .args([
            "--name",
            "sec-cap-netbind",
            "--cap-drop",
            "ALL",
            "--cap-add",
            "NET_BIND_SERVICE",
            "--",
            "/bin/sh",
            "-c",
            "grep '^CapEff:' /proc/self/status",
        ])
        .output()
        .unwrap();
    assert!(
        out_cap.status.success(),
        "cap-add container failed: {}",
        String::from_utf8_lossy(&out_cap.stderr)
    );
    let cap_stdout = String::from_utf8_lossy(&out_cap.stdout);
    assert!(
        cap_stdout.contains("CapEff:\t0000000000000400"),
        "expected only CAP_NET_BIND_SERVICE (0x400), got:\n{cap_stdout}"
    );

    // 3. `coral security audit --json` produces structured CIS report
    let audit = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--addr")
        .arg("http://127.0.0.1:1")
        .args(["security", "audit", "--json"])
        .output()
        .unwrap();
    assert!(
        audit.status.success(),
        "coral security audit failed: {}",
        String::from_utf8_lossy(&audit.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&audit.stdout)
        .expect("coral security audit --json should emit valid JSON");
    assert!(report["score"].as_u64().unwrap_or(0) >= 80, "report: {report}");
    let checks = report["checks"].as_array().unwrap();
    assert!(checks.iter().any(|c| c["id"] == "CIS-1.1"));
    assert!(checks.iter().any(|c| c["id"] == "CIS-2.1"));
    assert!(checks.iter().any(|c| c["id"] == "CIS-2.2"));

    // 4. `--read-only` locks container rootfs against writes
    let ro_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("run")
        .arg("--image")
        .arg(&rootfs)
        .args([
            "--name",
            "sec-readonly",
            "--read-only",
            "--",
            "/bin/sh",
            "-c",
            "touch /ro-probe 2>&1; echo EXIT=$?",
        ])
        .output()
        .unwrap();
    assert!(
        ro_out.status.success(),
        "readonly container failed to run: {}",
        String::from_utf8_lossy(&ro_out.stderr)
    );
    let ro_stdout = String::from_utf8_lossy(&ro_out.stdout);
    assert!(
        ro_stdout.contains("Read-only file system") && !ro_stdout.contains("EXIT=0"),
        "expected write to / to fail on --read-only rootfs, got:\n{ro_stdout}"
    );
}

#[test]
fn daemon_bearer_token_rejects_unauthenticated_cli_and_accepts_coral_auth_token() {
    let coral = ensure_coral_binary();
    let daemon = coral.with_file_name("coral-daemon");
    if !daemon.is_file() {
        return;
    }
    let tmp = tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let listen = format!("127.0.0.1:{port}");
    let addr = format!("http://{listen}");

    let mut child = Command::new(&daemon)
        .args([
            "--listen",
            &listen,
            "--no-docker-socket",
            "--no-unix-socket",
            "--auth-token",
            "coral-e2e-secret-291",
            "--state-dir",
        ])
        .arg(tmp.path().join("daemon"))
        .spawn()
        .expect("spawn coral-daemon with --auth-token");

    // Wait for TCP port to accept connections (up to 5s).
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(&listen).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Without token: `coral ps` MUST fail with unauthenticated error.
    let unauth = Command::new(coral)
        .env_remove("CORAL_AUTH_TOKEN")
        .args(["--addr", &addr, "ps"])
        .output()
        .unwrap();
    assert!(
        !unauth.status.success(),
        "expected unauthenticated `coral ps` to be rejected"
    );
    let unauth_err = String::from_utf8_lossy(&unauth.stderr);
    assert!(
        unauth_err.to_ascii_lowercase().contains("unauthenticated")
            || unauth_err.contains("Bearer"),
        "expected unauthenticated error, got:\n{unauth_err}"
    );

    // With matching CORAL_AUTH_TOKEN: `coral ps` MUST succeed.
    let authed = Command::new(coral)
        .env("CORAL_AUTH_TOKEN", "coral-e2e-secret-291")
        .args(["--addr", &addr, "ps"])
        .output()
        .unwrap();

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        authed.status.success(),
        "expected authenticated `coral ps` to succeed, stderr:\n{}",
        String::from_utf8_lossy(&authed.stderr)
    );
}
