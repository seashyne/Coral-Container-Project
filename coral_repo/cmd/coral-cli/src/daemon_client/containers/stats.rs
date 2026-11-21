//! `coral stats` — live CPU / memory / PID / health metrics.

use crate::catalog;
use crate::daemon_client::{container_state_label, short_id, try_connect_channel};
use anyhow::{Context, Result};
use coral_api::runtime_service_client::RuntimeServiceClient;
use coral_api::{ContainerState, ContainerStatusRequest};
use coral_daemon::service::metrics::LiveContainerMetrics;
use std::io::Write;
use std::time::Duration;

pub async fn cmd_stats(addr: &str, id: &str, watch: bool, interval_ms: u64) -> Result<bool> {
    let Some(ch) = try_connect_channel(addr).await else {
        return Ok(false);
    };
    let mut rt = RuntimeServiceClient::new(ch);
    let first = rt
        .container_status(ContainerStatusRequest {
            container_id: id.to_string(),
            verbose: true,
        })
        .await;
    let Ok(resp) = first else {
        return Ok(false);
    };
    let mut current = resp.into_inner();
    let step = Duration::from_millis(interval_ms.clamp(100, 60_000));

    println!(
        "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} SOURCE",
        "CONTAINER", "STATE", "HEALTH", "PID", "MEMORY", "CPU TIME", "PIDS", "RESTARTS"
    );

    loop {
        let st = current.status.as_ref();
        let state_str = st
            .map(|s| container_state_label(s.state))
            .unwrap_or("unknown");
        let health = current
            .info
            .get("healthStatus")
            .map(String::as_str)
            .unwrap_or("none");
        let restarts = current
            .info
            .get("restartCount")
            .map(String::as_str)
            .unwrap_or("0");
        let metrics = current
            .info
            .get("metricsJson")
            .and_then(|j| serde_json::from_str::<LiveContainerMetrics>(j).ok());

        if let Some(m) = metrics {
            let mem_str = if m.memory_limit_bytes > 0 {
                format!(
                    "{} / {}",
                    catalog::format_bytes(m.memory_bytes),
                    catalog::format_bytes(m.memory_limit_bytes)
                )
            } else {
                catalog::format_bytes(m.memory_bytes)
            };
            let cpu_ms = m.cpu_usage_usec as f64 / 1000.0;
            println!(
                "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} {}",
                short_id(id),
                state_str,
                health,
                m.pid,
                mem_str,
                format!("{cpu_ms:.1}ms"),
                m.pids_current,
                restarts,
                m.source
            );
        } else {
            println!(
                "{:<18} {:<10} {:<10} {:<8} {:<20} {:<12} {:<6} {:<8} -",
                short_id(id),
                state_str,
                health,
                "-",
                "-",
                "-",
                "-",
                restarts
            );
        }
        let _ = std::io::stdout().flush();

        if !watch || st.is_some_and(|s| s.state == ContainerState::ContainerExited as i32) {
            break;
        }
        tokio::time::sleep(step).await;
        current = rt
            .container_status(ContainerStatusRequest {
                container_id: id.to_string(),
                verbose: true,
            })
            .await
            .with_context(|| format!("poll metrics for container '{id}'"))?
            .into_inner();
    }

    Ok(true)
}
