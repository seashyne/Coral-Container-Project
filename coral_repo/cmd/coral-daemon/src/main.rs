use anyhow::Result;
use clap::Parser;
use coral_api::image_service_server::ImageServiceServer;
use coral_api::runtime_service_server::RuntimeServiceServer;
use coral_daemon::images::default_images_dir;
use coral_daemon::{CoralDaemonConfig, CoralDaemonService};
use coral_runtime::default_state_root;
use std::net::SocketAddr;
use std::path::PathBuf;
use tonic::transport::Server;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "coral-daemon",
    version,
    about = "Coral Container Runtime CRI v1 gRPC Daemon"
)]
struct Args {
    /// Address to listen on for CRI v1 gRPC connections
    #[arg(long, default_value = "127.0.0.1:50051")]
    listen: SocketAddr,

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

    let service = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: state_dir.clone(),
        images_dir: Some(images_dir.clone()),
        coral_bin: args.coral_bin,
        default_rootfs: args.default_rootfs,
    });

    info!(
        listen = %args.listen,
        state_dir = %state_dir.display(),
        images_dir = %images_dir.display(),
        "starting coral-daemon CRI v1 gRPC server"
    );

    Server::builder()
        .add_service(RuntimeServiceServer::new(service.clone()))
        .add_service(ImageServiceServer::new(service))
        .serve_with_shutdown(args.listen, async {
            let _ = tokio::signal::ctrl_c().await;
            info!("received shutdown signal, stopping coral-daemon");
        })
        .await?;

    Ok(())
}
