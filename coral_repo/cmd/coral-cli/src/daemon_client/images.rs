//! Image management commands (`catalog`, `pull`, `images`, `rmi`, `build`, `commit`)
//! with automatic fallback between `coral-daemon` gRPC and local image storage.

use super::{short_id, try_connect_channel};
use crate::catalog::{format_bytes, print_catalog};
use anyhow::{Context, Result, bail};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::{Image, ImageSpec, ListImagesRequest, PullImageRequest, RemoveImageRequest};
use coral_daemon::images;
use std::collections::HashMap;
use std::path::Path;

pub async fn cmd_catalog(addr: &str, images_dir: &Path, local_only: bool) -> Result<()> {
    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
        let mut client = ImageServiceClient::new(ch);
        if let Ok(resp) = client.list_images(ListImagesRequest {}).await {
            let list = resp.into_inner().images;
            print_catalog(&list, &format!("daemon @ {addr}"));
            return Ok(());
        }
    }

    let stored = images::load_index(images_dir);
    let list: Vec<Image> = stored.iter().map(Image::from).collect();
    print_catalog(&list, &format!("local store {}", images_dir.display()));
    Ok(())
}

pub async fn cmd_pull(
    addr: &str,
    images_dir: &Path,
    image: &str,
    rootfs: Option<&Path>,
    local_only: bool,
) -> Result<()> {
    let image_name = image.trim();
    if image_name.is_empty() {
        bail!("image name must not be empty");
    }

    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
        let mut client = ImageServiceClient::new(ch);
        let mut annotations = HashMap::new();
        if let Some(p) = rootfs {
            annotations.insert("coral.rootfs".into(), p.to_string_lossy().into_owned());
        }
        eprintln!("Pulling '{image_name}' via coral-daemon ({addr})...");
        let resp = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: image_name.to_string(),
                    annotations,
                }),
            })
            .await
            .with_context(|| format!("daemon pull_image '{image_name}'"))?
            .into_inner();
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
        images::pull_image(image_name, images_dir).await?
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

pub async fn cmd_images(addr: &str, images_dir: &Path, local_only: bool) -> Result<()> {
    let (mut list, source) = if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
        let mut client = ImageServiceClient::new(ch);
        let resp = client
            .list_images(ListImagesRequest {})
            .await
            .context("daemon list_images")?
            .into_inner();
        (resp.images, format!("daemon @ {addr}"))
    } else {
        let stored = images::load_index(images_dir);
        let list: Vec<Image> = stored.iter().map(Image::from).collect();
        (list, format!("local {}", images_dir.display()))
    };

    list.sort_by(|a, b| {
        let ta = a.repo_tags.first().map(String::as_str).unwrap_or(&a.id);
        let tb = b.repo_tags.first().map(String::as_str).unwrap_or(&b.id);
        ta.cmp(tb)
    });

    if list.is_empty() {
        println!("No images installed ({source}).");
        println!("Run `coral catalog` to browse Linux base images or `coral pull <image>`.");
        return Ok(());
    }

    println!(
        "{:<48} {:>10}  {:<24} ROOTFS",
        "REPOSITORY / TAGS", "SIZE", "IMAGE ID"
    );
    for img in list {
        let tags = if img.repo_tags.is_empty() {
            img.id.clone()
        } else {
            img.repo_tags.join(", ")
        };
        println!(
            "{:<48} {:>10}  {:<24} {}",
            tags,
            format_bytes(img.size),
            short_id(&img.id),
            img.rootfs_path
        );
    }
    Ok(())
}

pub async fn cmd_rmi(addr: &str, images_dir: &Path, image: &str, local_only: bool) -> Result<()> {
    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
        let mut client = ImageServiceClient::new(ch);
        client
            .remove_image(RemoveImageRequest {
                image: Some(ImageSpec {
                    image: image.to_string(),
                    annotations: HashMap::new(),
                }),
            })
            .await
            .with_context(|| format!("daemon remove_image '{image}'"))?;
        println!("Removed image '{image}' (via daemon)");
        return Ok(());
    }

    let mut index = images::load_index(images_dir);
    let (removed, managed_paths) = images::remove_matching_images(&mut index, image);
    if removed == 0 {
        bail!("image '{image}' not found in {}", images_dir.display());
    }
    for p in managed_paths {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &p);
    }
    images::save_index(images_dir, &index)?;
    println!("Removed {removed} image entry matching '{image}'");
    Ok(())
}

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
) -> Result<()> {
    let tag_trimmed = tag.trim();
    if tag_trimmed.is_empty() {
        bail!("image tag (-t) must not be empty");
    }
    let dockerfile_content = std::fs::read_to_string(dockerfile_path)
        .with_context(|| format!("read Dockerfile '{}'", dockerfile_path.display()))?;
    let context_abs = std::fs::canonicalize(context_dir)
        .with_context(|| format!("resolve build context '{}'", context_dir.display()))?;

    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
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

pub async fn cmd_tag(
    addr: &str,
    images_dir: &Path,
    source: &str,
    target: &str,
    local_only: bool,
) -> Result<()> {
    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
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

    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
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

    if !local_only
        && let Some(ch) = try_connect_channel(addr).await
    {
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

pub async fn cmd_commit_via_daemon(
    addr: &str,
    container_id: &str,
    new_image: &str,
) -> Result<Option<String>> {
    let Some(ch) = try_connect_channel(addr).await else {
        return Ok(None);
    };
    let mut client = ImageServiceClient::new(ch);
    let mut annotations = HashMap::new();
    annotations.insert("coral.commit_container".into(), container_id.to_string());
    match client
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: new_image.to_string(),
                annotations,
            }),
        })
        .await
    {
        Ok(resp) => Ok(Some(resp.into_inner().image_ref)),
        Err(st) if st.code() == tonic::Code::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("daemon commit container '{container_id}'")),
    }
}
