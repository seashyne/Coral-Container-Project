//! Regression test for the atomic `state.json` write.
//!
//! The daemon records the spawn result of a container while the forked
//! container supervisor records its exit status. Both wrote through the same
//! `state.json.tmp`, so whichever renamed first left the other with
//! `rename state.json: No such file or directory` — a failure that surfaced
//! as an intermittent `coral run --daemon` error.
//!
//! The window is narrow, so this test drives many short-lived containers
//! through the real daemon path and asserts that every one of them succeeds
//! and leaves a readable state file behind.

use super::*;
use coral_runtime::store::StateStore;

const ITERATIONS: usize = 40;

/// Starts a daemon service on an ephemeral port and returns its client.
async fn start_daemon(rootfs: std::path::PathBuf) -> RuntimeServiceClient<tonic::transport::Channel> {
    let coral_bin = ensure_coral_binary().clone();
    let daemon_dir = tempdir().unwrap();
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = TcpListenerStream::new(listener);

    tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });

    RuntimeServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap()
}

#[tokio::test]
async fn cri_short_lived_containers_never_race_on_the_state_file() {
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists(), "Alpine rootfs must exist");

    let mut rt_client = start_daemon(rootfs.clone()).await;

    // A sandbox is required before a container can be created.
    let pod_id = rt_client
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "coral-state-race".into(),
                    uid: "uid-001".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "coral-state-race".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::new(),
            }),
            runtime_handler: String::new(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    for i in 0..ITERATIONS {
        let ctr_id = rt_client
            .create_container(CreateContainerRequest {
                pod_sandbox_id: pod_id.clone(),
                config: Some(ContainerConfig {
                    metadata: Some(ContainerMetadata {
                        name: format!("race-{i}"),
                        attempt: 0,
                    }),
                    image: Some(ImageSpec {
                        image: "alpine:latest".into(),
                        annotations: HashMap::new(),
                    }),
                    // Exits immediately: this is the case where the supervisor
                    // records the exit status while the daemon is still
                    // recording the spawn result.
                    command: vec!["/bin/sh".into(), "-c".into()],
                    args: vec!["echo STATE_RACE_OK".into()],
                    working_dir: "/".into(),
                    envs: Vec::new(),
                    labels: HashMap::new(),
                    annotations: HashMap::new(),
                    log_path: String::new(),
                    linux: None,
                }),
                sandbox_config: None,
            })
            .await
            .unwrap_or_else(|e| panic!("create_container {i} failed: {e:?}"))
            .into_inner()
            .container_id;

        rt_client
            .start_container(StartContainerRequest {
                container_id: ctr_id.clone(),
            })
            .await
            .unwrap_or_else(|e| panic!("start_container {i} failed: {e:?}"));

        // Wait for the container to exit and check that the recorded state
        // stays readable.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut exited = false;
        while Instant::now() < deadline {
            let resp = rt_client
                .container_status(ContainerStatusRequest {
                    container_id: ctr_id.clone(),
                    verbose: true,
                })
                .await
                .unwrap()
                .into_inner();
            let st = resp.status.as_ref().unwrap();
            if st.state == ContainerState::ContainerExited as i32 {
                assert_eq!(st.exit_code, 0, "container {i} failed: {}", st.message);
                exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(exited, "container {i} did not finish in time");
    }
}

/// The state file a container leaves behind must always be complete.
#[test]
fn runtime_state_files_stay_parseable_after_many_writes() {
    let dir = tempdir().unwrap();
    let store = StateStore::new(dir.path());

    let state = coral_oci::ContainerState::new(
        "c1",
        std::path::PathBuf::from("/tmp/bundle"),
        std::collections::BTreeMap::new(),
    );
    store.init_container(&state).unwrap();

    for _ in 0..50 {
        store.save(&state).unwrap();
        let loaded = store.load("c1").unwrap();
        assert_eq!(loaded.id, "c1");
    }
}
