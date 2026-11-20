//! Prometheus OpenMetrics `/metrics` and `/healthz` HTTP exposition endpoint
//! for `coral-daemon` (zero external HTTP server dependencies).

use super::CoralDaemonService;
use super::metrics::collect_container_metrics;
use coral_api::{ContainerState as CriContainerState, PodSandboxState};
use std::collections::HashSet;
use std::fmt::Write as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::warn;

/// Renders live daemon, container, sandbox, image, and Auto-GC gauges/counters
/// in Prometheus text exposition format (`0.0.4`).
pub async fn render_prometheus_metrics(svc: &CoralDaemonService) -> String {
    let runtime_root = svc.runtime_root();
    let (
        created,
        running,
        exited,
        pods_ready,
        pods_notready,
        images_count,
        images_bytes,
        auto_gc_runs,
        auto_gc_bytes,
        ctr_snapshots,
    ) = {
        let store = svc.store.read().await;
        let mut created = 0usize;
        let mut running = 0usize;
        let mut exited = 0usize;
        for c in store.containers.values() {
            match c.state {
                CriContainerState::ContainerCreated => created += 1,
                CriContainerState::ContainerRunning => running += 1,
                CriContainerState::ContainerExited => exited += 1,
                _ => {}
            }
        }
        let mut pods_ready = 0usize;
        let mut pods_notready = 0usize;
        for p in store.sandboxes.values() {
            if p.state == PodSandboxState::SandboxReady {
                pods_ready += 1;
            } else {
                pods_notready += 1;
            }
        }
        let mut seen = HashSet::new();
        let mut img_bytes = 0u64;
        for img in &store.images {
            if seen.insert(img.rootfs_path.clone()) {
                img_bytes = img_bytes.saturating_add(img.size);
            }
        }
        let snapshots: Vec<(String, String, bool, u32, i64, i64)> = store
            .containers
            .values()
            .map(|c| {
                let name = c
                    .config
                    .metadata
                    .as_ref()
                    .map(|m| m.name.clone())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| c.id.clone());
                let mem_limit = c
                    .config
                    .linux
                    .as_ref()
                    .and_then(|l| l.resources.as_ref())
                    .map(|r| r.memory_limit_in_bytes)
                    .unwrap_or(0);
                (
                    c.id.clone(),
                    name,
                    c.state == CriContainerState::ContainerRunning,
                    c.restart_count,
                    mem_limit,
                    c.started_at,
                )
            })
            .collect();
        (
            created,
            running,
            exited,
            pods_ready,
            pods_notready,
            store.images.len(),
            img_bytes,
            store.auto_gc_runs,
            store.auto_gc_bytes_reclaimed,
            snapshots,
        )
    };

    let mut out = String::with_capacity(2048);
    let ver = env!("CARGO_PKG_VERSION");
    let _ = writeln!(out, "# HELP coral_daemon_info Coral daemon build metadata.");
    let _ = writeln!(out, "# TYPE coral_daemon_info gauge");
    let _ = writeln!(out, "coral_daemon_info{{version=\"{}\"}} 1", esc_label(ver));

    let _ = writeln!(out, "# HELP coral_containers_total Containers tracked by state.");
    let _ = writeln!(out, "# TYPE coral_containers_total gauge");
    let _ = writeln!(out, "coral_containers_total{{state=\"created\"}} {created}");
    let _ = writeln!(out, "coral_containers_total{{state=\"running\"}} {running}");
    let _ = writeln!(out, "coral_containers_total{{state=\"exited\"}} {exited}");

    let _ = writeln!(out, "# HELP coral_pods_total Pod sandboxes tracked by state.");
    let _ = writeln!(out, "# TYPE coral_pods_total gauge");
    let _ = writeln!(out, "coral_pods_total{{state=\"ready\"}} {pods_ready}");
    let _ = writeln!(out, "coral_pods_total{{state=\"notready\"}} {pods_notready}");

    let _ = writeln!(out, "# HELP coral_images_total OCI images registered in daemon.");
    let _ = writeln!(out, "# TYPE coral_images_total gauge");
    let _ = writeln!(out, "coral_images_total {images_count}");

    let _ = writeln!(out, "# HELP coral_images_bytes_total Total deduplicated image rootfs bytes.");
    let _ = writeln!(out, "# TYPE coral_images_bytes_total gauge");
    let _ = writeln!(out, "coral_images_bytes_total {images_bytes}");

    let _ = writeln!(out, "# HELP coral_daemon_auto_gc_runs_total Background Auto-GC sweeps executed.");
    let _ = writeln!(out, "# TYPE coral_daemon_auto_gc_runs_total counter");
    let _ = writeln!(out, "coral_daemon_auto_gc_runs_total {auto_gc_runs}");

    let _ = writeln!(
        out,
        "# HELP coral_daemon_auto_gc_bytes_reclaimed_total Bytes reclaimed by background Auto-GC."
    );
    let _ = writeln!(out, "# TYPE coral_daemon_auto_gc_bytes_reclaimed_total counter");
    let _ = writeln!(out, "coral_daemon_auto_gc_bytes_reclaimed_total {auto_gc_bytes}");

    let _ = writeln!(out, "# HELP coral_container_restarts_total Supervisor restart count per container.");
    let _ = writeln!(out, "# TYPE coral_container_restarts_total counter");
    for (id, name, _, restarts, _, _) in &ctr_snapshots {
        let _ = writeln!(
            out,
            "coral_container_restarts_total{{id=\"{}\",name=\"{}\"}} {restarts}",
            esc_label(id),
            esc_label(name)
        );
    }

    let _ = writeln!(out, "# HELP coral_container_memory_bytes Live RSS/cgroup memory usage in bytes.");
    let _ = writeln!(out, "# TYPE coral_container_memory_bytes gauge");
    let _ = writeln!(out, "# HELP coral_container_memory_limit_bytes Configured memory cap in bytes.");
    let _ = writeln!(out, "# TYPE coral_container_memory_limit_bytes gauge");
    let _ = writeln!(out, "# HELP coral_container_cpu_usage_seconds_total CPU seconds consumed.");
    let _ = writeln!(out, "# TYPE coral_container_cpu_usage_seconds_total counter");
    let _ = writeln!(out, "# HELP coral_container_pids Current live process/thread count.");
    let _ = writeln!(out, "# TYPE coral_container_pids gauge");

    for (id, name, is_running, _, mem_limit, started_at) in &ctr_snapshots {
        if !*is_running {
            continue;
        }
        if let Some(m) = collect_container_metrics(&runtime_root, id, *mem_limit, *started_at) {
            let lbl = format!("id=\"{}\",name=\"{}\"", esc_label(id), esc_label(name));
            let _ = writeln!(out, "coral_container_memory_bytes{{{lbl}}} {}", m.memory_bytes);
            let _ = writeln!(
                out,
                "coral_container_memory_limit_bytes{{{lbl}}} {}",
                m.memory_limit_bytes
            );
            let _ = writeln!(
                out,
                "coral_container_cpu_usage_seconds_total{{{lbl}}} {:.6}",
                m.cpu_usage_usec as f64 / 1_000_000.0
            );
            let _ = writeln!(out, "coral_container_pids{{{lbl}}} {}", m.pids_current);
        }
    }

    out
}

/// Serves HTTP/1.1 `/metrics` and `/healthz` until `shutdown` signals true.
pub async fn serve_prometheus_metrics(
    svc: CoralDaemonService,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        let accepted = tokio::select! {
            _ = shutdown.changed() => break,
            res = listener.accept() => res,
        };
        let Ok((mut stream, _)) = accepted else {
            continue;
        };
        let svc_c = svc.clone();
        tokio::spawn(async move {
            let mut req_buf = [0u8; 1024];
            let Ok(n) = stream.read(&mut req_buf).await else {
                return;
            };
            let first_line = String::from_utf8_lossy(&req_buf[..n])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let path = first_line.split_whitespace().nth(1).unwrap_or("/");
            let (status_line, content_type, body) = if path == "/metrics" {
                (
                    "200 OK",
                    "text/plain; version=0.0.4; charset=utf-8",
                    render_prometheus_metrics(&svc_c).await,
                )
            } else if path == "/healthz" || path == "/readyz" {
                ("200 OK", "text/plain; charset=utf-8", "ok\n".to_string())
            } else {
                ("404 Not Found", "text/plain; charset=utf-8", "not found\n".to_string())
            };
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            if let Err(e) = stream.write_all(resp.as_bytes()).await {
                warn!(err = %e, "write prometheus response failed");
            }
            let _ = stream.shutdown().await;
        });
    }
    Ok(())
}

fn esc_label(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for ch in v.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::CoralDaemonConfig;
    use std::path::PathBuf;

    #[tokio::test]
    async fn renders_prometheus_metrics_and_serves_http() {
        let dir = tempfile::tempdir().unwrap();
        let svc = CoralDaemonService::new(CoralDaemonConfig {
            state_dir: dir.path().join("state"),
            images_dir: Some(dir.path().join("images")),
            coral_bin: PathBuf::from("coral"),
            default_rootfs: None,
        });
        let text = render_prometheus_metrics(&svc).await;
        assert!(text.contains("coral_daemon_info{version="));
        assert!(text.contains("coral_containers_total{state=\"running\"} 0"));
        assert!(text.contains("coral_daemon_auto_gc_runs_total 0"));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = watch::channel(false);
        let h = tokio::spawn(serve_prometheus_metrics(svc, listener, rx));

        let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut body = String::new();
        conn.read_to_string(&mut body).await.unwrap();
        assert!(body.starts_with("HTTP/1.1 200 OK"));
        assert!(body.contains("coral_images_total 0"));

        let _ = tx.send(true);
        let _ = h.await;
    }
}
