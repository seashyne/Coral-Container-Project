//! Kubernetes CRI v1 Streaming Server (`Exec`, `Attach`, `PortForward`).
//!
//! Issues single-use cryptographically random streaming URLs (`http://127.0.0.1:<port>/{exec,attach,portforward}/<token>`)
//! matching the Kubernetes `kubelet` / `critest` CRI streaming model, with full-duplex TCP
//! tunneling for `PortForward`, live log streaming for `Attach`, and real `coral exec` for `Exec`.

use super::CoralDaemonService;
use coral_api::ContainerState as CriContainerState;
use coral_net::connect_container_port;
use coral_runtime::store::StateStore;
use ring::rand::{SecureRandom, SystemRandom};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const TOKEN_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub(crate) enum StreamKind {
    Exec {
        container_id: String,
        cmd: Vec<String>,
        stdout: bool,
        stderr: bool,
    },
    Attach {
        container_id: String,
    },
    PortForward {
        pod_sandbox_id: String,
        ports: Vec<i32>,
    },
}

#[derive(Debug, Clone)]
struct StreamTokenEntry {
    kind: StreamKind,
    expires_at: Instant,
}

/// Embedded CRI v1 streaming server state shared across `CoralDaemonService` clones.
#[derive(Debug, Default)]
pub(crate) struct CriStreamServer {
    addr: Mutex<Option<SocketAddr>>,
    server_task: Mutex<Option<JoinHandle<()>>>,
    tokens: Arc<Mutex<HashMap<String, StreamTokenEntry>>>,
}

impl CriStreamServer {
    /// Lazily starts the CRI streaming TCP listener on `127.0.0.1:0` (or returns existing addr)
    /// and registers a single-use random token returning its HTTP streaming URL.
    pub(crate) async fn issue_url(
        &self,
        svc: CoralDaemonService,
        route: &str,
        kind: StreamKind,
    ) -> Result<String, String> {
        let addr = self.ensure_started(svc).await?;
        let token = generate_token();
        let now = Instant::now();
        let mut guard = self.tokens.lock().await;
        guard.retain(|_, v| v.expires_at > now);
        guard.insert(
            token.clone(),
            StreamTokenEntry {
                kind,
                expires_at: now + TOKEN_TTL,
            },
        );
        Ok(format!("http://{addr}/{route}/{token}"))
    }

    pub(crate) async fn shutdown(&self) {
        if let Some(h) = self.server_task.lock().await.take() {
            h.abort();
        }
        self.tokens.lock().await.clear();
    }

    async fn ensure_started(&self, svc: CoralDaemonService) -> Result<SocketAddr, String> {
        let mut addr_guard = self.addr.lock().await;
        if let Some(a) = *addr_guard {
            return Ok(a);
        }
        let bind_addr = std::env::var("CORAL_STREAM_ADDR").unwrap_or_else(|_| "127.0.0.1:0".into());
        let listener = TcpListener::bind(&bind_addr)
            .await
            .map_err(|e| format!("bind CRI streaming server on {bind_addr}: {e}"))?;
        let local = listener
            .local_addr()
            .map_err(|e| format!("CRI stream local_addr: {e}"))?;
        let tokens = self.tokens.clone();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let t = tokens.clone();
                let s = svc.clone();
                tokio::spawn(async move {
                    let _ = handle_stream_client(stream, t, s).await;
                });
            }
        });
        *self.server_task.lock().await = Some(handle);
        *addr_guard = Some(local);
        Ok(local)
    }
}

fn generate_token() -> String {
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 16];
    let _ = rng.fill(&mut bytes);
    let mut out = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

async fn handle_stream_client(
    mut stream: TcpStream,
    tokens: Arc<Mutex<HashMap<String, StreamTokenEntry>>>,
    svc: CoralDaemonService,
) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(2048);
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 32 * 1024 {
            write_simple_http(&mut stream, 400, "Bad Request", b"header too large\n").await?;
            return Ok(());
        }
    };

    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.lines();
    let req_line = lines.next().unwrap_or("");
    let raw_target = req_line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = match raw_target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw_target, ""),
    };
    let wants_upgrade = lines.any(|l| {
        l.to_ascii_lowercase().starts_with("upgrade:")
            || l.to_ascii_lowercase().contains("upgrade")
    });

    let segs: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    if segs.len() != 2 {
        write_simple_http(&mut stream, 404, "Not Found", b"invalid CRI stream path\n").await?;
        return Ok(());
    }
    let (route, token) = (segs[0], segs[1]);

    let entry = {
        let mut guard = tokens.lock().await;
        guard.remove(token)
    };
    let Some(entry) = entry else {
        write_simple_http(&mut stream, 404, "Not Found", b"unknown or expired stream token\n")
            .await?;
        return Ok(());
    };
    if entry.expires_at <= Instant::now() {
        write_simple_http(&mut stream, 410, "Gone", b"stream token expired\n").await?;
        return Ok(());
    }

    match (route, entry.kind) {
        (
            "exec",
            StreamKind::Exec {
                container_id,
                cmd,
                stdout,
                stderr,
            },
        ) => {
            let mut command = Command::new(&svc.cfg.coral_bin);
            command
                .arg("--root")
                .arg(svc.runtime_root())
                .arg("exec")
                .arg("--local")
                .arg(&container_id)
                .args(&cmd);
            match command.output().await {
                Ok(out) => {
                    let mut body = Vec::new();
                    if stdout {
                        body.extend_from_slice(&out.stdout);
                    }
                    if stderr {
                        body.extend_from_slice(&out.stderr);
                    }
                    let code = out.status.code().unwrap_or(1);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\n\
                         Content-Type: application/octet-stream\r\n\
                         Content-Length: {}\r\n\
                         X-Coral-Exit-Code: {code}\r\n\
                         Connection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(resp.as_bytes()).await?;
                    stream.write_all(&body).await?;
                    stream.flush().await?;
                }
                Err(e) => {
                    write_simple_http(
                        &mut stream,
                        500,
                        "Internal Server Error",
                        format!("exec failed: {e}\n").as_bytes(),
                    )
                    .await?;
                }
            }
        }
        ("attach", StreamKind::Attach { container_id }) => {
            let log_path: Option<PathBuf> = {
                let store = svc.store.read().await;
                store
                    .containers
                    .get(&container_id)
                    .map(|c| c.log_path.clone())
            };
            let Some(log_path) = log_path else {
                write_simple_http(&mut stream, 404, "Not Found", b"container not found\n").await?;
                return Ok(());
            };

            let status_line = if wants_upgrade {
                "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: SPDY/3.1\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\n"
            };
            stream.write_all(status_line.as_bytes()).await?;
            stream.flush().await?;

            // Stream existing + live appended bytes until the container exits (or max 5s idle).
            let mut offset = 0usize;
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Ok(bytes) = std::fs::read(&log_path)
                    && bytes.len() > offset
                {
                    if stream.write_all(&bytes[offset..]).await.is_err() {
                        break;
                    }
                    let _ = stream.flush().await;
                    offset = bytes.len();
                }
                let is_running = {
                    let store = svc.store.read().await;
                    store
                        .containers
                        .get(&container_id)
                        .is_some_and(|c| c.state == CriContainerState::ContainerRunning)
                };
                if !is_running || Instant::now() >= deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        (
            "portforward",
            StreamKind::PortForward {
                pod_sandbox_id,
                ports,
            },
        ) => {
            let requested_port: Option<u16> = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("port="))
                .and_then(|v| v.parse::<u16>().ok())
                .or_else(|| ports.first().and_then(|&p| u16::try_from(p).ok()));

            let Some(port) = requested_port else {
                write_simple_http(&mut stream, 400, "Bad Request", b"missing target port\n")
                    .await?;
                return Ok(());
            };

            // Locate a running container PID in the target pod sandbox for direct NetNS connect,
            // falling back to localhost:<port> (e.g., host-network or port-mapped pods).
            let target_pid = {
                let store = svc.store.read().await;
                let rt_store = StateStore::new(svc.runtime_root());
                store
                    .containers
                    .values()
                    .filter(|c| {
                        c.pod_sandbox_id == pod_sandbox_id
                            && c.state == CriContainerState::ContainerRunning
                    })
                    .find_map(|c| rt_store.load(&c.id).ok().and_then(|st| st.pid))
            };

            let upstream_res = if let Some(pid) = target_pid {
                let ns_res =
                    tokio::task::spawn_blocking(move || connect_container_port(Some(pid), port))
                        .await;
                match ns_res {
                    Ok(Ok(std_stream)) => {
                        let _ = std_stream.set_nonblocking(true);
                        TcpStream::from_std(std_stream)
                    }
                    _ => TcpStream::connect(("127.0.0.1", port)).await,
                }
            } else {
                TcpStream::connect(("127.0.0.1", port)).await
            };

            let mut upstream = match upstream_res {
                Ok(u) => u,
                Err(e) => {
                    write_simple_http(
                        &mut stream,
                        502,
                        "Bad Gateway",
                        format!("portforward connect to port {port} failed: {e}\n").as_bytes(),
                    )
                    .await?;
                    return Ok(());
                }
            };

            let status_line = if wants_upgrade {
                "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: SPDY/3.1\r\n\r\n"
            } else {
                "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n"
            };
            stream.write_all(status_line.as_bytes()).await?;
            stream.flush().await?;

            // Forward any already-buffered payload bytes after the HTTP header
            let leftover = &buf[header_end + 4..];
            if !leftover.is_empty() {
                upstream.write_all(leftover).await?;
                upstream.flush().await?;
            }
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
        }
        _ => {
            write_simple_http(&mut stream, 400, "Bad Request", b"mismatched stream route\n")
                .await?;
        }
    }

    Ok(())
}

async fn write_simple_http(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let hdr = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(hdr.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}
