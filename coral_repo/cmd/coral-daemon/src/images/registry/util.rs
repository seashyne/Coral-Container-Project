//! Shared registry constants and helpers.

use sha2::{Digest, Sha256};

/// Minimum spacing between progress snapshot flushes while streaming layer bytes.
pub(super) const PROGRESS_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);

pub(super) const ACCEPT_MANIFESTS: &str = concat!(
    "application/vnd.oci.image.index.v1+json, ",
    "application/vnd.docker.distribution.manifest.list.v2+json, ",
    "application/vnd.oci.image.manifest.v1+json, ",
    "application/vnd.docker.distribution.manifest.v2+json"
);

pub(super) fn safe_slug(display_tag: &str) -> String {
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
