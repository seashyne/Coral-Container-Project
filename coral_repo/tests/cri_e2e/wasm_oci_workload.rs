//! WebAssembly / OCI workloads, KVM probe and AI sandbox.

use super::*;

#[tokio::test]
async fn cri_v2_wasm_oci_workload_kvm_probe_and_ai_sandbox_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    assert!(rootfs.join("bin/sh").exists());

    let daemon_dir = tempdir().unwrap();
    let images_dir = daemon_dir.path().join("images");
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: Some(images_dir.clone()),
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });
    let svc_ctrl = svc.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc.clone()))
            .add_service(ImageServiceServer::new(svc))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut rt = RuntimeServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    // 1. Probe host /dev/kvm via CRI (`coral.system.action = "vmm_probe"`).
    let probe_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([("coral.system.action".into(), "vmm_probe".into())]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let probe_val: serde_json::Value = serde_json::from_str(&probe_json).unwrap();
    assert!(probe_val["capabilities"]["arch"].is_string());
    if probe_val["capabilities"]["available"].as_bool() == Some(true) {
        assert_eq!(
            probe_val["selftest"]["serialOutput"].as_str(),
            Some("CORAL_KVM_VMM_64BIT_OK\n")
        );
        assert_eq!(probe_val["selftest"]["exitCode"].as_i64(), Some(0));
    }

    // 2. Run inline WASI Preview 1 WebAssembly workload via CRI (`coral.system.action = "wasm_run"`).
    let wat_src = r#"
    (module
      (import "wasi_snapshot_preview1" "fd_write"
        (func $fd_write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1)
      (data (i32.const 0) "\10\00\00\00\13\00\00\00")
      (data (i32.const 16) "CORAL_WASM_CRI_OK!\n")
      (func (export "_start")
        (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 64)))))
    "#;
    let wasm_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "wasm_run".into()),
                    ("coral.wasm.source".into(), wat_src.into()),
                    ("coral.wasm.fuel".into(), "1000000".into()),
                    ("coral.wasm.memory_mb".into(), "8".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let wasm_val: serde_json::Value = serde_json::from_str(&wasm_json).unwrap();
    assert_eq!(wasm_val["exitCode"].as_i64(), Some(0));
    assert_eq!(wasm_val["stdout"].as_str(), Some("CORAL_WASM_CRI_OK!\n"));
    assert!(wasm_val["fuelConsumed"].as_u64().unwrap_or(0) > 0);

    // 3. Pack a `.wat` file as a Coral OCI Wasm image (`coral wasm pack`) and run it as a CRI container!
    let wat_file = daemon_dir.path().join("hello.wat");
    std::fs::write(&wat_file, wat_src).unwrap();
    let pack_out = tokio::process::Command::new(&coral_bin)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("wasm")
        .arg("pack")
        .arg(&wat_file)
        .arg("-t")
        .arg("hello-wasm:v1")
        .output()
        .await
        .unwrap();
    assert!(
        pack_out.status.success(),
        "coral wasm pack failed: {}",
        String::from_utf8_lossy(&pack_out.stderr)
    );
    let wasm_run_cli = tokio::process::Command::new(&coral_bin)
        .arg("--images-dir")
        .arg(&images_dir)
        .arg("run")
        .arg("--runtime")
        .arg("wasm")
        .arg("--image")
        .arg("hello-wasm:v1")
        .output()
        .await
        .unwrap();
    assert!(wasm_run_cli.status.success());
    assert_eq!(
        String::from_utf8_lossy(&wasm_run_cli.stdout),
        "CORAL_WASM_CRI_OK!\n"
    );

    // 4. Test AI Agent Untrusted Code Sandbox (`coral.system.action = "sandbox_run"`):
    //    4a. Normal command runs air-gapped and returns structured JSON.
    let sbx_ok_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "sandbox_run".into()),
                    ("coral.sandbox.image".into(), "alpine:latest".into()),
                    ("coral.sandbox.cmd".into(), "echo AGENT_SANDBOX_SAFE".into()),
                    ("coral.sandbox.timeout".into(), "5".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let sbx_ok: serde_json::Value = serde_json::from_str(&sbx_ok_json).unwrap();
    assert_eq!(sbx_ok["exitCode"].as_i64(), Some(0));
    assert_eq!(sbx_ok["timedOut"].as_bool(), Some(false));
    assert!(
        sbx_ok["stdout"]
            .as_str()
            .unwrap_or("")
            .contains("AGENT_SANDBOX_SAFE")
    );

    //    4b. Runaway command (`sleep 30`) is deterministically killed by the 1s sandbox timeout!
    let sbx_to_json = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: None,
                hostname: String::new(),
                log_directory: String::new(),
                labels: HashMap::new(),
                annotations: HashMap::from([
                    ("coral.system.action".into(), "sandbox_run".into()),
                    ("coral.sandbox.image".into(), "alpine:latest".into()),
                    ("coral.sandbox.cmd".into(), "sleep 30".into()),
                    ("coral.sandbox.timeout".into(), "1".into()),
                ]),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;
    let sbx_to: serde_json::Value = serde_json::from_str(&sbx_to_json).unwrap();
    assert_eq!(sbx_to["timedOut"].as_bool(), Some(true));
    assert_eq!(sbx_to["exitCode"].as_i64(), Some(124));

    svc_ctrl.shutdown().await;
    handle.abort();
}
