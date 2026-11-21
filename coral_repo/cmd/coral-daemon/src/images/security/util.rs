//! Shared key-file, image-lookup and digest helpers for image signing.

use crate::images::auth::base64_decode;
use crate::images::{StoredImage, find_image, load_index};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub(super) const PEM_PRIV_BEGIN: &str = "-----BEGIN CORAL ED25519 PRIVATE KEY-----";
pub(super) const PEM_PRIV_END: &str = "-----END CORAL ED25519 PRIVATE KEY-----";
pub(super) const PEM_PUB_BEGIN: &str = "-----BEGIN CORAL ED25519 PUBLIC KEY-----";
pub(super) const PEM_PUB_END: &str = "-----END CORAL ED25519 PUBLIC KEY-----";

pub(super) fn read_pem_body(path: &Path, begin: &str, end: &str) -> Result<Vec<u8>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read key file '{}'", path.display()))?;
    let body: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != begin && *l != end)
        .collect();
    base64_decode(&body).ok_or_else(|| anyhow::anyhow!("invalid base64 key in '{}'", path.display()))
}

pub(super) fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

pub(super) fn resolve_stored_image(images_dir: &Path, image_ref: &str) -> Result<StoredImage> {
    let idx = load_index(images_dir);
    find_image(&idx, image_ref)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("image '{image_ref}' not found in {}", images_dir.display()))
}
