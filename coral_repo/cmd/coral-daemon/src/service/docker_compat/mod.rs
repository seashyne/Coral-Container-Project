//! Docker Engine HTTP REST API (`/v1.24`..`/v1.47`) compatibility shim over Unix socket
//! (`docker.sock`) and TCP, allowing Docker API clients (Testcontainers, Traefik, Portainer,
//! Docker CLI over `DOCKER_HOST=unix://.../docker.sock`) to query and manage Coral containers.

mod containers;
mod handlers;
#[cfg(test)]
mod tests;

use super::CoralDaemonService;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};
use tokio::sync::watch;
use tracing::warn;

pub struct HttpResponse {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(status: &'static str, val: &serde_json::Value) -> Self {
        Self {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(val).unwrap_or_else(|_| b"{}".to_vec()),
        }
    }

    pub fn text(status: &'static str, text: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: text.into().into_bytes(),
        }
    }

    pub fn empty(status: &'static str) -> Self {
        Self {
            status,
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        }
    }
}

/// Serves the Docker Engine REST API compatibility shim on a Unix Domain Socket (`docker.sock`).
pub async fn serve_docker_unix(
    svc: CoralDaemonService,
    listener: UnixListener,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        let accepted = tokio::select! {
            _ = shutdown.changed() => break,
            res = listener.accept() => res,
        };
        let Ok((stream, _)) = accepted else {
            continue;
        };
        let svc_c = svc.clone();
        tokio::spawn(async move {
            handle_http_connection(svc_c, stream).await;
        });
    }
    Ok(())
}

/// Serves the Docker Engine REST API compatibility shim on a TCP listener.
pub async fn serve_docker_tcp(
    svc: CoralDaemonService,
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    loop {
        let accepted = tokio::select! {
            _ = shutdown.changed() => break,
            res = listener.accept() => res,
        };
        let Ok((stream, _)) = accepted else {
            continue;
        };
        let svc_c = svc.clone();
        tokio::spawn(async move {
            handle_http_connection(svc_c, stream).await;
        });
    }
    Ok(())
}

async fn handle_http_connection<S>(svc: CoralDaemonService, mut stream: S)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some((method, target, body)) = read_http_request(&mut stream).await else {
        return;
    };
    let (raw_path, query) = target.split_once('?').unwrap_or((&target, ""));
    let route = strip_api_version(raw_path);
    let resp = handlers::dispatch_docker_request(&svc, &method, route, query, &body).await;

    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nApi-Version: 1.45\r\nDocker-Experimental: false\r\nServer: Coral/{} (linux)\r\nConnection: close\r\n\r\n",
        resp.status,
        resp.content_type,
        resp.body.len(),
        env!("CARGO_PKG_VERSION")
    );
    if let Err(e) = stream.write_all(header.as_bytes()).await {
        warn!(err = %e, "write docker_compat header failed");
        return;
    }
    if !resp.body.is_empty()
        && let Err(e) = stream.write_all(&resp.body).await
    {
        warn!(err = %e, "write docker_compat body failed");
    }
    let _ = stream.shutdown().await;
}

/// Strips an optional `/v1.XX` version prefix from a Docker REST API path.
pub fn strip_api_version(path: &str) -> &str {
    if let Some(after_v) = path.strip_prefix("/v")
        && let Some(slash_idx) = after_v.find('/')
    {
        let ver = &after_v[..slash_idx];
        if !ver.is_empty() && ver.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return &after_v[slash_idx..];
        }
    }
    path
}

async fn read_http_request<S>(stream: &mut S) -> Option<(String, String, Vec<u8>)>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };

    let header_str = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = header_str.lines();
    let req_line = lines.next()?;
    let mut parts = req_line.split_whitespace();
    let method = parts.next()?.to_ascii_uppercase();
    let target = parts.next()?.to_string();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
            && let Ok(len) = v.trim().parse::<usize>()
        {
            content_length = len.min(4 * 1024 * 1024);
        }
    }

    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Some((method, target, body))
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}
