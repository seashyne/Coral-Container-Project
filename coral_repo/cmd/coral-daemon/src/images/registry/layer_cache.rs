//! Content-addressable OCI layer blob cache (`<images_dir>/.layer-cache/sha256/<hex>.blob`)
//! and bounded-concurrency parallel layer downloader.

use super::auth::{PullAuth, authorized_get};
use super::manifest::LayerDescriptor;
use super::util::PROGRESS_FLUSH_INTERVAL;
use crate::images::progress::PullProgressReporter;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;

/// Maximum number of OCI layer blobs downloaded concurrently during `coral pull`.
const MAX_CONCURRENT_LAYER_DOWNLOADS: usize = 4;

pub(super) struct DownloadedLayer {
    pub(super) idx: usize,
    pub(super) digest: String,
    pub(super) blob_path: PathBuf,
    pub(super) media_type: String,
    pub(super) cached_in_store: bool,
}

pub(super) struct LayerFetchContext<'a> {
    pub(super) client: &'a reqwest::Client,
    pub(super) scheme: &'a str,
    pub(super) registry: &'a str,
    pub(super) repository: &'a str,
    pub(super) display_tag: &'a str,
    pub(super) images_dir: &'a Path,
    pub(super) staging_dir: &'a Path,
    pub(super) auth_header: Option<PullAuth>,
    pub(super) creds: Option<(String, String)>,
    pub(super) reporter: Option<Arc<PullProgressReporter>>,
}

/// Resolves the content-addressable cache path `<images_dir>/.layer-cache/sha256/<hex>.blob`
/// for a valid `sha256:<hex>` layer digest.
pub(crate) fn cached_layer_blob_path(images_dir: &Path, digest: &str) -> Option<PathBuf> {
    let hex = digest.strip_prefix("sha256:")?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        images_dir
            .join(".layer-cache")
            .join("sha256")
            .join(format!("{hex}.blob")),
    )
}

/// Looks up a previously verified layer blob in `<images_dir>/.layer-cache/sha256/`
/// and confirms its SHA-256 digest matches `digest`.
pub(crate) fn lookup_cached_layer(images_dir: &Path, digest: &str) -> Option<(PathBuf, u64)> {
    let expected_hex = digest.strip_prefix("sha256:")?;
    let path = cached_layer_blob_path(images_dir, digest)?;
    let meta = fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let mut file = File::open(&path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hex = format!("{:x}", hasher.finalize());
    if actual_hex != expected_hex {
        let _ = fs::remove_file(&path);
        return None;
    }
    Some((path, meta.len()))
}

/// Atomically stores a verified layer file into `<images_dir>/.layer-cache/sha256/<hex>.blob`.
pub(crate) fn promote_to_layer_cache(
    images_dir: &Path,
    digest: &str,
    verified_part_path: &Path,
) -> Option<PathBuf> {
    let cache_path = cached_layer_blob_path(images_dir, digest)?;
    let parent = cache_path.parent()?;
    fs::create_dir_all(parent).ok()?;
    if fs::rename(verified_part_path, &cache_path).is_ok() {
        return Some(cache_path);
    }
    if fs::copy(verified_part_path, &cache_path).is_ok() {
        let _ = fs::remove_file(verified_part_path);
        return Some(cache_path);
    }
    None
}

/// Downloads all layers in `layers` with bounded concurrency (`MAX_CONCURRENT_LAYER_DOWNLOADS`),
/// reusing verified blobs from `<images_dir>/.layer-cache/sha256/` whenever present, and returns
/// the downloaded layers sorted in manifest order (`0..layers.len()`).
pub(super) async fn download_layers_parallel(
    ctx: &LayerFetchContext<'_>,
    layers: &[LayerDescriptor],
) -> Result<Vec<DownloadedLayer>> {
    let total_layers = layers.len();
    let declared_total: u64 = layers.iter().map(|l| l.size).sum();
    if let Some(rep) = &ctx.reporter {
        rep.set_layers_total(total_layers);
        rep.set_bytes_total(declared_total);
        rep.flush();
    }

    let tasks = layers.iter().cloned().enumerate().map(|(idx, layer)| {
        let client = ctx.client.clone();
        let scheme = ctx.scheme.to_string();
        let registry = ctx.registry.to_string();
        let repository = ctx.repository.to_string();
        let display_tag = ctx.display_tag.to_string();
        let images_dir = ctx.images_dir.to_path_buf();
        let staging_dir = ctx.staging_dir.to_path_buf();
        let mut auth_header = ctx.auth_header.clone();
        let creds = ctx.creds.clone();
        let reporter = ctx.reporter.clone();

        async move {
            if let Some((cached_path, cached_len)) = lookup_cached_layer(&images_dir, &layer.digest)
            {
                debug!(
                    image = %display_tag,
                    layer = idx + 1,
                    total = total_layers,
                    digest = %layer.digest,
                    "reusing cached layer blob"
                );
                let credit = if layer.size > 0 { layer.size } else { cached_len };
                if let Some(rep) = &reporter {
                    rep.begin_layer(idx + 1, credit);
                    rep.add_bytes(credit);
                    rep.flush();
                }
                return Ok::<DownloadedLayer, anyhow::Error>(DownloadedLayer {
                    idx,
                    digest: layer.digest,
                    blob_path: cached_path,
                    media_type: layer.media_type,
                    cached_in_store: true,
                });
            }

            debug!(
                image = %display_tag,
                layer = idx + 1,
                total = total_layers,
                digest = %layer.digest,
                "downloading layer"
            );
            let blob_url = format!("{scheme}://{registry}/v2/{repository}/blobs/{}", layer.digest);
            let blob_resp = authorized_get(
                &client,
                &blob_url,
                None,
                &mut auth_header,
                creds.as_ref(),
            )
            .await
            .with_context(|| format!("download layer {}", layer.digest))?;

            let content_length = blob_resp.content_length().unwrap_or(0);
            let layer_total = if content_length > 0 {
                content_length
            } else {
                layer.size
            };
            if let Some(rep) = &reporter {
                if layer_total > 0 && content_length > layer.size {
                    rep.set_bytes_total(
                        declared_total
                            .saturating_sub(layer.size)
                            .saturating_add(content_length),
                    );
                }
                rep.begin_layer(idx + 1, layer_total);
                rep.flush();
            }

            let part_path = staging_dir.join(format!("layer-{idx}.blob.part"));
            let mut out = File::create(&part_path)
                .with_context(|| format!("create layer temp file '{}'", part_path.display()))?;
            let mut hasher = Sha256::new();
            let mut stream = blob_resp.bytes_stream();
            let mut last_flush = std::time::Instant::now();
            while let Some(chunk) = stream.next().await {
                let bytes = chunk.context("stream layer chunk")?;
                hasher.update(&bytes);
                out.write_all(&bytes).context("write layer chunk")?;
                if let Some(rep) = &reporter {
                    rep.add_bytes(bytes.len() as u64);
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
                    let _ = fs::remove_file(&part_path);
                    bail!(
                        "layer digest mismatch for {}: expected sha256:{expected_hex}, got sha256:{actual_hex}",
                        display_tag
                    );
                }
            }

            if let Some(cached_path) = promote_to_layer_cache(&images_dir, &layer.digest, &part_path)
            {
                Ok(DownloadedLayer {
                    idx,
                    digest: layer.digest,
                    blob_path: cached_path,
                    media_type: layer.media_type,
                    cached_in_store: true,
                })
            } else {
                Ok(DownloadedLayer {
                    idx,
                    digest: layer.digest,
                    blob_path: part_path,
                    media_type: layer.media_type,
                    cached_in_store: false,
                })
            }
        }
    });

    let mut results: Vec<DownloadedLayer> = futures_util::stream::iter(tasks)
        .buffer_unordered(MAX_CONCURRENT_LAYER_DOWNLOADS)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    results.sort_by_key(|d| d.idx);
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn content_addressable_layer_cache_promotes_verifies_and_evicts_corrupt_blobs() {
        let dir = tempdir().unwrap();
        let payload = b"coral-shared-layer-blob-v1";
        let mut h = Sha256::new();
        h.update(payload);
        let digest = format!("sha256:{:x}", h.finalize());

        let part = dir.path().join("layer.part");
        fs::write(&part, payload).unwrap();
        let cached = promote_to_layer_cache(dir.path(), &digest, &part).unwrap();
        assert!(cached.is_file());
        assert!(!part.exists());

        let (found_path, found_len) = lookup_cached_layer(dir.path(), &digest).unwrap();
        assert_eq!(found_path, cached);
        assert_eq!(found_len, payload.len() as u64);

        // Corrupting the cached blob must cause lookup_cached_layer to reject and evict it.
        fs::write(&cached, b"corrupted").unwrap();
        assert!(lookup_cached_layer(dir.path(), &digest).is_none());
        assert!(!cached.exists());
    }
}
