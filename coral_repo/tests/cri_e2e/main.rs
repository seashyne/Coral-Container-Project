//! CRI v1 end-to-end tests for the Coral Container Runtime.
//!
//! The suite is split thematically into submodules. Every test lives in exactly
//! one submodule, so the set of executed tests is identical to the previous flat
//! `cri_e2e.rs` file.

mod sandbox_lifecycle;
mod image_lifecycle;
mod compose_lifecycle;
mod persistence_healthcheck;
mod boot_resume_prune;
mod exec_stream_network;
mod wasm_oci_workload;
mod kubelet_cni;
mod kubernetes_socket;
mod udp_portforward_compose;

use coral_api::image_service_client::ImageServiceClient;
use coral_api::image_service_server::ImageServiceServer;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::runtime_service_server::RuntimeServiceServer;
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ExecSyncRequest, ImageSpec, KeyValue, LinuxContainerConfig,
    LinuxContainerResources, ListContainersRequest, ListImagesRequest, ListPodSandboxRequest,
    PodSandboxConfig, PodSandboxMetadata, PullImageRequest, RemoveContainerRequest,
    RemoveImageRequest, RemovePodSandboxRequest, RunPodSandboxRequest, StartContainerRequest,
    StopContainerRequest, StopPodSandboxRequest, VersionRequest,
};
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

/// Splits an `http://host/path` URL into its host and path components.
pub(crate) fn parse_http_url(url: &str) -> (String, String) {
    let rest = url.strip_prefix("http://").unwrap_or(url);
    match rest.split_once('/') {
        Some((h, p)) => (h.to_string(), format!("/{p}")),
        None => (rest.to_string(), "/".into()),
    }
}

/// Performs a plain HTTP GET and returns the full raw response.
pub(crate) async fn http_get_raw(url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (host, path) = parse_http_url(url);
    let mut s = tokio::net::TcpStream::connect(&host).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out
}

/// Performs an HTTP GET and returns the first chunk that arrives.
pub(crate) async fn http_get_first_chunk(url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (host, path) = parse_http_url(url);
    let mut s = tokio::net::TcpStream::connect(&host).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !out.contains("ATTACH_LINE_OK") {
        let Ok(Ok(n)) = tokio::time::timeout(Duration::from_millis(500), s.read(&mut buf)).await else {
            break;
        };
        if n == 0 {
            break;
        }
        out.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    out
}
