//! OCI manifest / multi-arch index parsing helpers.

use anyhow::{Result, anyhow};

#[derive(Debug, Clone)]
pub(super) struct LayerDescriptor {
    pub(super) digest: String,
    pub(super) media_type: String,
    /// Declared compressed layer size from the manifest (`0` when absent).
    pub(super) size: u64,
}

pub(super) fn parse_layers(manifest: &serde_json::Value) -> Result<Vec<LayerDescriptor>> {
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
        let size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        layers.push(LayerDescriptor {
            digest,
            media_type,
            size,
        });
    }
    Ok(layers)
}

pub(super) fn parse_config_digest(manifest: &serde_json::Value) -> Option<String> {
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

/// Selects a matching `linux/<arch>` manifest digest from an OCI image index,
/// honoring an explicit `platform_override` such as `"linux/arm64"` or `"arm64"`.
pub(super) fn select_linux_platform_digest_for(
    manifests: &[serde_json::Value],
    platform_override: Option<&str>,
) -> Result<String> {
    let target_arch = platform_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|p| {
            let arch = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase();
            match arch.as_str() {
                "x86_64" | "amd64" => "amd64".to_string(),
                "aarch64" | "arm64" | "arm64v8" => "arm64".to_string(),
                other => other.to_string(),
            }
        })
        .unwrap_or_else(|| host_oci_arch().to_string());
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
