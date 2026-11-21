//! Image pull entry point: manifest resolution, layer streaming and rootfs install.

use super::auth::{PullAuth, authorized_get};
use super::manifest::{
    parse_config_digest, parse_layers, select_linux_platform_digest,
};
use super::util::{ACCEPT_MANIFESTS, PROGRESS_FLUSH_INTERVAL, safe_slug};
use crate::images::auth::{load_registry_auth, registry_scheme};
use crate::images::config::OciImageConfig;
use crate::images::extract::extract_layer;
use crate::images::progress::{PullPhase, PullProgressReporter};
use crate::images::reference::ImageRef;
use crate::images::{StoredImage, dir_size, ensure_scratch_image, remove_dir_force};
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

pub async fn pull_image(
    image_ref_str: &str,
    images_dir: &Path,
    reporter: Option<Arc<PullProgressReporter>>,
) -> Result<StoredImage> {
    match pull_image_inner(image_ref_str, images_dir, reporter.clone()).await {
        Ok(stored) => {
            if let Some(rep) = reporter {
                rep.set_phase(PullPhase::Done);
                rep.finish();
            }
            Ok(stored)
        }
        Err(err) => {
            if let Some(rep) = reporter {
                // Leave the `failed` snapshot on disk: the CLI reads it to print the reason and
                // removes the file itself. Auto-GC sweeps anything a crashed CLI leaves behind.
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
            let chosen_digest = select_linux_platform_digest(manifests)?;
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

    let pull_result: Result<()> = async {
        let total_layers = layers.len();
        let declared_total: u64 = layers.iter().map(|l| l.size).sum();
        if let Some(rep) = &reporter {
            rep.set_layers_total(total_layers);
            rep.set_bytes_total(declared_total);
            rep.flush();
        }

        for (idx, layer) in layers.iter().enumerate() {
            debug!(
                image = %display_tag,
                layer = idx + 1,
                total = total_layers,
                digest = %layer.digest,
                "downloading layer"
            );
            let blob_url = format!("{scheme}://{registry}/v2/{repository}/blobs/{}", layer.digest);
            let blob_resp =
                authorized_get(&client, &blob_url, None, &mut auth_header, creds.as_ref())
                    .await
                    .with_context(|| format!("download layer {}", layer.digest))?;

            // Registries report the compressed blob size; prefer it over the manifest hint and
            // widen `bytes_total` when the manifest under-declared (multi-arch child manifests).
            let content_length = blob_resp.content_length().unwrap_or(0);
            let layer_total = if content_length > 0 {
                content_length
            } else {
                layer.size
            };
            if let Some(rep) = &reporter {
                if layer_total > 0 {
                    let known = layer.size;
                    if content_length > 0 && content_length > known {
                        rep.set_bytes_total(
                            declared_total.saturating_sub(known).saturating_add(content_length),
                        );
                    }
                }
                rep.begin_layer(idx + 1, layer_total);
                rep.flush();
            }

            let blob_path = staging_dir.join(format!("layer-{idx}.blob"));
            let mut out = File::create(&blob_path)
                .with_context(|| format!("create layer temp file '{}'", blob_path.display()))?;
            let mut hasher = Sha256::new();
            let mut stream = blob_resp.bytes_stream();
            let mut last_flush = std::time::Instant::now();
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.context("stream layer chunk")?;
                hasher.update(&bytes);
                out.write_all(&bytes).context("write layer chunk")?;
                if let Some(rep) = &reporter {
                    rep.add_bytes(bytes.len() as u64);
                    // Time-throttled so a fast local registry does not fsync per 64 KiB chunk.
                    if last_flush.elapsed() >= PROGRESS_FLUSH_INTERVAL {
                        rep.flush();
                        last_flush = std::time::Instant::now();
                    }
                }
            }
            out.flush()?;
            drop(out);

            if let Some(expected_hex) = layer.digest.strip_prefix("sha256:") {
                let actual_hex = format!("{:x}", hasher.finalize());
                if actual_hex != expected_hex {
                    bail!(
                        "layer digest mismatch for {}: expected sha256:{expected_hex}, got sha256:{actual_hex}",
                        display_tag
                    );
                }
            }

            if let Some(rep) = &reporter {
                rep.set_phase(PullPhase::Extracting);
                rep.flush();
            }
            let rootfs_clone = staging_rootfs.clone();
            let blob_clone = blob_path.clone();
            let media_type = layer.media_type.clone();
            tokio::task::spawn_blocking(move || {
                extract_layer(&blob_clone, &rootfs_clone, &media_type)
            })
            .await
            .context("join layer extraction task")??;

            let _ = fs::remove_file(&blob_path);
            if let Some(rep) = &reporter {
                rep.finish_layer();
                rep.flush();
            }
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
