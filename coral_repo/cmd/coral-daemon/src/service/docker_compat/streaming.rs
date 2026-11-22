//! Streaming and HTTP Hijack (`Upgrade: tcp`) handlers for `docker.sock`:
//! - `POST /exec/{id}/start` with `Upgrade: tcp` (`101 UPGRADED` raw stream hijack)
//! - `GET /containers/{id}/logs?follow=1` (`Transfer-Encoding: chunked` live log tail)
//! - `GET /events?stream=1` / `until=<future>` (`Transfer-Encoding: chunked` live event stream)

use super::handlers::parse_query_param;
use super::state::{DockerCompatState, encode_docker_stream_frame};
use crate::service::CoralDaemonService;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{ContainerState as CriContainerState, ExecSyncRequest};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tonic::Request;

/// Attempts to handle a streaming or HTTP-hijacked Docker Engine API request directly on `stream`.
/// Returns `Ok(true)` if handled, or `Ok(false)` to fall through to normal request/response dispatch.
pub(super) async fn try_handle_streaming_request<S>(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    method: &str,
    route: &str,
    query: &str,
    headers: &HashMap<String, String>,
    stream: &mut S,
) -> std::io::Result<bool>
where
    S: AsyncWrite + Unpin,
{
    let wants_upgrade = headers
        .get("upgrade")
        .is_some_and(|v| v.eq_ignore_ascii_case("tcp"))
        || headers
            .get("connection")
            .is_some_and(|v| v.to_ascii_lowercase().contains("upgrade"));

    // 1. HTTP Hijack for `POST /exec/{id}/start`
    if method == "POST"
        && wants_upgrade
        && let Some(rest) = route.strip_prefix("/exec/")
        && let Some(exec_id) = rest.strip_suffix("/start")
    {
        handle_hijacked_exec_start(svc, compat, exec_id.trim_matches('/'), stream).await?;
        return Ok(true);
    }

    // 2. Chunked live log streaming for `GET /containers/{id}/logs?follow=1`
    if method == "GET"
        && let Some(rest) = route.strip_prefix("/containers/")
        && let Some(cid_raw) = rest.strip_suffix("/logs")
        && parse_query_param(query, "follow")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        stream_container_logs_chunked(svc, cid_raw.trim_matches('/'), stream).await?;
        return Ok(true);
    }

    // 3. Chunked live event stream for `GET /events?stream=1` or `until=<future>`
    if method == "GET" && route == "/events" {
        let explicit_stream = parse_query_param(query, "stream")
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        let until_epoch = parse_query_param(query, "until").and_then(|s| s.parse::<i64>().ok());
        let now_epoch = chrono::Utc::now().timestamp();
        if explicit_stream || until_epoch.is_some_and(|u| u > now_epoch) {
            let since = parse_query_param(query, "since").and_then(|s| s.parse::<i64>().ok());
            stream_events_chunked(compat, since, until_epoch, stream).await?;
            return Ok(true);
        }
    }

    Ok(false)
}

async fn handle_hijacked_exec_start<S>(
    svc: &CoralDaemonService,
    compat: &DockerCompatState,
    exec_id: &str,
    stream: &mut S,
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let rec = compat.execs.read().await.get(exec_id).cloned();
    let Some(rec) = rec else {
        let msg = br#"{"message":"no such exec"}"#;
        let hdr = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            msg.len()
        );
        stream.write_all(hdr.as_bytes()).await?;
        stream.write_all(msg).await?;
        return Ok(());
    };

    let upgrade_hdr = concat!(
        "HTTP/1.1 101 UPGRADED\r\n",
        "Content-Type: application/vnd.docker.raw-stream\r\n",
        "Connection: Upgrade\r\n",
        "Upgrade: tcp\r\n",
        "Api-Version: 1.45\r\n\r\n"
    );
    stream.write_all(upgrade_hdr.as_bytes()).await?;
    stream.flush().await?;

    if let Ok(resp) = svc
        .exec_sync(Request::new(ExecSyncRequest {
            container_id: rec.container_id.clone(),
            cmd: rec.cmd.clone(),
            timeout: 30,
        }))
        .await
    {
        let inner = resp.into_inner();
        if let Some(entry) = compat.execs.write().await.get_mut(exec_id) {
            entry.running = false;
            entry.exit_code = inner.exit_code;
        }
        let out_frame = encode_docker_stream_frame(1, &inner.stdout, rec.tty);
        let err_frame = encode_docker_stream_frame(2, &inner.stderr, rec.tty);
        if !out_frame.is_empty() {
            stream.write_all(&out_frame).await?;
        }
        if !err_frame.is_empty() {
            stream.write_all(&err_frame).await?;
        }
        stream.flush().await?;
    }
    Ok(())
}

async fn stream_container_logs_chunked<S>(
    svc: &CoralDaemonService,
    query_id: &str,
    stream: &mut S,
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let resolved = {
        let store = svc.store.read().await;
        store
            .resolve_container_id(query_id)
            .and_then(|cid| store.containers.get(&cid).map(|c| (cid, c.log_path.clone())))
    };
    let Some((cid, log_path)) = resolved else {
        let msg = br#"{"message":"container not found"}"#;
        let hdr = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            msg.len()
        );
        stream.write_all(hdr.as_bytes()).await?;
        stream.write_all(msg).await?;
        return Ok(());
    };

    let hdr = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/vnd.docker.raw-stream\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Api-Version: 1.45\r\n\r\n"
    );
    stream.write_all(hdr.as_bytes()).await?;
    stream.flush().await?;

    let mut offset = 0usize;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(bytes) = fs::read(&log_path)
            && bytes.len() > offset
        {
            let slice = &bytes[offset..];
            offset = bytes.len();
            let frame = encode_docker_stream_frame(1, slice, false);
            if write_http_chunk(stream, &frame).await.is_err() {
                return Ok(());
            }
        }
        let still_running = {
            let store = svc.store.read().await;
            store
                .containers
                .get(&cid)
                .is_some_and(|c| c.state == CriContainerState::ContainerRunning)
        };
        if !still_running || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }

    let _ = stream.write_all(b"0\r\n\r\n").await;
    let _ = stream.flush().await;
    Ok(())
}

async fn stream_events_chunked<S>(
    compat: &DockerCompatState,
    since: Option<i64>,
    until_epoch: Option<i64>,
    stream: &mut S,
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let hdr = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Content-Type: application/json\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Api-Version: 1.45\r\n\r\n"
    );
    stream.write_all(hdr.as_bytes()).await?;
    stream.flush().await?;

    let mut rx = compat.subscribe_events();
    for ev in compat.snapshot_events(since).await {
        if let Ok(mut line) = serde_json::to_string(&ev) {
            line.push('\n');
            if write_http_chunk(stream, line.as_bytes()).await.is_err() {
                return Ok(());
            }
        }
    }

    let max_wait = until_epoch
        .map(|u| {
            let diff = u.saturating_sub(chrono::Utc::now().timestamp()).clamp(1, 30);
            Duration::from_secs(diff as u64)
        })
        .unwrap_or(Duration::from_secs(15));
    let deadline = Instant::now() + max_wait;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => {
                if let Ok(mut line) = serde_json::to_string(&ev) {
                    line.push('\n');
                    if write_http_chunk(stream, line.as_bytes()).await.is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => break,
        }
    }

    let _ = stream.write_all(b"0\r\n\r\n").await;
    let _ = stream.flush().await;
    Ok(())
}

async fn write_http_chunk<S: AsyncWrite + Unpin>(stream: &mut S, data: &[u8]) -> std::io::Result<()> {
    if data.is_empty() {
        return Ok(());
    }
    let header = format!("{:x}\r\n", data.len());
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(data).await?;
    stream.write_all(b"\r\n").await?;
    stream.flush().await
}
