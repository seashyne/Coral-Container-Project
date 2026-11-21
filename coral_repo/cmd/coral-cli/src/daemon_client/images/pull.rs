//! `coral pull` — pull an OCI image or register a local rootfs.

use super::ProgressMode;
use crate::catalog::format_bytes;
use crate::daemon_client::try_connect_channel;
use crate::progress_bar::{ProgressFollower, next_pull_id};
use anyhow::{Context, Result, bail};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::{ImageSpec, PullImageRequest};
use coral_daemon::images;
use std::collections::HashMap;
use std::path::Path;

pub async fn cmd_pull(
    addr: &str,
    images_dir: &Path,
    image: &str,
    rootfs: Option<&Path>,
    local_only: bool,
    state_root: &Path,
    progress_mode: ProgressMode,
) -> Result<()> {
    let image_name = image.trim();
    if image_name.is_empty() {
        bail!("image name must not be empty");
    }

    if !local_only && let Some(ch) = try_connect_channel(addr).await {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        if let Some(p) = rootfs {
            annotations.insert("coral.rootfs".into(), p.to_string_lossy().into_owned());
        }
        // A local directory is a rootfs registration, not a registry pull: no bytes to track.
        let track_progress = progress_mode != ProgressMode::None
            && rootfs.is_none()
            && !Path::new(image_name).is_dir();
        let pull_id = track_progress.then(next_pull_id);
        if let Some(id) = &pull_id {
            annotations.insert("coral.progress_id".into(), id.clone());
        }
        eprintln!("Pulling '{image_name}' via coral-daemon ({addr})...");

        // Wrap the RPC in an async block so `rpc` is a `Future<Output = anyhow::Result<_>>`:
        // `anyhow::Context` is implemented for `Result`, not for futures, so the context must
        // be attached *after* the await rather than chained onto the client call.
        let rpc = async move {
            client
                .pull_image(PullImageRequest {
                    image: Some(ImageSpec {
                        image: image_name.to_string(),
                        annotations,
                    }),
                })
                .await
                .with_context(|| format!("daemon pull_image '{image_name}'"))
        };

        // With progress tracking, the bar and the RPC are driven concurrently; without it the
        // RPC is simply awaited. `rpc` is consumed by exactly one of the two code paths below.
        let resp = if let Some(id) = pull_id {
            let mut follower =
                ProgressFollower::new(state_root, &id, progress_mode == ProgressMode::Json);
            // Pin once and reborrow via `as_mut()` in both branches, so the same future is
            // polled by whichever branch wins and is never moved twice.
            let mut rpc = std::pin::pin!(rpc);
            let mut ticker = tokio::time::interval(std::time::Duration::from_millis(50));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let outcome = tokio::select! {
                res = rpc.as_mut() => {
                    // Drain a final snapshot the daemon may have just written.
                    let _ = follower.tick();
                    res
                }
                _ = async {
                    loop {
                        ticker.tick().await;
                        if !follower.tick() {
                            break;
                        }
                    }
                } => rpc.as_mut().await,
            };
            match outcome {
                Ok(resp) => {
                    follower.finish(image_name, true);
                    resp.into_inner()
                }
                Err(err) => {
                    let reason = follower.failure_reason();
                    follower.finish(image_name, false);
                    return Err(match reason {
                        Some(msg) => err.context(msg),
                        None => err,
                    });
                }
            }
        } else {
            rpc.await?.into_inner()
        };
        println!("Pulled {image_name} ({})", resp.image_ref);
        return Ok(());
    }

    let stored = if let Some(p) = rootfs {
        eprintln!(
            "Registering local rootfs '{}' as '{image_name}'...",
            p.display()
        );
        images::register_local_rootfs(image_name, p)?
    } else if Path::new(image_name).is_dir() {
        eprintln!("Registering local directory '{image_name}'...");
        images::register_local_rootfs(image_name, Path::new(image_name))?
    } else {
        eprintln!(
            "Pulling '{image_name}' directly into {}...",
            images_dir.display()
        );
        images::pull_image(image_name, images_dir, None).await?
    };

    let mut index = images::load_index(images_dir);
    let id = stored.id.clone();
    let size = stored.size;
    let path = stored.rootfs_path.clone();
    let orphaned = images::upsert_image(&mut index, stored);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    images::save_index(images_dir, &index)?;

    println!(
        "Ready: {image_name}\n  ID:     {id}\n  Size:   {}\n  Rootfs: {path}",
        format_bytes(size)
    );
    Ok(())
}
