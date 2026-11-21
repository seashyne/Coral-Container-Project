//! `coral catalog`, `coral images`, `coral rmi` and `coral commit`.

use crate::catalog::{format_bytes, print_catalog};
use crate::daemon_client::{short_id, try_connect_channel};
use anyhow::{Context, Result, bail};
use coral_api::image_service_client::ImageServiceClient;
use coral_api::{Image, ImageSpec, ListImagesRequest, PullImageRequest, RemoveImageRequest};
use coral_daemon::images;
use std::collections::HashMap;
use std::path::Path;

pub async fn cmd_catalog(addr: &str, images_dir: &Path, local_only: bool) -> Result<()> {
    if !local_only && let Some(ch) = try_connect_channel(addr).await {
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

pub async fn cmd_images(addr: &str, images_dir: &Path, local_only: bool) -> Result<()> {
    let (mut list, source) = if !local_only && let Some(ch) = try_connect_channel(addr).await {
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
    if !local_only && let Some(ch) = try_connect_channel(addr).await {
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
