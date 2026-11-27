use anyhow::{Context, Result};
use clap::Parser;
use coral_api::image_service_server::ImageServiceServer;
use coral_api::runtime_service_server::RuntimeServiceServer;
use coral_daemon::images::default_images_dir;
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_runtime::default_state_root;
use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tokio::net::UnixListener;
use tokio::sync::watch;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "coral-daemon",
    version,
    about = "Coral Container Runtime CRI v1 gRPC Daemon (TCP + Kubernetes Unix Domain Socket)"
)]
struct Args {
    /// TCP address to listen on for CRI v1 gRPC connections (Coral Desktop & CLI)
    #[arg(long, default_value = "127.0.0.1:50051")]
    listen: SocketAddr,

    /// Optional Unix Domain Socket path for Kubernetes kubelet / crictl CRI v1 (`unix:///...`).
    /// Defaults to `<state_dir>/coral.sock` unless `--no-unix-socket` is specified.
    #[arg(long)]
    unix_socket: Option<PathBuf>,

    /// Disable the CRI v1 Unix Domain Socket listener (TCP only)
    #[arg(long, default_value_t = false)]
    no_unix_socket: bool,

    /// Root directory for daemon state, bundles, and logs
    #[arg(long)]
    state_dir: Option<PathBuf>,

    /// Directory where pulled OCI base images and `images.json` are persisted
    #[arg(long)]
    images_dir: Option<PathBuf>,

    /// Path to the `coral` OCI runtime CLI binary
    #[arg(long, default_value = "coral")]
    coral_bin: PathBuf,

    /// Optional local rootfs directory registered as `alpine:latest`
    #[arg(long)]
    default_rootfs: Option<PathBuf>,

    /// Kubernetes CNI configuration directory (default `/etc/cni/net.d`)
    #[arg(long, env = "CORAL_CNI_CONF_DIR", default_value = "/etc/cni/net.d")]
    cni_conf_dir: PathBuf,

    /// Kubernetes CNI plugin binaries directory (default `/opt/cni/bin`)
    #[arg(long, env = "CORAL_CNI_BIN_DIR", default_value = "/opt/cni/bin")]
    cni_bin_dir: PathBuf,

    /// Maximum active container log file size in MiB before rotation (`0` disables rotation)
    #[arg(long, env = "CORAL_LOG_MAX_SIZE_MB", default_value_t = 10)]
    log_max_size_mb: u64,

    /// Number of archived rotated log files (`.1`..`.N`) kept per container
    #[arg(long, env = "CORAL_LOG_MAX_FILES", default_value_t = 3)]
    log_max_files: usize,

    /// Background Auto-GC sweep interval in seconds (`0` disables Auto-GC)
    #[arg(long, env = "CORAL_AUTO_GC_INTERVAL_SECS", default_value_t = 300)]
    auto_gc_interval_secs: u64,

    /// HTTP address for Prometheus `/metrics` & `/healthz` endpoint
    #[arg(long, env = "CORAL_METRICS_LISTEN", default_value = "127.0.0.1:9108")]
    metrics_listen: SocketAddr,

    /// Disable the Prometheus `/metrics` HTTP listener
    #[arg(long, default_value_t = false)]
    no_metrics: bool,

    /// Optional Docker Engine REST API compatibility Unix socket path (defaults to `<state_dir>/docker.sock`)
    #[arg(long)]
    docker_socket: Option<PathBuf>,

    /// Disable the Docker Engine REST API compatibility Unix socket
    #[arg(long, default_value_t = false)]
    no_docker_socket: bool,

    /// Optional Bearer auth token required on TCP gRPC requests (recommended when binding non-loopback)
    #[arg(long, env = "CORAL_AUTH_TOKEN")]
    auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let explicit_state_dir = args.state_dir.is_some();
    let state_dir = args
        .state_dir
        .unwrap_or_else(|| default_state_root().join("daemon"));
    let images_dir = args.images_dir.unwrap_or_else(|| {
        if explicit_state_dir {
            state_dir.join("images")
        } else {
            default_images_dir()
        }
    });

    let unix_sock_path = if args.no_unix_socket {
        None
    } else {
        Some(
            args.unix_socket
                .unwrap_or_else(|| state_dir.join("coral.sock")),
        )
    };

    let service = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: state_dir.clone(),
        images_dir: Some(images_dir.clone()),
        coral_bin: args.coral_bin,
        default_rootfs: args.default_rootfs,
    })
    .with_cni(args.cni_conf_dir, args.cni_bin_dir)
    .with_log_rotation(
        args.log_max_size_mb.saturating_mul(1024 * 1024),
        args.log_max_files,
    )
    .with_auto_gc(args.auto_gc_interval_secs);
    let (shutdown_tx, mut shutdown_rx_tcp) = watch::channel(false);
    let mut shutdown_rx_uds = shutdown_tx.subscribe();
    let shutdown_rx_metrics = shutdown_tx.subscribe();
    let shutdown_rx_docker = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("received shutdown signal, stopping coral-daemon");
        let _ = shutdown_tx.send(true);
    });

    let sock_mode: u32 = if coral_runtime::MultiUidSupport::detect().host_uid == 0 {
        0o660
    } else {
        0o600
    };
    let auth_token: Option<std::sync::Arc<String>> = args
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| std::sync::Arc::new(s.to_string()));
    if !args.listen.ip().is_loopback() && auth_token.is_none() {
        tracing::warn!(
            listen = %args.listen,
            "SECURITY WARNING: coral-daemon TCP gRPC is bound to a non-loopback address without --auth-token / CORAL_AUTH_TOKEN"
        );
    }

    if !args.no_docker_socket {
        let docker_sock = args
            .docker_socket
            .unwrap_or_else(|| state_dir.join("docker.sock"));
        if let Some(parent) = docker_sock.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if docker_sock.exists() {
            let _ = fs::remove_file(&docker_sock);
        }
        match UnixListener::bind(&docker_sock) {
            Ok(listener) => {
                let _ = fs::set_permissions(&docker_sock, fs::Permissions::from_mode(sock_mode));
                let docker_svc = service.clone();
                info!(
                    docker_socket = %docker_sock.display(),
                    "starting Docker Engine REST API v1.45 compatibility socket"
                );
                tokio::spawn(async move {
                    let _ = coral_daemon::service::docker_compat::serve_docker_unix(
                        docker_svc,
                        listener,
                        shutdown_rx_docker,
                    )
                    .await;
                });
            }
            Err(e) => {
                tracing::warn!(
                    path = %docker_sock.display(),
                    err = %e,
                    "could not bind Docker compat socket (continuing without docker.sock)"
                );
            }
        }
    }

    if !args.no_metrics {
        let metrics_addr = args.metrics_listen;
        match tokio::net::TcpListener::bind(metrics_addr).await {
            Ok(listener) => {
                let metrics_svc = service.clone();
                info!(
                    metrics = %format!("http://{metrics_addr}/metrics"),
                    "starting Prometheus /metrics & /healthz endpoint"
                );
                tokio::spawn(async move {
                    if let Err(e) = coral_daemon::service::prometheus::serve_prometheus_metrics(
                        metrics_svc,
                        listener,
                        shutdown_rx_metrics,
                    )
                    .await
                    {
                        tracing::warn!(err = %e, "Prometheus /metrics server exited");
                    }
                });
            }
            Err(e) => {
                tracing::warn!(addr = %metrics_addr, err = %e, "could not bind Prometheus /metrics listener (continuing without /metrics)");
            }
        }
    }

    #[derive(Clone)]
    struct BearerAuth(Option<std::sync::Arc<String>>);

    impl tonic::service::Interceptor for BearerAuth {
        #[allow(clippy::result_large_err)]
        fn call(&mut self, req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
            let Some(secret) = self.0.as_deref() else {
                return Ok(req);
            };
            let provided = req
                .metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
                .unwrap_or("");
            if coral_daemon::service::nextgen::constant_time_eq(
                provided.as_bytes(),
                secret.as_bytes(),
            ) {
                Ok(req)
            } else {
                Err(tonic::Status::unauthenticated(
                    "invalid or missing Bearer token (set CORAL_AUTH_TOKEN)",
                ))
            }
        }
    }

    let tcp_svc = service.clone();
    let tcp_addr = args.listen;
    let tcp_auth = BearerAuth(auth_token.clone());
    let tcp_fut = async move {
        Server::builder()
            .add_service(RuntimeServiceServer::with_interceptor(
                tcp_svc.clone(),
                tcp_auth.clone(),
            ))
            .add_service(ImageServiceServer::with_interceptor(tcp_svc, tcp_auth))
            .serve_with_shutdown(tcp_addr, async move {
                let _ = shutdown_rx_tcp.changed().await;
            })
            .await
            .with_context(|| format!("TCP CRI server on {tcp_addr}"))
    };
    if let Some(sock_path) = unix_sock_path {
        if let Some(parent) = sock_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create socket parent dir '{}'", parent.display()))?;
        }
        if sock_path.exists() {
            let _ = fs::remove_file(&sock_path);
        }
        let uds = UnixListener::bind(&sock_path)
            .with_context(|| format!("bind CRI Unix socket '{}'", sock_path.display()))?;
        let _ = fs::set_permissions(&sock_path, fs::Permissions::from_mode(sock_mode));
        let uds_stream = UnixListenerStream::new(uds);

        info!(
            listen = %args.listen,
            unix_socket = %sock_path.display(),
            state_dir = %state_dir.display(),
            images_dir = %images_dir.display(),
            "starting coral-daemon CRI v1 gRPC server (TCP + Unix socket)"
        );

        let uds_svc = service.clone();
        let uds_fut = async move {
            Server::builder()
                .add_service(RuntimeServiceServer::new(uds_svc.clone()))
                .add_service(ImageServiceServer::new(uds_svc))
                .serve_with_incoming_shutdown(uds_stream, async move {
                    let _ = shutdown_rx_uds.changed().await;
                })
                .await
                .context("Unix socket CRI server")
        };

        // Both listeners are bound at this point, so kubelet/CLI readiness probes succeed
        // immediately. Boot Auto-Resume (port forwarders + container re-exec) and the Auto-GC
        // timer both run behind them instead of delaying `CoralDaemonService::new` and the
        // first CRI calls by their full duration.
        service.start_boot_auto_resume();
        service.start_auto_gc();

        let res = tokio::try_join!(tcp_fut, uds_fut);
        let _ = fs::remove_file(&sock_path);
        res?;
    } else {
        info!(
            listen = %args.listen,
            state_dir = %state_dir.display(),
            images_dir = %images_dir.display(),
            "starting coral-daemon CRI v1 gRPC server (TCP only)"
        );
        // TCP-only mode: the listener is owned by `serve_with_shutdown`, so resume starts here.
        service.start_boot_auto_resume();
        service.start_auto_gc();
        tcp_fut.await?;
    }

    Ok(())
}
