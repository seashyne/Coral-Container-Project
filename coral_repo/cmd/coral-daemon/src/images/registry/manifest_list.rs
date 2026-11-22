//! Multi-Architecture OCI Image Index (`application/vnd.oci.image.index.v1+json`)
//! builder and Registry v2 manifest-list pusher (`coral build --platform a,b --push`).

use crate::images::auth::load_registry_auth;
use crate::images::reference::ImageRef;
use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const OCI_IMAGE_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
pub const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Single platform descriptor inside a multi-arch OCI Image Index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformManifestEntry {
    pub digest: String,
    pub size: u64,
    pub os: String,
    pub architecture: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// Builds a standards-compliant OCI Image Index v1 JSON document from platform manifests.
pub fn build_oci_image_index(entries: &[PlatformManifestEntry]) -> serde_json::Value {
    let manifests: Vec<serde_json::Value> = entries
        .iter()
        .map(|e| {
            let mut platform = serde_json::json!({
                "os": e.os,
                "architecture": e.architecture,
            });
            if let Some(ref v) = e.variant
                && !v.is_empty()
            {
                platform["variant"] = serde_json::Value::String(v.clone());
            }
            serde_json::json!({
                "mediaType": OCI_MANIFEST_MEDIA_TYPE,
                "digest": e.digest,
                "size": e.size,
                "platform": platform,
            })
        })
        .collect();

    serde_json::json!({
        "schemaVersion": 2,
        "mediaType": OCI_IMAGE_INDEX_MEDIA_TYPE,
        "manifests": manifests,
    })
}

/// Pushes a multi-architecture OCI Image Index manifest list to `image_ref` on an OCI Registry v2
/// and returns its `sha256:...` digest.
pub async fn push_oci_image_index(
    image_ref: &str,
    entries: &[PlatformManifestEntry],
) -> Result<String> {
    if entries.is_empty() {
        bail!("cannot push an empty OCI Image Index");
    }
    let (registry, repository, reference) = match ImageRef::parse(image_ref)? {
        ImageRef::Oci {
            registry,
            repository,
            reference,
            ..
        } => (registry, repository, reference),
        ImageRef::Scratch => bail!("cannot push OCI Image Index for 'scratch'"),
    };
    let client = Client::builder()
        .user_agent(concat!("coral/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let creds = load_registry_auth(&registry);
    let scheme = if registry.starts_with("localhost") || registry.starts_with("127.0.0.1") {
        "http"
    } else {
        "https"
    };
    let manifest_url = format!("{scheme}://{registry}/v2/{repository}/manifests/{reference}");

    let index_json = build_oci_image_index(entries);
    let body = serde_json::to_vec_pretty(&index_json)?;
    let digest = format!("sha256:{:x}", Sha256::digest(&body));

    let mut req = client
        .put(&manifest_url)
        .header("Content-Type", OCI_IMAGE_INDEX_MEDIA_TYPE)
        .body(body.clone());
    if let Some((u, p)) = creds {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("PUT OCI Image Index to {manifest_url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let err_body = resp.text().await.unwrap_or_default();
        bail!("registry rejected OCI Image Index ({status}): {err_body}");
    }
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_valid_multi_arch_oci_image_index() {
        let idx = build_oci_image_index(&[
            PlatformManifestEntry {
                digest: "sha256:aaaa".into(),
                size: 1024,
                os: "linux".into(),
                architecture: "amd64".into(),
                variant: None,
            },
            PlatformManifestEntry {
                digest: "sha256:bbbb".into(),
                size: 1048,
                os: "linux".into(),
                architecture: "arm64".into(),
                variant: Some("v8".into()),
            },
        ]);
        assert_eq!(idx["schemaVersion"], 2);
        assert_eq!(idx["mediaType"], OCI_IMAGE_INDEX_MEDIA_TYPE);
        let arr = idx["manifests"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["platform"]["architecture"], "amd64");
        assert_eq!(arr[1]["platform"]["architecture"], "arm64");
        assert_eq!(arr[1]["platform"]["variant"], "v8");
    }
}
