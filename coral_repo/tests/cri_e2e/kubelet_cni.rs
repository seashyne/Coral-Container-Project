//! Kubelet CNI plugin chain, streaming, attach and port forward.

use super::*;

#[tokio::test]
async fn cri_v22_kubelet_cni_chain_streaming_attach_portforward_critest_e2e() {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    let dir = tempdir().unwrap();

    // 1. Set up a real multi-plugin CNI v1.0.0 `.conflist` chain (mock-calico -> mock-portmap)
    let cni_conf_dir = dir.path().join("cni-net.d");
    let cni_bin_dir = dir.path().join("cni-bin");
    let cni_trace = dir.path().join("cni.trace");
    fs::create_dir_all(&cni_conf_dir).unwrap();
    fs::create_dir_all(&cni_bin_dir).unwrap();

    let p1 = cni_bin_dir.join("mock-calico");
    fs::write(
        &p1,
        format!(
            "#!/bin/sh\n\
             cat >/dev/null\n\
             echo \"$CNI_COMMAND:calico:$CNI_CONTAINERID\" >> \"{}\"\n\
             if [ \"$CNI_COMMAND\" = \"ADD\" ]; then\n\
               echo '{{\"cniVersion\":\"1.0.0\",\"interfaces\":[{{\"name\":\"eth0\",\"mac\":\"02:42:0a:f4:01:2a\"}}],\"ips\":[{{\"address\":\"10.244.1.42/24\",\"gateway\":\"10.244.1.1\",\"interface\":0}}]}}'\n\
             fi\n",
            cni_trace.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&p1, fs::Permissions::from_mode(0o755)).unwrap();

    let p2 = cni_bin_dir.join("mock-portmap");
    fs::write(
        &p2,
        format!(
            "#!/bin/sh\n\
             IN=$(cat)\n\
             echo \"$CNI_COMMAND:portmap:$CNI_CONTAINERID\" >> \"{}\"\n\
             if [ \"$CNI_COMMAND\" = \"ADD\" ]; then\n\
               echo \"$IN\" | grep -q '10.244.1.42/24' || exit 33\n\
               echo '{{\"cniVersion\":\"1.0.0\",\"interfaces\":[{{\"name\":\"eth0\",\"mac\":\"02:42:0a:f4:01:2a\"}}],\"ips\":[{{\"address\":\"10.244.1.42/24\",\"gateway\":\"10.244.1.1\",\"interface\":0}}]}}'\n\
             fi\n",
            cni_trace.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&p2, fs::Permissions::from_mode(0o755)).unwrap();

    fs::write(
        cni_conf_dir.join("10-coral-calico.conflist"),
        r#"{
          "cniVersion": "1.0.0",
          "name": "coral-k8s-cni",
          "plugins": [
            { "type": "mock-calico" },
            { "type": "mock-portmap" }
          ]
        }"#,
    )
    .unwrap();

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("daemon"),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    })
    .with_cni(&cni_conf_dir, &cni_bin_dir);
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

    let ep = format!("http://{addr}");
    let mut rt = RuntimeServiceClient::connect(ep.clone()).await.unwrap();
    let mut img_cli = ImageServiceClient::connect(ep).await.unwrap();

    // 2. UpdateRuntimeConfig + ImageFsInfo (`critest` compliance)
    rt.update_runtime_config(coral_api::UpdateRuntimeConfigRequest {
        runtime_config: Some(coral_api::RuntimeConfig {
            network_config: Some(coral_api::NetworkConfig {
                pod_cidr: "10.244.1.0/24".into(),
            }),
        }),
    })
    .await
    .unwrap();

    let fs_info = img_cli
        .image_fs_info(coral_api::ImageFsInfoRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(!fs_info.image_filesystems.is_empty());

    // 3. RunPodSandbox triggers CNI plugin chain -> assigns 10.244.1.42
    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "k8s-cni-pod".into(),
                    uid: "k8s-cni-uid".into(),
                    namespace: "default".into(),
                    attempt: 0,
                }),
                hostname: "k8s-cni".into(),
                log_directory: String::new(),
                labels: HashMap::from([("app".into(), "demo".into())]),
                annotations: HashMap::new(),
            }),
            runtime_handler: "coral".into(),
        })
        .await
        .unwrap()
        .into_inner()
        .pod_sandbox_id;

    let pod_st = rt
        .pod_sandbox_status(coral_api::PodSandboxStatusRequest {
            pod_sandbox_id: pod_id.clone(),
            verbose: true,
        })
        .await
        .unwrap()
        .into_inner()
        .status
        .unwrap();
    assert_eq!(pod_st.network.unwrap().ip, "10.244.1.42");

    // 4. Create & Start a container, verify ContainerStats, ListContainerStats, ReopenContainerLog
    let cid = rt
        .create_container(CreateContainerRequest {
            pod_sandbox_id: pod_id.clone(),
            config: Some(ContainerConfig {
                metadata: Some(ContainerMetadata {
                    name: "app".into(),
                    attempt: 0,
                }),
                image: Some(ImageSpec {
                    image: "alpine:latest".into(),
                    annotations: HashMap::new(),
                }),
                command: vec!["/bin/sh".into()],
                args: vec!["-c".into(), "echo ATTACH_LINE_OK; sleep 10".into()],
                working_dir: "/".into(),
                envs: Vec::new(),
                labels: HashMap::from([("app".into(), "demo".into())]),
                annotations: HashMap::new(),
                log_path: String::new(),
                linux: None,
            }),
            sandbox_config: None,
        })
        .await
        .unwrap()
        .into_inner()
        .container_id;

    rt.start_container(StartContainerRequest {
        container_id: cid.clone(),
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let stats = rt
        .container_stats(coral_api::ContainerStatsRequest {
            container_id: cid.clone(),
        })
        .await
        .unwrap()
        .into_inner()
        .stats
        .unwrap();
    assert_eq!(stats.attributes.unwrap().id, cid);

    let list_stats = rt
        .list_container_stats(coral_api::ListContainerStatsRequest { filter: None })
        .await
        .unwrap()
        .into_inner();
    assert!(list_stats.stats.iter().any(|s| s.attributes.as_ref().unwrap().id == cid));

    rt.reopen_container_log(coral_api::ReopenContainerLogRequest {
        container_id: cid.clone(),
    })
    .await
    .unwrap();

    // 5. Streaming Exec RPC -> HTTP GET to returned streaming URL
    let exec_url = rt
        .exec(coral_api::ExecRequest {
            container_id: cid.clone(),
            cmd: vec!["/bin/sh".into(), "-c".into(), "echo CRI_STREAM_EXEC_OK".into()],
            tty: false,
            stdin: false,
            stdout: true,
            stderr: true,
        })
        .await
        .unwrap()
        .into_inner()
        .url;
    let exec_resp = http_get_raw(&exec_url).await;
    assert!(exec_resp.contains("CRI_STREAM_EXEC_OK"), "exec_resp={exec_resp}");

    // 6. Streaming Attach RPC -> HTTP GET to returned streaming URL reads live container output
    let attach_url = rt
        .attach(coral_api::AttachRequest {
            container_id: cid.clone(),
            stdin: false,
            tty: false,
            stdout: true,
            stderr: true,
        })
        .await
        .unwrap()
        .into_inner()
        .url;
    let attach_resp = http_get_first_chunk(&attach_url).await;
    assert!(
        attach_resp.contains("ATTACH_LINE_OK"),
        "attach_resp={attach_resp}"
    );

    // 7. Streaming PortForward RPC -> full-duplex TCP tunnel to a live echo backend
    let echo_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let echo_port = echo_listener.local_addr().unwrap().port();
    let echo_task = tokio::spawn(async move {
        if let Ok((mut s, _)) = echo_listener.accept().await {
            let mut b = [0u8; 64];
            if let Ok(n) = s.read(&mut b).await {
                let _ = s.write_all(&b[..n]).await;
            }
        }
    });

    let pf_url = rt
        .port_forward(coral_api::PortForwardRequest {
            pod_sandbox_id: pod_id.clone(),
            port: vec![i32::from(echo_port)],
        })
        .await
        .unwrap()
        .into_inner()
        .url;

    let (pf_host, pf_path) = parse_http_url(&pf_url);
    let mut pf_conn = tokio::net::TcpStream::connect(&pf_host).await.unwrap();
    let req_bytes = format!(
        "POST {pf_path}?port={echo_port} HTTP/1.1\r\nHost: {pf_host}\r\nConnection: Upgrade\r\nUpgrade: SPDY/3.1\r\n\r\nPING_PORTFORWARD_OK"
    );
    pf_conn.write_all(req_bytes.as_bytes()).await.unwrap();
    let mut pf_buf = vec![0u8; 512];
    let mut pf_out = String::new();
    let pf_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < pf_deadline && !pf_out.contains("PING_PORTFORWARD_OK") {
        let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(500), pf_conn.read(&mut pf_buf)).await else {
            break;
        };
        if n == 0 {
            break;
        }
        pf_out.push_str(&String::from_utf8_lossy(&pf_buf[..n]));
    }
    assert!(pf_out.contains("101 Switching Protocols"), "pf_out={pf_out}");
    assert!(pf_out.contains("PING_PORTFORWARD_OK"), "pf_out={pf_out}");
    let _ = echo_task.await;

    // 8. Stop/Remove Container & PodSandbox -> verifies reverse-order CNI DEL execution
    rt.stop_container(StopContainerRequest {
        container_id: cid.clone(),
        timeout: 2,
    })
    .await
    .unwrap();
    rt.remove_container(RemoveContainerRequest { container_id: cid })
        .await
        .unwrap();
    rt.stop_pod_sandbox(StopPodSandboxRequest {
        pod_sandbox_id: pod_id.clone(),
    })
    .await
    .unwrap();
    rt.remove_pod_sandbox(RemovePodSandboxRequest {
        pod_sandbox_id: pod_id.clone(),
    })
    .await
    .unwrap();

    let trace = fs::read_to_string(&cni_trace).unwrap();
    let lines: Vec<&str> = trace.lines().collect();
    assert_eq!(
        lines,
        vec![
            format!("ADD:calico:{pod_id}"),
            format!("ADD:portmap:{pod_id}"),
            format!("DEL:portmap:{pod_id}"),
            format!("DEL:calico:{pod_id}"),
        ]
    );

    svc_ctrl.shutdown().await;
    handle.abort();
}
