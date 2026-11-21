//! Real-registry end-to-end tests for OCI image pulls, live download progress, and
//! automatic pod-sandbox garbage collection.
//!
//! These tests reach out to **live registries** (Docker Hub, GHCR) and therefore `#[ignore]`d by
//! default. Run them explicitly:
//!
//! ```text
//! cargo test -p coral-e2e-tests --test real_image_e2e -- --ignored --test-threads=1
//! ```
//!
//! Unlike `cri_e2e.rs` (which seeds a local Alpine minirootfs), everything here exercises the
//! genuine `PullImage` → registry v2 → layer streaming → `extract_layer` path, so a regression in
//! Bearer auth, multi-arch index resolution, or SHA-256 verification surfaces here.
//!
//! The suite is split thematically into submodules. Every test lives in exactly one
//! submodule, so the set of executed tests is identical to the previous flat
//! `real_image_e2e.rs` file.

mod real_pull;
mod auto_gc;

use coral_api::image_service_client::ImageServiceClient;
use coral_api::image_service_server::ImageServiceServer;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::runtime_service_server::RuntimeServiceServer;
use coral_api::{
    ContainerConfig, ContainerMetadata, ContainerState, ContainerStatusRequest,
    CreateContainerRequest, ImageSpec, KeyValue, ListImagesRequest, ListPodSandboxRequest,
    PodSandboxConfig, PodSandboxMetadata, PullImageRequest, RemovePodSandboxRequest,
    RunPodSandboxRequest, StartContainerRequest,
};
use coral_daemon::images::progress::{PullPhase, PullProgress, PullProgressReporter};
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;

/// Boots a daemon on an ephemeral port and returns its endpoint plus the reloadable service.
pub(crate) async fn spawn_daemon(
    state_dir: PathBuf,
    images_dir: PathBuf,
) -> (
    String,
    CoralDaemonService,
    tokio::task::JoinHandle<()>,
) {
    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir,
        images_dir: Some(images_dir),
        coral_bin: ensure_coral_binary().clone(),
        default_rootfs: None,
    });
    let ctrl = svc.clone();
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
    (format!("http://{addr}"), ctrl, handle)
}

/// Polls a progress snapshot file until it reaches a terminal phase or `deadline` elapses.
pub(crate) async fn wait_for_terminal(path: &std::path::Path, timeout: Duration) -> Option<PullProgress> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(raw) = std::fs::read_to_string(path)
            && let Ok(p) = serde_json::from_str::<PullProgress>(&raw)
            && p.phase.is_terminal()
        {
            return Some(p);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    None
}
