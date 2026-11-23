//! gRPC client helpers for connecting `coral` CLI to `coral-daemon` (`RuntimeService` & `ImageService`).

pub mod compose;
pub mod containers;
pub mod images;
pub mod pods;
pub mod system;

pub use compose::{
    cmd_compose_build, cmd_compose_config, cmd_compose_down, cmd_compose_exec, cmd_compose_logs,
    cmd_compose_ps, cmd_compose_restart, cmd_compose_start, cmd_compose_stop, cmd_compose_up,
    cmd_compose_watch,
};
pub use containers::{
    DaemonRunOptions, cmd_diff_via_daemon, cmd_exec, cmd_logs, cmd_pause, cmd_ps, cmd_rename,
    cmd_rm, cmd_run_via_daemon, cmd_stats, cmd_stats_all, cmd_stop, cmd_top, cmd_unpause,
    cmd_update, cmd_wait,
};
pub use images::{
    ProgressMode, cmd_build_with_options, cmd_catalog, cmd_commit_via_daemon, cmd_images,
    cmd_load, cmd_pull, cmd_rmi, cmd_save, cmd_tag,
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
    let trimmed = addr.trim();
    if let Some(sock_path) = trimmed
        .strip_prefix("unix://")
        .or_else(|| trimmed.starts_with('/').then_some(trimmed))
    {
        let path = PathBuf::from(sock_path);
        let endpoint = tonic::transport::Endpoint::try_from("http://[::]:50051")
            .context("construct dummy URI for Unix socket transport")?
            .connect_timeout(Duration::from_secs(timeout_secs));
        return endpoint
            .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
                let p = path.clone();
                async move {
                    let stream = tokio::net::UnixStream::connect(p).await?;
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(stream))
                }
            }))
            .await
            .with_context(|| format!("connect to coral-daemon over Unix socket '{sock_path}'"));
    }

    let url = normalize_addr(trimmed);
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
        let uid_support = coral_runtime::MultiUidSupport::detect();
        if uid_support.host_uid == 0 {
            println!("userns: rootful (host uid 0)");
        } else if uid_support.is_available() {
            let sub_u = uid_support
                .subuid_ranges
                .first()
                .map(|r| format!("{}:{}", r.start, r.count))
                .unwrap_or_default();
            let sub_g = uid_support
                .subgid_ranges
                .first()
                .map(|r| format!("{}:{}", r.start, r.count))
                .unwrap_or_default();
            println!("userns: multi-uid rootless (subuid={sub_u}, subgid={sub_g})");
        } else {
            println!(
                "userns: single-uid rootless ({})",
                uid_support.unavailable_reason()
            );
        }
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

pub async fn cmd_cri_status(addr: &str, verbose: bool) -> Result<()> {
    let ch = connect_channel(addr, 5).await?;
    let mut client = RuntimeServiceClient::new(ch);
    let resp = client
        .status(coral_api::StatusRequest { verbose })
        .await
        .with_context(|| format!("query CRI Status from coral-daemon at {addr}"))?
        .into_inner();
    let conditions: Vec<serde_json::Value> = resp
        .status
        .map(|s| {
            s.conditions
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "type": c.r#type,
                        "status": c.status,
                        "reason": c.reason,
                        "message": c.message,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let payload = serde_json::json!({
        "endpoint": addr,
        "conditions": conditions,
        "info": resp.info,
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
