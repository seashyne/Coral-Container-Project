//! Tauri CRI image commands (`list_images`, `pull_image`, `remove_image`,
//! `commit_container`, `build_image`).

use super::{AppState, err};
use coral_api::{ImageSpec, ListImagesRequest, PullImageRequest, RemoveImageRequest};
use serde::Serialize;
use std::collections::HashMap;
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRow {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: u64,
    pub rootfs_path: String,
}

#[tauri::command]
pub async fn list_images(state: State<'_, AppState>) -> Result<Vec<ImageRow>, String> {
    let resp = state
        .image()
        .await?
        .list_images(ListImagesRequest {})
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp
        .images
        .into_iter()
        .map(|i| ImageRow {
            id: i.id,
            repo_tags: i.repo_tags,
            size: i.size,
            rootfs_path: i.rootfs_path,
        })
        .collect())
}

#[tauri::command]
pub async fn pull_image(
    state: State<'_, AppState>,
    name: String,
    rootfs: Option<String>,
) -> Result<String, String> {
    let mut annotations = HashMap::new();
    if let Some(path) = rootfs
        && !path.trim().is_empty()
    {
        annotations.insert("coral.rootfs".into(), path.trim().into());
    }
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: name,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}

#[tauri::command]
pub async fn remove_image(state: State<'_, AppState>, name: String) -> Result<(), String> {
    state
        .image()
        .await?
        .remove_image(RemoveImageRequest {
            image: Some(ImageSpec {
                image: name,
                annotations: HashMap::new(),
            }),
        })
        .await
        .map_err(err)?;
    Ok(())
}

#[tauri::command]
pub async fn commit_container(
    state: State<'_, AppState>,
    id: String,
    image_name: String,
) -> Result<String, String> {
    let tag = image_name.trim().to_string();
    if tag.is_empty() {
        return Err("target image tag must not be empty".into());
    }
    let mut annotations = HashMap::new();
    annotations.insert("coral.commit_container".into(), id);
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: tag,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}

#[tauri::command]
pub async fn build_image(
    state: State<'_, AppState>,
    name: String,
    dockerfile: String,
    context_dir: Option<String>,
    no_cache: Option<bool>,
) -> Result<String, String> {
    let tag = name.trim().to_string();
    if tag.is_empty() {
        return Err("image tag must not be empty".into());
    }
    if dockerfile.trim().is_empty() {
        return Err("Dockerfile content must not be empty".into());
    }
    let mut annotations = HashMap::new();
    annotations.insert("coral.dockerfile".into(), dockerfile);
    if let Some(ctx) = context_dir
        && !ctx.trim().is_empty()
    {
        annotations.insert("coral.context".into(), ctx.trim().into());
    }
    if no_cache.unwrap_or(false) {
        annotations.insert("coral.no_cache".into(), "true".into());
    }
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: tag,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}

#[tauri::command]
pub async fn tag_image(
    state: State<'_, AppState>,
    source: String,
    target: String,
) -> Result<String, String> {
    let src = source.trim().to_string();
    let dst = target.trim().to_string();
    if src.is_empty() || dst.is_empty() {
        return Err("source and target image tags must not be empty".into());
    }
    let mut annotations = HashMap::new();
    annotations.insert("coral.tag_source".into(), src);
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: dst,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}

#[tauri::command]
pub async fn save_image(
    state: State<'_, AppState>,
    image: String,
    output_path: String,
) -> Result<String, String> {
    let img = image.trim().to_string();
    let out = output_path.trim().to_string();
    if img.is_empty() || out.is_empty() {
        return Err("image name and output archive path must not be empty".into());
    }
    let mut annotations = HashMap::new();
    annotations.insert("coral.save_archive".into(), out);
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: img,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}

#[tauri::command]
pub async fn load_image(
    state: State<'_, AppState>,
    input_path: String,
    tag: Option<String>,
) -> Result<String, String> {
    let inp = input_path.trim().to_string();
    if inp.is_empty() {
        return Err("archive file path must not be empty".into());
    }
    let spec_name = tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("__archive__")
        .to_string();
    let mut annotations = HashMap::new();
    annotations.insert("coral.load_archive".into(), inp);
    let resp = state
        .image()
        .await?
        .pull_image(PullImageRequest {
            image: Some(ImageSpec {
                image: spec_name,
                annotations,
            }),
        })
        .await
        .map_err(err)?
        .into_inner();
    Ok(resp.image_ref)
}
