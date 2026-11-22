//! `coral build` — build an OCI image from a Dockerfile.

use crate::catalog::format_bytes;
use crate::daemon_client::try_connect_channel;
use anyhow::{Context, Result, bail};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::{ImageSpec, PullImageRequest};
use coral_daemon::images;
use std::collections::HashMap;
use std::path::Path;

#[allow(clippy::too_many_arguments)]
pub async fn cmd_build(
    addr: &str,
    state_root: &Path,
    images_dir: &Path,
    tag: &str,
    dockerfile_path: &Path,
    context_dir: &Path,
    local_only: bool,
    no_cache: bool,
    target: Option<&str>,
    build_args: &[String],
    platform: Option<&str>,
) -> Result<()> {
    let tag_trimmed = tag.trim();
    if tag_trimmed.is_empty() {
        bail!("image tag (-t) must not be empty");
    }
    let dockerfile_content = std::fs::read_to_string(dockerfile_path)
        .with_context(|| format!("read Dockerfile '{}'", dockerfile_path.display()))?;
    let context_abs = std::fs::canonicalize(context_dir)
        .with_context(|| format!("resolve build context '{}'", context_dir.display()))?;

    if !local_only && let Some(ch) = try_connect_channel(addr).await {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        annotations.insert("coral.dockerfile".into(), dockerfile_content);
        annotations.insert(
            "coral.context".into(),
            context_abs.to_string_lossy().into_owned(),
        );
        if no_cache {
            annotations.insert("coral.no_cache".into(), "true".into());
        }
        if let Some(t) = target.map(str::trim).filter(|s| !s.is_empty()) {
            annotations.insert("coral.build.target".into(), t.to_string());
        }
        if !build_args.is_empty() {
            annotations.insert("coral.build.args".into(), build_args.join("\n"));
        }
        if let Some(plat) = platform.map(str::trim).filter(|s| !s.is_empty()) {
            annotations.insert("coral.platform".into(), plat.to_string());
        }
        eprintln!(
            "Building '{tag_trimmed}' from {} via coral-daemon ({addr})...",
            dockerfile_path.display()
        );
        let resp = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: tag_trimmed.to_string(),
                    annotations,
                }),
            })
            .await
            .with_context(|| format!("daemon build image '{tag_trimmed}'"))?
            .into_inner();
        println!("Built {tag_trimmed} ({})", resp.image_ref);
        return Ok(());
    }

    let coral_bin = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("coral"));
    eprintln!(
        "Building '{tag_trimmed}' from {} into {}...",
        dockerfile_path.display(),
        images_dir.display()
    );
    let stored = coral_daemon::builder::build_image(coral_daemon::builder::BuildOptions {
        tag: tag_trimmed,
        dockerfile_content: &dockerfile_content,
        context_dir: Some(&context_abs),
        images_dir,
        state_dir: state_root,
        coral_bin: &coral_bin,
        no_cache,
        target,
        build_args,
        platform,
    })
    .await?;

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
        "Built: {tag_trimmed}\n  ID:     {id}\n  Size:   {}\n  Rootfs: {path}",
        format_bytes(size)
    );
    Ok(())
}
