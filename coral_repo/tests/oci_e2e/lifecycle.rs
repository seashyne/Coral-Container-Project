//! Split `create`/`start`/`state`/`list`/`delete` container lifecycle.

use super::*;

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
