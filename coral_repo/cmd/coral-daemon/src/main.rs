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
    });

    let (shutdown_tx, mut shutdown_rx_tcp) = watch::channel(false);
    let mut shutdown_rx_uds = shutdown_tx.subscribe();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        info!("received shutdown signal, stopping coral-daemon");
        let _ = shutdown_tx.send(true);
    });

    let tcp_svc = service.clone();
    let tcp_addr = args.listen;
    let tcp_fut = async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(tcp_svc.clone()))
            .add_service(ImageServiceServer::new(tcp_svc))
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
        let _ = fs::set_permissions(&sock_path, fs::Permissions::from_mode(0o660));
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
        tcp_fut.await?;
    }

    Ok(())
}
