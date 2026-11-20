//! Online OCI Registry v2 HTTP client: Bearer token authentication, multi-arch manifest
//! index resolution (`linux/amd64` / `linux/arm64`), and SHA-256 verified layer streaming.

use super::auth::{load_registry_auth, registry_scheme};
use super::config::OciImageConfig;
use super::extract::extract_layer;
use super::push::fetch_bearer_token_with_creds;
use super::reference::{ImageRef, parse_bearer_challenge};
use super::{StoredImage, dir_size, ensure_scratch_image, remove_dir_force};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue, WWW_AUTHENTICATE};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tracing::{debug, info};

const ACCEPT_MANIFESTS: &str = concat!(
    "application/vnd.oci.image.index.v1+json, ",
    "application/vnd.docker.distribution.manifest.list.v2+json, ",
    "application/vnd.oci.image.manifest.v1+json, ",
    "application/vnd.docker.distribution.manifest.v2+json"
);

/// Pulls an OCI image (or prepares `scratch`) and extracts its Linux rootfs into `images_dir`.
pub async fn pull_image(image_ref_str: &str, images_dir: &Path) -> Result<StoredImage> {
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
        for (idx, layer) in layers.iter().enumerate() {
            debug!(
                image = %display_tag,
                layer = idx + 1,
                total = layers.len(),
                digest = %layer.digest,
                "downloading layer"
            );
            let blob_url = format!("{scheme}://{registry}/v2/{repository}/blobs/{}", layer.digest);
            let blob_resp =
                authorized_get(&client, &blob_url, None, &mut auth_header, creds.as_ref())
                    .await
                    .with_context(|| format!("download layer {}", layer.digest))?;

            let blob_path = staging_dir.join(format!("layer-{idx}.blob"));
            let mut out = File::create(&blob_path)
                .with_context(|| format!("create layer temp file '{}'", blob_path.display()))?;
            let mut hasher = Sha256::new();
            let mut stream = blob_resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.context("stream layer chunk")?;
                hasher.update(&bytes);
                out.write_all(&bytes).context("write layer chunk")?;
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

            let rootfs_clone = staging_rootfs.clone();
            let blob_clone = blob_path.clone();
            let media_type = layer.media_type.clone();
            tokio::task::spawn_blocking(move || {
                extract_layer(&blob_clone, &rootfs_clone, &media_type)
            })
            .await
            .context("join layer extraction task")??;

            let _ = fs::remove_file(&blob_path);
        }
        Ok(())
    }
    .await;

    if let Err(err) = pull_result {
        let _ = remove_dir_force(&staging_dir);
        return Err(err);
    }

    // 3. Atomically swap staging rootfs into `<images_dir>/<slug>/rootfs`.
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

#[derive(Debug, Clone)]
struct LayerDescriptor {
    digest: String,
    media_type: String,
}

fn parse_layers(manifest: &serde_json::Value) -> Result<Vec<LayerDescriptor>> {
    let arr = manifest
        .get("layers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("OCI manifest does not contain a 'layers' array"))?;
    let mut layers = Vec::with_capacity(arr.len());
    for item in arr {
        let digest = item
            .get("digest")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("layer descriptor missing 'digest'"))?
            .to_string();
        let media_type = item
            .get("mediaType")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        layers.push(LayerDescriptor { digest, media_type });
    }
    Ok(layers)
}

fn parse_config_digest(manifest: &serde_json::Value) -> Option<String> {
    manifest
        .get("config")
        .and_then(|c| c.get("digest"))
        .and_then(|d| d.as_str())
        .map(str::to_string)
}

fn host_oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        "powerpc64" => "ppc64le",
        other => other,
    }
}

fn select_linux_platform_digest(manifests: &[serde_json::Value]) -> Result<String> {
    let target_arch = host_oci_arch();
    let mut fallback: Option<String> = None;

    for m in manifests {
        if m.get("annotations")
            .and_then(|a| a.get("vnd.docker.reference.type"))
            .and_then(|v| v.as_str())
            == Some("attestation-manifest")
        {
            continue;
        }
        let Some(digest) = m.get("digest").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(platform) = m.get("platform") else {
            continue;
        };
        let os = platform.get("os").and_then(|v| v.as_str()).unwrap_or("");
        let arch = platform
            .get("architecture")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if os != "linux" || arch != target_arch {
            continue;
        }
        let variant = platform
            .get("variant")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if target_arch == "arm64" && !variant.is_empty() && variant != "v8" {
            fallback.get_or_insert_with(|| digest.to_string());
            continue;
        }
        return Ok(digest.to_string());
    }

    fallback
        .ok_or_else(|| anyhow!("no compatible linux/{target_arch} manifest found in image index"))
}

enum PullAuth {
    Bearer(String),
    Basic(String, String),
}

async fn authorized_get(
    client: &reqwest::Client,
    url: &str,
    accept: Option<&str>,
    auth_header: &mut Option<PullAuth>,
    creds: Option<&(String, String)>,
) -> Result<reqwest::Response> {
    let send = |auth: Option<&PullAuth>| {
        let mut req = client.get(url);
        if let Some(a) = accept {
            req = req.header(ACCEPT, a);
        }
        match auth {
            Some(PullAuth::Bearer(tok)) => {
                req = req.header(AUTHORIZATION, format!("Bearer {tok}"));
            }
            Some(PullAuth::Basic(u, p)) => {
                req = req.basic_auth(u, Some(p));
            }
            None => {}
        }
        req.send()
    };

    let resp = send(auth_header.as_ref()).await.context("HTTP GET failed")?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return resp
            .error_for_status()
            .with_context(|| format!("registry returned error for {url}"));
    }

    let www_auth = resp
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .unwrap_or("");

    if let Some(challenge) = parse_bearer_challenge(www_auth) {
        let token = fetch_bearer_token_with_creds(client, &challenge, creds).await?;
        *auth_header = Some(PullAuth::Bearer(token));
    } else if let Some((u, p)) = creds {
        *auth_header = Some(PullAuth::Basic(u.clone(), p.clone()));
    } else {
        bail!(
            "401 Unauthorized from {url} (WWW-Authenticate: '{www_auth}') — run `coral login` first"
        );
    }

    let retry = send(auth_header.as_ref())
        .await
        .context("authenticated HTTP GET failed")?;
    retry
        .error_for_status()
        .with_context(|| format!("registry returned error for {url} after auth"))
}

fn safe_slug(display_tag: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(display_tag.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let short_hash = &hash[..8];

    let sanitized: String = display_tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    let prefix: String = trimmed.chars().take(48).collect();
    format!("{prefix}-{short_hash}")
}
