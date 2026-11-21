//! `coral tag`, `coral save` and `coral load` — image aliasing and archive import/export.

use crate::catalog::format_bytes;
use crate::daemon_client::try_connect_channel;
use anyhow::{Context, Result};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::{ImageSpec, PullImageRequest};
use coral_daemon::images;
use std::collections::HashMap;
use std::path::Path;

pub async fn cmd_tag(
    addr: &str,
    images_dir: &Path,
    source: &str,
    target: &str,
    local_only: bool,
) -> Result<()> {
    if !local_only && let Some(ch) = try_connect_channel(addr).await {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        annotations.insert("coral.tag_source".into(), source.to_string());
        let resp = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: target.to_string(),
                    annotations,
                }),
            })
            .await
            .with_context(|| format!("daemon tag '{source}' -> '{target}'"))?
            .into_inner();
        println!("Tagged {source} -> {target} ({})", resp.image_ref);
        return Ok(());
    }

    let mut index = images::load_index(images_dir);
    let tagged = images::tag_image(&mut index, source, target)?;
    images::save_index(images_dir, &index)?;
    println!("Tagged {source} -> {target} ({})", tagged.id);
    Ok(())
}

pub async fn cmd_save(
    addr: &str,
    images_dir: &Path,
    image: &str,
    output: &Path,
    local_only: bool,
) -> Result<()> {
    let abs_output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        std::env::current_dir()?.join(output)
    };

    if !local_only && let Some(ch) = try_connect_channel(addr).await {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        annotations.insert(
            "coral.save_archive".into(),
            abs_output.to_string_lossy().into_owned(),
        );
        let resp = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: image.to_string(),
                    annotations,
                }),
            })
            .await
            .with_context(|| format!("daemon save image '{image}'"))?
            .into_inner();
        let bytes: u64 = resp
            .image_ref
            .strip_prefix("saved:")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        println!(
            "Saved {image} -> {} ({})",
            abs_output.display(),
            format_bytes(bytes)
        );
        return Ok(());
    }

    let index = images::load_index(images_dir);
    let bytes = images::save_image_archive(&index, image, &abs_output)?;
    println!(
        "Saved {image} -> {} ({})",
        abs_output.display(),
        format_bytes(bytes)
    );
    Ok(())
}

pub async fn cmd_load(
    addr: &str,
    images_dir: &Path,
    input: &Path,
    tag_override: Option<&str>,
    local_only: bool,
) -> Result<()> {
    let abs_input = std::fs::canonicalize(input)
        .with_context(|| format!("resolve archive path '{}'", input.display()))?;

    if !local_only && let Some(ch) = try_connect_channel(addr).await {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        annotations.insert(
            "coral.load_archive".into(),
            abs_input.to_string_lossy().into_owned(),
        );
        let spec_name = tag_override
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("__archive__");
        let resp = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: spec_name.to_string(),
                    annotations,
                }),
            })
            .await
            .with_context(|| format!("daemon load archive '{}'", abs_input.display()))?
            .into_inner();
        println!("Loaded {} (from {})", resp.image_ref, abs_input.display());
        return Ok(());
    }

    let stored = images::load_image_archive(images_dir, &abs_input, tag_override)?;
    let tags = stored.repo_tags.join(", ");
    let id = stored.id.clone();
    let size = stored.size;
    let path = stored.rootfs_path.clone();

    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, stored);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    images::save_index(images_dir, &index)?;
    println!(
        "Loaded: {tags}\n  ID:     {id}\n  Size:   {}\n  Rootfs: {path}",
        format_bytes(size)
    );
    Ok(())
}
