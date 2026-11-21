//! Cosign-style Ed25519 signing and verification of stored Coral images.

use super::digest::compute_image_tree_digest;
use super::model::{ImageSignatureEnvelope, VerifyReport};
use super::util::{
    PEM_PRIV_BEGIN, PEM_PRIV_END, PEM_PUB_BEGIN, PEM_PUB_END, read_pem_body, resolve_stored_image,
    sha256_hex,
};
use crate::images::auth::{base64_decode, base64_encode};
use crate::images::StoredImage;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use std::fs;
use std::path::{Path, PathBuf};

/// Cryptographically signs a stored Coral image using an Ed25519 private key and writes
/// `<image_dir>/signature.json`.
pub fn sign_image(
    images_dir: &Path,
    image_ref: &str,
    priv_key_path: &Path,
) -> Result<ImageSignatureEnvelope> {
    let img = resolve_stored_image(images_dir, image_ref)?;
    let rootfs = PathBuf::from(&img.rootfs_path);
    let tree_digest = compute_image_tree_digest(&rootfs)?;

    let pkcs8_bytes = read_pem_body(priv_key_path, PEM_PRIV_BEGIN, PEM_PRIV_END)?;
    let keypair = Ed25519KeyPair::from_pkcs8(&pkcs8_bytes)
        .map_err(|_| anyhow::anyhow!("invalid Ed25519 private key '{}'", priv_key_path.display()))?;
    let pub_bytes = keypair.public_key().as_ref();
    let fingerprint = format!("sha256:{}", sha256_hex(pub_bytes));
    let signed_at = Utc::now().to_rfc3339();

    let payload = serde_json::json!({
        "critical": {
            "identity": { "docker-reference": image_ref },
            "image": { "coral-tree-digest": tree_digest },
            "type": "coral cosign container image signature"
        },
        "optional": {
            "signedAt": signed_at,
            "signerFingerprint": fingerprint,
            "coralVersion": env!("CARGO_PKG_VERSION")
        }
    });
    let payload_bytes = serde_json::to_vec(&payload)?;
    let sig = keypair.sign(&payload_bytes);

    let envelope = ImageSignatureEnvelope {
        schema_version: "coral.sig/v1".into(),
        algorithm: "Ed25519".into(),
        image_ref: image_ref.to_string(),
        tree_digest,
        signer_fingerprint: fingerprint,
        public_key_b64: base64_encode(pub_bytes),
        payload_b64: base64_encode(&payload_bytes),
        signature_b64: base64_encode(sig.as_ref()),
        signed_at,
    };

    let sig_path = signature_file_for_image(images_dir, &img);
    if let Some(parent) = sig_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&sig_path, serde_json::to_string_pretty(&envelope)?)
        .with_context(|| format!("write signature '{}'", sig_path.display()))?;

    Ok(envelope)
}

/// Verifies both the Ed25519 cryptographic signature AND the live rootfs Merkle digest of
/// a stored Coral image against `pub_key_path`.
pub fn verify_image(
    images_dir: &Path,
    image_ref: &str,
    pub_key_path: &Path,
) -> Result<VerifyReport> {
    let img = resolve_stored_image(images_dir, image_ref)?;
    let sig_path = signature_file_for_image(images_dir, &img);
    if !sig_path.is_file() {
        bail!(
            "no signature found for image '{}' (expected '{}'; sign first with `coral sign`)",
            image_ref,
            sig_path.display()
        );
    }
    let raw = fs::read_to_string(&sig_path)?;
    let env: ImageSignatureEnvelope = serde_json::from_str(&raw)
        .with_context(|| format!("parse signature '{}'", sig_path.display()))?;

    let expected_pub = read_pem_body(pub_key_path, PEM_PUB_BEGIN, PEM_PUB_END)?;
    let expected_fp = format!("sha256:{}", sha256_hex(&expected_pub));
    if env.signer_fingerprint != expected_fp {
        bail!(
            "signer key mismatch: signature was created by {}, but '{}' has fingerprint {}",
            env.signer_fingerprint,
            pub_key_path.display(),
            expected_fp
        );
    }

    let payload_bytes = base64_decode(&env.payload_b64)
        .ok_or_else(|| anyhow::anyhow!("corrupted base64 payload in signature"))?;
    let sig_bytes = base64_decode(&env.signature_b64)
        .ok_or_else(|| anyhow::anyhow!("corrupted base64 signature bytes"))?;

    let verifier = UnparsedPublicKey::new(&ED25519, &expected_pub);
    verifier
        .verify(&payload_bytes, &sig_bytes)
        .map_err(|_| anyhow::anyhow!("Ed25519 signature verification FAILED for '{image_ref}'"))?;

    // Verify signed payload's digest matches live rootfs tree (detects any post-signing tampering).
    let payload_val: serde_json::Value = serde_json::from_slice(&payload_bytes)?;
    let signed_digest = payload_val
        .pointer("/critical/image/coral-tree-digest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("signed payload missing coral-tree-digest"))?;

    let live_digest = compute_image_tree_digest(Path::new(&img.rootfs_path))?;
    if live_digest != signed_digest {
        bail!(
            "IMAGE TAMPERING DETECTED for '{}': signed digest {} != live rootfs digest {}",
            image_ref,
            signed_digest,
            live_digest
        );
    }

    Ok(VerifyReport {
        verified: true,
        image_ref: image_ref.to_string(),
        tree_digest: live_digest,
        signer_fingerprint: expected_fp,
        signed_at: env.signed_at,
        signature_path: sig_path.display().to_string(),
    })
}

fn signature_file_for_image(images_dir: &Path, img: &StoredImage) -> PathBuf {
    let safe_id = img.id.replace([':', '/'], "_");
    images_dir.join("signatures").join(format!("{safe_id}.sig.json"))
}
