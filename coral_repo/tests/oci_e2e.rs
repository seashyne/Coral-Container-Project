//! Core OCI Runtime v1.0.2 E2E tests: `spec`, namespace/rootfs isolation, split `create`/`start`/`state`/`list`/`delete`,
//! Base Image Catalog + `pull --local` + `run --image`, OverlayFS Copy-on-Write isolation, and live `setns()` `exec`.

use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use coral_oci::{ContainerState, ContainerStatus, OciSpec};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

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

#[test]
fn oci_split_create_start_state_list_delete_lifecycle() {
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
    spec.process.as_mut().unwrap().args =
        vec!["/bin/sh".into(), "-c".into(), "sleep 0.1; exit 0".into()];
    spec.to_file(bundle.join("config.json")).unwrap();

    // 1. Create
    let create_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("create")
        .arg("--bundle")
        .arg(&bundle)
        .arg("e2e-lifecycle")
        .output()
        .expect("coral create");
    assert!(
        create_out.status.success(),
        "coral create failed: {}",
        String::from_utf8_lossy(&create_out.stderr)
    );
    let created: ContainerState = serde_json::from_slice(&create_out.stdout).unwrap();
    assert_eq!(created.status, ContainerStatus::Created);
    assert!(created.pid.is_some());

    // 2. State check
    let state_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["state", "e2e-lifecycle"])
        .output()
        .expect("coral state");
    assert!(state_out.status.success());
    let queried: ContainerState = serde_json::from_slice(&state_out.stdout).unwrap();
    assert_eq!(queried.status, ContainerStatus::Created);

    // 3. List
    let list_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("list")
        .output()
        .expect("coral list");
    assert!(list_out.status.success());
    assert!(String::from_utf8_lossy(&list_out.stdout).contains("e2e-lifecycle"));

    // 4. Start
    let start_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["start", "e2e-lifecycle"])
        .output()
        .expect("coral start");
    assert!(
        start_out.status.success(),
        "coral start failed: {}",
        String::from_utf8_lossy(&start_out.stderr)
    );

    // 5. Wait for container to transition to Stopped
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut final_status = ContainerStatus::Running;
    while Instant::now() < deadline {
        let out = Command::new(coral)
            .arg("--root")
            .arg(&state_root)
            .args(["state", "e2e-lifecycle"])
            .output()
            .unwrap();
        let st: ContainerState = serde_json::from_slice(&out.stdout).unwrap();
        final_status = st.status;
        if final_status == ContainerStatus::Stopped {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(final_status, ContainerStatus::Stopped);

    // 6. Delete
    let del_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .args(["delete", "e2e-lifecycle"])
        .output()
        .expect("coral delete");
    assert!(
        del_out.status.success(),
        "coral delete failed: {}",
        String::from_utf8_lossy(&del_out.stderr)
    );
}

#[test]
fn cli_catalog_pull_local_and_run_from_image() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    // 1. `coral catalog` shows all 7 Linux base images
    let cat_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["catalog", "--local"])
        .output()
        .expect("coral catalog");
    assert!(cat_out.status.success());
    let cat_str = String::from_utf8_lossy(&cat_out.stdout);
    for expected in [
        "alpine:3.20",
        "debian:bookworm-slim",
        "ubuntu:24.04",
        "busybox:musl",
        "rockylinux:9-minimal",
        "registry.access.redhat.com/ubi9/ubi-minimal:latest",
        "scratch",
    ] {
        assert!(
            cat_str.contains(expected),
            "catalog missing {expected}:\n{cat_str}"
        );
    }

    // 2. `coral pull scratch --local` + `coral images --local`
    let scratch_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "scratch", "--local"])
        .output()
        .expect("coral pull scratch");
    assert!(
        scratch_out.status.success(),
        "coral pull scratch failed: {}",
        String::from_utf8_lossy(&scratch_out.stderr)
    );

    // 3. Register local Alpine rootfs and run directly via `coral run --image`
    let reg_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "my-alpine:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .output()
        .expect("coral pull --rootfs");
    assert!(
        reg_out.status.success(),
        "coral pull --rootfs failed: {}",
        String::from_utf8_lossy(&reg_out.stderr)
    );

    let imgs_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["images", "--local"])
        .output()
        .expect("coral images");
    assert!(imgs_out.status.success());
    let imgs_str = String::from_utf8_lossy(&imgs_out.stdout);
    assert!(imgs_str.contains("scratch"));
    assert!(imgs_str.contains("my-alpine:3.20"));

    let run_out = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "my-alpine:3.20",
            "--name",
            "cli-img-demo",
            "-e",
            "GREET=coral-cli",
            "--",
            "/bin/sh",
            "-c",
            "echo RUN_OK:$GREET:$(hostname):$(cat /etc/alpine-release)",
        ])
        .output()
        .expect("coral run --image");
    let run_stdout = String::from_utf8_lossy(&run_out.stdout);
    let run_stderr = String::from_utf8_lossy(&run_out.stderr);
    assert!(
        run_out.status.success(),
        "coral run --image failed:\nstdout:\n{run_stdout}\nstderr:\n{run_stderr}"
    );
    assert!(
        run_stdout.contains("RUN_OK:coral-cli:cli-img-demo:3.20"),
        "unexpected output:\n{run_stdout}"
    );

    // 4. `coral rmi scratch --local`
    let rmi_out = Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["rmi", "scratch", "--local"])
        .output()
        .expect("coral rmi");
    assert!(rmi_out.status.success());
}

#[test]
fn overlayfs_cow_isolates_writes_from_shared_base_rootfs() {
    let rootfs = alpine_rootfs();
    let coral = ensure_coral_binary();
    let work = tempdir().unwrap();
    let state_root = work.path().join("state");
    let images_dir = work.path().join("images");

    Command::new(coral)
        .arg("--images-dir")
        .arg(&images_dir)
        .args(["pull", "alpine-cow:3.20", "--local", "--rootfs"])
        .arg(&rootfs)
        .status()
        .unwrap();

    // Container 1 creates /cow_only.txt inside its OverlayFS upperdir.
    let c1 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-cow:3.20",
            "--name",
            "cow-1",
            "--",
            "/bin/sh",
            "-c",
            "echo secret > /cow_only.txt && cat /cow_only.txt",
        ])
        .output()
        .unwrap();
    assert!(
        c1.status.success(),
        "cow-1 failed: {}",
        String::from_utf8_lossy(&c1.stderr)
    );
    assert!(String::from_utf8_lossy(&c1.stdout).contains("secret"));

    // Base rootfs must remain untouched!
    assert!(
        !rootfs.join("cow_only.txt").exists(),
        "OverlayFS CoW failed: base rootfs was mutated!"
    );

    // Container 2 sharing the same base image must NOT see /cow_only.txt!
    let c2 = Command::new(coral)
        .arg("--root")
        .arg(&state_root)
        .arg("--images-dir")
        .arg(&images_dir)
        .args([
            "run",
            "--image",
            "alpine-cow:3.20",
            "--name",
            "cow-2",
            "--",
            "/bin/sh",
            "-c",
            "test ! -e /cow_only.txt && echo CLEAN_BASE",
        ])
        .output()
        .unwrap();
    assert!(c2.status.success());
    assert!(String::from_utf8_lossy(&c2.stdout).contains("CLEAN_BASE"));
}

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
