//! `coral spec`, namespace/rootfs isolation and exit-code propagation.

use super::*;

#[test]
fn oci_spec_command_generates_valid_config() {
    let coral = ensure_coral_binary();
    let dir = tempdir().unwrap();
    let out_path = dir.path().join("config.json");

    let output = Command::new(coral)
        .args(["spec", "--rootless", "--output"])
        .arg(&out_path)
        .output()
        .expect("run coral spec");

    assert!(
        output.status.success(),
        "coral spec failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let spec = OciSpec::from_file(&out_path).expect("generated spec must be valid OCI v1.0.2");
    assert_eq!(spec.oci_version, "1.0.2");
}

#[test]
fn alpine_container_runs_with_pid_uts_mount_env_isolation() {
    let rootfs = alpine_rootfs();
    assert!(
        rootfs.join("bin/sh").exists(),
        "Alpine rootfs not found at {}; run scripts/setup-rootfs.sh",
        rootfs.display()
    );

    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let bundle = work.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();

    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
    spec.hostname = Some("coral-isolated-box".into());
    spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    let proc_cfg = spec.process.as_mut().unwrap();
    proc_cfg.env.push("CORAL_E2E_VAR=bluecoral-v1".into());
    proc_cfg.args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "echo PID=$$; echo HOST=$(hostname); echo ALPINE=$(cat /etc/alpine-release); echo ENV=$CORAL_E2E_VAR; echo null-ok > /dev/null; echo HELLO_EXISTS=$(test -f /hello.txt && echo yes || echo no)".into(),
    ];
    spec.to_file(bundle.join("config.json")).unwrap();

    let output = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("run")
        .arg("--bundle")
        .arg(&bundle)
        .arg("e2e-isolation-1")
        .output()
        .expect("execute coral run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "coral run failed (code={:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    assert!(stdout.contains("PID=1"), "expected PID=1, got:\n{stdout}");
    assert!(
        stdout.contains("HOST=coral-isolated-box"),
        "expected isolated hostname, got:\n{stdout}"
    );
    assert!(
        stdout.contains("ALPINE=3.20"),
        "expected Alpine 3.20 rootfs via pivot_root, got:\n{stdout}"
    );
    assert!(
        stdout.contains("ENV=bluecoral-v1"),
        "expected OCI env propagation, got:\n{stdout}"
    );
    assert!(
        stdout.contains("HELLO_EXISTS=yes"),
        "expected /hello.txt inside rootfs, got:\n{stdout}"
    );
}

#[test]
fn alpine_container_propagates_non_zero_exit_code() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let bundle = work.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();

    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
    spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    spec.process.as_mut().unwrap().args = vec!["/bin/sh".into(), "-c".into(), "exit 42".into()];
    spec.to_file(bundle.join("config.json")).unwrap();

    let status = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("run")
        .arg("--bundle")
        .arg(&bundle)
        .arg("e2e-exit-42")
        .status()
        .expect("execute coral run");

    assert_eq!(status.code(), Some(42));
}
