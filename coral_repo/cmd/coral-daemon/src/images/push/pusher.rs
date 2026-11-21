//! Packages a stored image and pushes it to an OCI registry.

use super::auth::send_with_auth;
use super::blob::upload_blob_if_missing;
use super::layer::{host_oci_arch, pack_rootfs_layer_gzip, sha256_digest};
use super::model::{PushResult, RegistryAuthHeader};
use super::{MEDIA_OCI_CONFIG, MEDIA_OCI_LAYER_GZIP, MEDIA_OCI_MANIFEST};
use crate::images::auth::{load_registry_auth, registry_scheme};
use crate::images::reference::ImageRef;
use crate::images::StoredImage;
use anyhow::{Context, Result, bail};
use reqwest::header::CONTENT_TYPE;
use std::path::PathBuf;
use tracing::info;

/// Packages `stored.rootfs_path` into an OCI gzipped layer + config JSON + OCI manifest
/// and pushes it to the registry specified by `image_ref_str`.
pub async fn push_image(image_ref_str: &str, stored: &StoredImage) -> Result<PushResult> {
    let parsed = ImageRef::parse(image_ref_str)?;
    let (registry, repository, reference, display_tag) = match parsed {
        ImageRef::Scratch => bail!("cannot push the special 'scratch' base image"),
        ImageRef::Oci {
            registry,
            repository,
            reference,
            display_tag,
        } => (registry, repository, reference, display_tag),
    };

    let rootfs = PathBuf::from(&stored.rootfs_path);
    if !rootfs.is_dir() {
        bail!("image rootfs '{}' does not exist", rootfs.display());
    }

    info!(
        image = %display_tag,
        registry = %registry,
        repository = %repository,
        reference = %reference,
        "packaging and pushing OCI image"
    );

    // 1. Pack rootfs into gzipped tar blob + compute uncompressed diff_id & compressed digest.
    let (layer_bytes, diff_id, layer_digest) =
        tokio::task::spawn_blocking(move || pack_rootfs_layer_gzip(&rootfs))
            .await
            .context("join rootfs packaging task")??;
    let layer_size = layer_bytes.len() as u64;

    // 2. Build OCI v1 image config JSON.
    let config_json = serde_json::json!({
        "created": chrono::Utc::now().to_rfc3339(),
        "architecture": host_oci_arch(),
        "os": "linux",
        "config": {
            "WorkingDir": "/",
            "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
            "Cmd": ["/bin/sh"],
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": [diff_id],
        }
    });
    let config_bytes = serde_json::to_vec(&config_json)?;
    let config_digest = sha256_digest(&config_bytes);
    let config_size = config_bytes.len() as u64;

    // 3. Build OCI v1 image manifest JSON.
    let manifest_json = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MEDIA_OCI_MANIFEST,
        "config": {
            "mediaType": MEDIA_OCI_CONFIG,
            "digest": config_digest,
            "size": config_size,
        },
        "layers": [{
            "mediaType": MEDIA_OCI_LAYER_GZIP,
            "digest": layer_digest,
            "size": layer_size,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest_json)?;
    let local_manifest_digest = sha256_digest(&manifest_bytes);

    let scheme = registry_scheme(&registry);
    let base_url = format!("{scheme}://{registry}");
    let creds = load_registry_auth(&registry);
    let client = reqwest::Client::builder()
        .user_agent("coral-daemon/1.3")
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build HTTP client for push")?;

    let mut auth = RegistryAuthHeader::None;

    // 4. Upload config blob and layer blob.
    upload_blob_if_missing(
        &client,
        &base_url,
        &repository,
        &config_digest,
        config_bytes,
        &mut auth,
        creds.as_ref(),
    )
    .await
    .context("upload OCI image config blob")?;

    upload_blob_if_missing(
        &client,
        &base_url,
        &repository,
        &layer_digest,
        layer_bytes,
        &mut auth,
        creds.as_ref(),
    )
    .await
    .context("upload OCI image layer blob")?;

    // 5. PUT OCI image manifest.
    let manifest_url = format!("{base_url}/v2/{repository}/manifests/{reference}");
    let resp = send_with_auth(
        &client,
        &mut auth,
        creds.as_ref(),
        |c, a| {
            a.apply(
                c.put(&manifest_url)
                    .header(CONTENT_TYPE, MEDIA_OCI_MANIFEST)
                    .body(manifest_bytes.clone()),
            )
        },
    )
    .await
    .with_context(|| format!("push manifest to {manifest_url}"))?;

    let remote_digest = resp
        .headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or(local_manifest_digest);

    Ok(PushResult {
        image: display_tag,
        registry,
        repository,
        reference,
        manifest_digest: remote_digest,
        layer_digest,
        layer_size,
    })
}
