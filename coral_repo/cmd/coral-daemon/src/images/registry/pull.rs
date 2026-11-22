//! Image pull entry point: manifest resolution, layer streaming and rootfs install.

use super::auth::{PullAuth, authorized_get};
use super::layer_cache::{LayerFetchContext, download_layers_parallel};
use super::layer_store::{
    extract_or_reuse_layer, layers_safe_for_direct_overlay, materialize_hardlink_rootfs,
    save_image_layers_manifest,
};
use super::manifest::{
    parse_config_digest, parse_layers, select_linux_platform_digest_for,
};
use super::util::{ACCEPT_MANIFESTS, safe_slug};
use crate::images::auth::{load_registry_auth, registry_scheme};
use crate::images::config::OciImageConfig;
use crate::images::extract::extract_layer;
use crate::images::progress::{PullPhase, PullProgressReporter};
use crate::images::reference::ImageRef;
use crate::images::{StoredImage, dir_size, ensure_scratch_image, remove_dir_force};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::info;

pub async fn pull_image(
    image_ref_str: &str,
    images_dir: &Path,
    reporter: Option<Arc<PullProgressReporter>>,
) -> Result<StoredImage> {
    pull_image_for_platform(image_ref_str, images_dir, reporter, None).await
}

/// Pulls an OCI image for an explicit target platform (e.g. `"linux/arm64"` or `"linux/amd64"`),
/// defaulting to the host architecture when `platform_override` is `None`.
pub async fn pull_image_for_platform(
    image_ref_str: &str,
    images_dir: &Path,
    reporter: Option<Arc<PullProgressReporter>>,
    platform_override: Option<&str>,
) -> Result<StoredImage> {
    match pull_image_inner(image_ref_str, images_dir, reporter.clone(), platform_override).await {
        Ok(stored) => {
            if let Some(rep) = reporter {
                rep.set_phase(PullPhase::Done);
                rep.finish();
            }
            Ok(stored)
        }
        Err(err) => {
            if let Some(rep) = reporter {
                rep.fail(format!("{err:#}"));
            }
            Err(err)
        }
    }
}

async fn pull_image_inner(
    image_ref_str: &str,
    images_dir: &Path,
    reporter: Option<Arc<PullProgressReporter>>,
    platform_override: Option<&str>,
) -> Result<StoredImage> {
    if let Some(rep) = &reporter {
        rep.set_phase(PullPhase::Resolving);
        rep.flush();
    }
    let parsed = ImageRef::parse(image_ref_str)?;
    let (registry, repository, reference, display_tag) = match parsed {
        ImageRef::Scratch => return ensure_scratch_image(images_dir),
        ImageRef::Oci {
            registry,
            repository,
            reference,
            display_tag,
        } => (registry, repository, reference, display_tag),
    };

    info!(
        image = %display_tag,
        registry = %registry,
        repository = %repository,
        reference = %reference,
        "pulling OCI image"
    );

    let client = reqwest::Client::builder()
        .user_agent("coral-daemon/1.0")
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build HTTP client")?;

    let scheme = registry_scheme(&registry);
    let creds = load_registry_auth(&registry);
    let mut auth_header: Option<PullAuth> = None;

    // 1. Fetch manifest or multi-arch index.
    let manifest_url = format!("{scheme}://{registry}/v2/{repository}/manifests/{reference}");
    let resp = authorized_get(
        &client,
        &manifest_url,
        Some(ACCEPT_MANIFESTS),
        &mut auth_header,
        creds.as_ref(),
    )
    .await
    .with_context(|| format!("fetch manifest for '{display_tag}'"))?;

    let mut manifest_digest = resp
        .headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let manifest_bytes = resp.bytes().await.context("read manifest body")?;
    let manifest_json: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).context("parse manifest JSON")?;

    let (layers, resolved_digest, config_digest) =
        if let Some(manifests) = manifest_json.get("manifests").and_then(|v| v.as_array()) {
            let chosen_digest = select_linux_platform_digest_for(manifests, platform_override)?;
            let child_url =
                format!("{scheme}://{registry}/v2/{repository}/manifests/{chosen_digest}");
            let child_resp = authorized_get(
                &client,
                &child_url,
                Some(ACCEPT_MANIFESTS),
                &mut auth_header,
                creds.as_ref(),
            )
            .await
            .with_context(|| format!("fetch platform manifest '{chosen_digest}'"))?;
            let child_digest = child_resp
                .headers()
                .get("docker-content-digest")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .unwrap_or_else(|| chosen_digest.clone());
            let child_bytes = child_resp
                .bytes()
                .await
                .context("read platform manifest body")?;
            let child_json: serde_json::Value =
                serde_json::from_slice(&child_bytes).context("parse platform manifest JSON")?;
            (
                parse_layers(&child_json)?,
                child_digest,
                parse_config_digest(&child_json),
            )
        } else {
            let digest = manifest_digest.take().unwrap_or_else(|| {
                let mut hasher = Sha256::new();
                hasher.update(&manifest_bytes);
                format!("sha256:{:x}", hasher.finalize())
            });
            (
                parse_layers(&manifest_json)?,
                digest,
                parse_config_digest(&manifest_json),
            )
        };

    // 1b. Best-effort fetch of OCI Image Config blob (Entrypoint, Cmd, Env, WorkingDir, User, StopSignal).
    let mut fetched_config: Option<OciImageConfig> = None;
    if let Some(cfg_digest) = config_digest {
        let cfg_url = format!("{scheme}://{registry}/v2/{repository}/blobs/{cfg_digest}");
        if let Ok(cfg_resp) =
            authorized_get(&client, &cfg_url, None, &mut auth_header, creds.as_ref()).await
            && let Ok(cfg_bytes) = cfg_resp.bytes().await
            && let Ok(parsed_cfg) = OciImageConfig::from_oci_blob(&cfg_bytes)
        {
            fetched_config = Some(parsed_cfg);
        }
    }

    // 2. Stage extraction in a temporary directory inside `images_dir/.tmp`.
    let slug = safe_slug(&display_tag);
    let ts = chrono::Utc::now().timestamp_millis();
    let staging_dir = images_dir.join(".tmp").join(format!("{slug}-{ts}"));
    let staging_rootfs = staging_dir.join("rootfs");
    fs::create_dir_all(&staging_rootfs)
        .with_context(|| format!("create staging dir '{}'", staging_rootfs.display()))?;

    let mut shared_layers_base_to_top: Vec<PathBuf> = Vec::new();
    let mut all_layers_shared = true;
    let pull_result: Result<()> = async {
        let downloaded = download_layers_parallel(
            &LayerFetchContext {
                client: &client,
                scheme,
                registry: &registry,
                repository: &repository,
                display_tag: &display_tag,
                images_dir,
                staging_dir: &staging_dir,
                auth_header: auth_header.clone(),
                creds: creds.clone(),
                reporter: reporter.clone(),
            },
            &layers,
        )
        .await?;

        for item in downloaded {
            if let Some(rep) = &reporter {
                rep.set_phase(PullPhase::Extracting);
                rep.flush();
            }
            let images_dir_clone = images_dir.to_path_buf();
            let blob_clone = item.blob_path.clone();
            let digest_clone = item.digest.clone();
            let media_type = item.media_type.clone();
            let shared_dir = tokio::task::spawn_blocking(move || {
                extract_or_reuse_layer(&images_dir_clone, &blob_clone, &digest_clone, &media_type)
            })
            .await
            .context("join layer extraction task")??;

            if let Some(layer_dir) = shared_dir {
                shared_layers_base_to_top.push(layer_dir);
            } else {
                all_layers_shared = false;
                let rootfs_clone = staging_rootfs.clone();
                let blob_clone = item.blob_path.clone();
                let media_type = item.media_type;
                tokio::task::spawn_blocking(move || {
                    extract_layer(&blob_clone, &rootfs_clone, &media_type)
                })
                .await
                .context("join direct layer extraction task")??;
            }

            if !item.cached_in_store {
                let _ = fs::remove_file(&item.blob_path);
            }
            if let Some(rep) = &reporter {
                rep.finish_layer();
                rep.flush();
            }
        }

        if all_layers_shared && !shared_layers_base_to_top.is_empty() {
            let layers_to_link = shared_layers_base_to_top.clone();
            let rootfs_clone = staging_rootfs.clone();
            tokio::task::spawn_blocking(move || {
                materialize_hardlink_rootfs(&layers_to_link, &rootfs_clone)
            })
            .await
            .context("join hardlink rootfs materialization")??;
        }
        Ok(())
    }
    .await;

    if let Err(err) = pull_result {
        let _ = remove_dir_force(&staging_dir);
        return Err(err);
    }

    // 3. Atomically swap staging rootfs into `<images_dir>/<slug>/rootfs`.
    if let Some(rep) = &reporter {
        rep.set_phase(PullPhase::Finalizing);
        rep.flush();
    }
    let final_parent = images_dir.join(&slug);
    let final_rootfs = final_parent.join("rootfs");
    fs::create_dir_all(&final_parent)
        .with_context(|| format!("create image dir '{}'", final_parent.display()))?;

    let backup_dir = images_dir.join(".tmp").join(format!("{slug}-old-{ts}"));
    let had_existing = final_rootfs.exists();
    if had_existing {
        fs::rename(&final_rootfs, &backup_dir)
            .with_context(|| format!("move old rootfs '{}'", final_rootfs.display()))?;
    }

    if let Err(e) = fs::rename(&staging_rootfs, &final_rootfs) {
        if had_existing {
            let _ = fs::rename(&backup_dir, &final_rootfs);
        }
        let _ = remove_dir_force(&staging_dir);
        return Err(e).with_context(|| format!("install rootfs '{}'", final_rootfs.display()));
    }

    if had_existing {
        let _ = remove_dir_force(&backup_dir);
    }
    let _ = remove_dir_force(&staging_dir);

    let abs_rootfs = fs::canonicalize(&final_rootfs)?;
    if let Some(cfg) = &fetched_config {
        let _ = cfg.save_for_rootfs(&abs_rootfs);
    }
    if all_layers_shared
        && !shared_layers_base_to_top.is_empty()
        && layers_safe_for_direct_overlay(&shared_layers_base_to_top)
    {
        let top_to_base: Vec<PathBuf> = shared_layers_base_to_top
            .into_iter()
            .rev()
            .filter_map(|p| fs::canonicalize(p).ok())
            .collect();
        let _ = save_image_layers_manifest(&abs_rootfs, &top_to_base);
    }
    let size = dir_size(&abs_rootfs);
    let repo_tags = vec![display_tag.clone()];

    info!(
        image = %display_tag,
        digest = %resolved_digest,
        size_bytes = size,
        rootfs = %abs_rootfs.display(),
        "OCI image ready"
    );

    Ok(StoredImage {
        id: resolved_digest,
        repo_tags,
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    })
}
