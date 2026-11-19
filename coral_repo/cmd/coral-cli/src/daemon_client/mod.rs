//! gRPC client helpers for connecting `coral` CLI to `coral-daemon` (`RuntimeService` & `ImageService`).

pub mod compose;
pub mod containers;
pub mod images;
pub mod pods;
pub mod system;

pub use compose::{cmd_compose_down, cmd_compose_logs, cmd_compose_ps, cmd_compose_up};
pub use containers::{
    DaemonRunOptions, cmd_diff_via_daemon, cmd_exec, cmd_logs, cmd_ps, cmd_rm, cmd_run_via_daemon,
    cmd_stats, cmd_stop,
};
pub use images::{
    cmd_build, cmd_catalog, cmd_commit_via_daemon, cmd_images, cmd_load, cmd_pull, cmd_rmi,
    cmd_save, cmd_tag,
};
pub use pods::{cmd_pods_list, cmd_pods_rm, cmd_pods_stop};
pub use system::{
    cmd_inspect_via_daemon, cmd_login, cmd_logout, cmd_push, cmd_system_df, cmd_system_prune,
};

use anyhow::{Context, Result};
use chrono::DateTime;
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, VersionRequest};
use coral_daemon::images as daemon_images;
use std::path::PathBuf;
use std::time::Duration;
use tonic::transport::Channel;

pub fn normalize_addr(addr: &str) -> String {
    let trimmed = addr.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

pub(crate) async fn connect_channel(addr: &str, timeout_secs: u64) -> Result<Channel> {
    let url = normalize_addr(addr);
    Channel::from_shared(url.clone())
        .with_context(|| format!("invalid daemon address '{url}'"))?
        .connect_timeout(Duration::from_secs(timeout_secs))
        .connect()
        .await
        .with_context(|| format!("connect to coral-daemon at {url}"))
}

pub(crate) async fn try_connect_channel(addr: &str) -> Option<Channel> {
    connect_channel(addr, 1).await.ok()
}

pub(crate) fn format_nanos(nanos: i64) -> String {
    if nanos <= 0 {
        return "-".into();
    }
    let secs = nanos / 1_000_000_000;
    let sub = (nanos % 1_000_000_000).unsigned_abs() as u32;
    DateTime::from_timestamp(secs, sub)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "-".into())
}

pub(crate) fn container_state_label(state: i32) -> &'static str {
    match ContainerState::try_from(state) {
        Ok(ContainerState::ContainerCreated) => "created",
        Ok(ContainerState::ContainerRunning) => "running",
        Ok(ContainerState::ContainerExited) => "exited",
        _ => "unknown",
    }
}

pub(crate) fn short_id(id: &str) -> String {
    if id.len() > 24 {
        format!("{}…", &id[..23])
    } else {
        id.to_string()
    }
}

pub async fn cmd_version(addr: &str, strict: bool) -> Result<()> {
    if !strict {
        println!("coral CLI v{}", env!("CARGO_PKG_VERSION"));
    }
    let ch = if strict {
        Some(connect_channel(addr, 5).await?)
    } else {
        try_connect_channel(addr).await
    };
    if let Some(ch) = ch {
        let mut client = RuntimeServiceClient::new(ch);
        let resp = client
            .version(VersionRequest {
                version: "0.1.0".into(),
            })
            .await
            .with_context(|| format!("query version from coral-daemon at {addr}"))?
            .into_inner();
        println!(
            "runtime={} version={} api={} cri={}",
            resp.runtime_name, resp.runtime_version, resp.runtime_api_version, resp.version
        );
    } else {
        println!("daemon: not connected ({addr})");
    }
    Ok(())
}

pub fn resolve_images_dir(cli_images_dir: Option<PathBuf>) -> PathBuf {
    cli_images_dir.unwrap_or_else(daemon_images::default_images_dir)
}
