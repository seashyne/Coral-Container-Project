//! Rootless multi-UID chown, su, subuid cleanup and service users.

use super::*;

#[test]
fn multi_uid_rootless_chown_su_and_subuid_cleanup() {
    let support = coral_runtime::MultiUidSupport::detect();
    if !support.is_available() {
        eprintln!(
            "skipping multi_uid_rootless test: {}",
            support.unavailable_reason()
        );
        return;
    }

    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");
    let bundle = work.path().join("bundle-multi");
    std::fs::create_dir_all(&bundle).unwrap();

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // 1. Verify default `coral run` activates Multi-UID ranges, allows `setgroups`,
    // supports `chown` across arbitrary container UIDs/GIDs, and allows `su nobody`.
    let out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--pull",
            "never",
            "--name",
            "multi-uid-live",
            "alpine:3.20",
            "/bin/sh",
            "-c",
            concat!(
                "set -eu; ",
                "echo UID_LINES=$(wc -l < /proc/self/uid_map); ",
                "echo GID_LINES=$(wc -l < /proc/self/gid_map); ",
                "echo SETGROUPS=$(cat /proc/self/setgroups); ",
                "mkdir -p /var/tmp/subtest && echo ok > /var/tmp/subtest/file; ",
                "chown -R 1000:1000 /var/tmp/subtest && chmod 0700 /var/tmp/subtest; ",
                "echo OWNER=$(stat -c '%u:%g:%a' /var/tmp/subtest); ",
                "echo SUBUSER=$(su -s /bin/sh nobody -c 'id -u'); ",
                "echo MULTI_UID_OK"
            ),
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "multi-UID container failed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("MULTI_UID_OK"),
        "missing MULTI_UID_OK:\n{stdout}"
    );
    assert!(
        stdout.contains("SETGROUPS=allow"),
        "expected SETGROUPS=allow:\n{stdout}"
    );
    assert!(
        stdout.contains("OWNER=1000:1000:700"),
        "expected chowned dir metadata:\n{stdout}"
    );
    assert!(
        stdout.contains("SUBUSER=65534"),
        "expected `su nobody` -> uid 65534:\n{stdout}"
    );
    let uid_lines: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("UID_LINES="))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    assert!(
        uid_lines >= 2,
        "expected >= 2 uid_map lines, got:\n{stdout}"
    );

    // 2. Verify OverlayFS upperdir files created under `chown 1000:1000 + chmod 0700`
    // produce real host subuid-owned directories that standard `fs::remove_dir_all`
    // cannot remove directly (`PermissionDenied`), and `remove_dir_all_rootless_safe`
    // cleans up completely via an unshared Multi-UID helper UserNS.
    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = coral_oci::OciSpec::default_spec(true, host_uid, host_gid);
    spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    spec.annotations.insert("coral.overlay".into(), "true".into());
    spec.process.as_mut().unwrap().args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "mkdir -p /var/tmp/sub-persist && echo x > /var/tmp/sub-persist/f && chown -R 1000:1000 /var/tmp/sub-persist && chmod 0700 /var/tmp/sub-persist".into(),
    ];
    spec.to_file(bundle.join("config.json")).unwrap();

    let mgr = coral_runtime::ContainerManager::new(
        state_root.join("rt"),
        coral_cgroups::CgroupManager::detect(),
    );
    let exit = mgr
        .run(
            "multi-uid-upper",
            &bundle,
            &coral_runtime::RunOptions::default(),
        )
        .unwrap();
    assert_eq!(exit.exit_code, 0);

    assert!(
        has_named_dir(&bundle, "sub-persist"),
        "expected chowned sub-persist dir inside overlay bundle"
    );
    let direct_rm = std::fs::remove_dir_all(&bundle);
    assert!(
        direct_rm.is_err(),
        "expected unprivileged host fs::remove_dir_all on subuid-owned 0700 dir to fail"
    );
    coral_runtime::remove_dir_all_rootless_safe(&bundle)
        .expect("subuid-safe recursive remove should succeed");
    assert!(!bundle.exists(), "bundle dir must be completely removed");
}

#[test]
fn multi_uid_rootless_service_user_exec_kill_and_single_uid_opt_out() {
    let support = coral_runtime::MultiUidSupport::detect();
    if !support.is_available() {
        return;
    }

    let rootfs = alpine_rootfs();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let mgr = coral_runtime::ContainerManager::new(
        &state_root,
        coral_cgroups::CgroupManager::detect(),
    );

    // 1. Launch detached container with `coral.user = "nobody"` (65534 -> host subuid),
    // verify `exec` into it works and `kill(SIGTERM)` + `delete` clean up properly.
    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let bundle_nobody = work.path().join("bundle-nobody");
    std::fs::create_dir_all(&bundle_nobody).unwrap();
    let mut spec = coral_oci::OciSpec::default_spec(true, host_uid, host_gid);
    spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    spec.annotations.insert("coral.user".into(), "nobody".into());
    spec.process.as_mut().unwrap().args = vec!["/bin/sleep".into(), "30".into()];
    spec.to_file(bundle_nobody.join("config.json")).unwrap();

    mgr.create("multi-nobody", &bundle_nobody).unwrap();
    mgr.start("multi-nobody").unwrap();

    // `start()` unblocks exec.fifo asynchronously; wait briefly for PID 1 to finish
    // mounting `/proc` and dropping to `nobody` (65534).
    let start_deadline = Instant::now() + Duration::from_secs(3);
    let mut exec_code = -1;
    while Instant::now() < start_deadline {
        if let Ok(code) = mgr.exec(
            "multi-nobody",
            &coral_runtime::ExecOptions {
                cmd: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "grep -qE '^Uid:[[:space:]]+65534[[:space:]]+65534' /proc/1/status".into(),
                ],
                env: vec![],
                cwd: None,
                tty: false,
                interactive: false,
            },
        ) {
            exec_code = code;
            if exec_code == 0 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(exec_code, 0, "exec check on PID 1 running as nobody failed");

    let sigterm = coral_runtime::container::parse_signal("SIGTERM").unwrap();
    mgr.kill("multi-nobody", sigterm)
        .expect("kill against container PID running under host subuid must succeed");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if mgr.state("multi-nobody").unwrap().status == coral_oci::ContainerStatus::Stopped {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    mgr.delete("multi-nobody", true).unwrap();

    // 2. Verify explicit `coral.userns.single_uid = "true"` opts back into 1-line single-UID map.
    let bundle_single = work.path().join("bundle-single");
    std::fs::create_dir_all(&bundle_single).unwrap();
    let mut single_spec = coral_oci::OciSpec::default_spec(true, host_uid, host_gid);
    single_spec.root.as_mut().unwrap().path = rootfs.to_string_lossy().into_owned();
    single_spec
        .annotations
        .insert("coral.userns.single_uid".into(), "true".into());
    single_spec.process.as_mut().unwrap().args = vec![
        "/bin/sh".into(),
        "-c".into(),
        "test \"$(wc -l < /proc/self/uid_map)\" -eq 1 && grep -q deny /proc/self/setgroups".into(),
    ];
    single_spec.to_file(bundle_single.join("config.json")).unwrap();

    let exit = mgr
        .run(
            "single-opt-out",
            &bundle_single,
            &coral_runtime::RunOptions::default(),
        )
        .unwrap();
    assert_eq!(exit.exit_code, 0);
}

fn has_named_dir(root: &std::path::Path, target: &str) -> bool {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if p.file_name().and_then(|n| n.to_str()) == Some(target) {
                    return true;
                }
                stack.push(p);
            }
        }
    }
    false
}
