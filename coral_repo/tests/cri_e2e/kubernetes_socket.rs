//! Kubernetes unix-socket transport and status reporting.

use super::*;

#[tokio::test]
async fn cri_v21_kubernetes_unix_socket_and_status_e2e() {
    let coral_bin = ensure_coral_binary().clone();
    let rootfs = alpine_rootfs();
    let dir = tempdir().unwrap();
    let sock_path = dir.path().join("coral.sock");

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("daemon"),
        images_dir: None,
        coral_bin,
        default_rootfs: Some(rootfs),
    });
    let svc_ctrl = svc.clone();

    let uds = tokio::net::UnixListener::bind(&sock_path).unwrap();
    let uds_stream = tokio_stream::wrappers::UnixListenerStream::new(uds);
    let handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(svc.clone()))
            .add_service(ImageServiceServer::new(svc))
            .serve_with_incoming(uds_stream)
            .await
            .unwrap();
    });

    // Connect over native Kubernetes CRI v1 Unix Domain Socket (`unix://`)
    let connect_path = sock_path.clone();
    let ch = tonic::transport::Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
            let p = connect_path.clone();
            async move {
                let s = tokio::net::UnixStream::connect(p).await?;
                Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
            }
        }))
        .await
        .unwrap();

    let mut rt = RuntimeServiceClient::new(ch);

    // 1. CRI Version over Unix socket
    let ver = rt
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(ver.runtime_name, "coral");

    // 2. CRI Status(verbose=true) — kubelet / crictl readiness probe
    let st = rt
        .status(coral_api::StatusRequest { verbose: true })
        .await
        .unwrap()
        .into_inner();
    let conds = st.status.unwrap().conditions;
    assert!(conds.iter().any(|c| c.r#type == "RuntimeReady" && c.status));
    assert!(conds.iter().any(|c| c.r#type == "NetworkReady" && c.status));
    assert!(st.info.contains_key("config"));

    // 3. RunPodSandbox + PodSandboxStatus over Unix socket
    let pod_id = rt
        .run_pod_sandbox(RunPodSandboxRequest {
            config: Some(PodSandboxConfig {
                metadata: Some(PodSandboxMetadata {
                    name: "k8s-uds-pod".into(),
                    uid: "k8s-uds-uid".into(),
                    namespace: "kube-system".into(),
                    attempt: 0,
                }),
                hostname: "k8s-uds".into(),
                log_directory: String::new(),
                labels: HashMap::new(),
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
    assert_eq!(pod_st.id, pod_id);
    assert_eq!(pod_st.runtime_handler, "coral");

    rt.remove_pod_sandbox(RemovePodSandboxRequest {
        pod_sandbox_id: pod_id,
    })
    .await
    .unwrap();

    svc_ctrl.shutdown().await;
    handle.abort();
}
