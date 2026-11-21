//! Live `setns()` exec into a running container namespace.

use super::*;

#[test]
fn real_setns_exec_enters_running_container_namespaces() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let bundle = work.path().join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();

    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
    spec.hostname = Some("live-exec-host".into());
    spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    spec.annotations.insert("coral.overlay".into(), "true".into());
    spec.process.as_mut().unwrap().args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "echo LIVE_FILE > /tmp/live_marker && echo OVERLAY_MARK > /live_ovl && sleep 15".into(),
    ];
    spec.to_file(bundle.join("config.json")).unwrap();

    let create = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["create", "--bundle"])
        .arg(&bundle)
        .arg("live-ctr")
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let start = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["start", "live-ctr"])
        .output()
        .unwrap();
    assert!(start.status.success());

    std::thread::sleep(Duration::from_millis(150));

    let exec_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args([
            "exec",
            "--local",
            "live-ctr",
            "--",
            "/bin/sh",
            "-c",
            "echo HOST=$(hostname) TMP=$(cat /tmp/live_marker) OVL=$(cat /live_ovl)",
        ])
        .output()
        .unwrap();

    let _ = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["delete", "--force", "live-ctr"])
        .output();

    let stdout = String::from_utf8_lossy(&exec_out.stdout);
    let stderr = String::from_utf8_lossy(&exec_out.stderr);
    assert!(
        exec_out.status.success(),
        "coral exec --local failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("HOST=live-exec-host TMP=LIVE_FILE OVL=OVERLAY_MARK"),
        "setns exec did not enter container namespaces:\n{stdout}"
    );
}
