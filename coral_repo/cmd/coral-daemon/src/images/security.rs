//! Supply-chain security for Coral OCI images:
//! - Ed25519 keypair generation (`coral keygen`, mode `0600` private key)
//! - Deterministic SHA-256 rootfs tree Merkle digest + Cosign-style Ed25519 signing & verification
//!   (`coral sign`, `coral verify`) detecting any single-byte post-signing tampering
//! - Standards-compliant SPDX 2.3 JSON SBOM generation (`coral sbom`) from Alpine APK,
//!   Debian/Ubuntu DPKG, and WebAssembly (`.wasm`/`.wat`) workloads

use super::auth::{base64_decode, base64_encode};
use super::{StoredImage, find_image, load_index};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use ring::rand::SystemRandom;
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

const PEM_PRIV_BEGIN: &str = "-----BEGIN CORAL ED25519 PRIVATE KEY-----";
const PEM_PRIV_END: &str = "-----END CORAL ED25519 PRIVATE KEY-----";
const PEM_PUB_BEGIN: &str = "-----BEGIN CORAL ED25519 PUBLIC KEY-----";
const PEM_PUB_END: &str = "-----END CORAL ED25519 PUBLIC KEY-----";

/// Summary returned by `coral keygen`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeypairReport {
    pub private_key_path: String,
    pub public_key_path: String,
    pub fingerprint: String,
}

/// Persisted Cosign-compatible image signature envelope (`<image_dir>/signature.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSignatureEnvelope {
    pub schema_version: String,
    pub algorithm: String,
    pub image_ref: String,
    pub tree_digest: String,
    pub signer_fingerprint: String,
    pub public_key_b64: String,
    pub payload_b64: String,
    pub signature_b64: String,
    pub signed_at: String,
}

/// Verification result returned by `coral verify`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub verified: bool,
    pub image_ref: String,
    pub tree_digest: String,
    pub signer_fingerprint: String,
    pub signed_at: String,
    pub signature_path: String,
}

/// Default locations for Coral Ed25519 signing keys (`~/.config/coral/cosign.{key,pub}`).
pub fn default_key_paths() -> (PathBuf, PathBuf) {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = base.join("coral");
    (dir.join("cosign.key"), dir.join("cosign.pub"))
}

/// Generates a new Ed25519 signing keypair and writes private key (`0600`) + public key (`0644`).
pub fn generate_keypair(priv_path: &Path, pub_path: &Path, force: bool) -> Result<KeypairReport> {
    if !force && (priv_path.exists() || pub_path.exists()) {
        bail!(
            "signing key already exists at '{}' (pass --force to overwrite)",
            priv_path.display()
        );
    }
    if let Some(parent) = priv_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = pub_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| anyhow::anyhow!("generate Ed25519 PKCS#8 keypair"))?;
    let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
        .map_err(|_| anyhow::anyhow!("parse generated Ed25519 PKCS#8 keypair"))?;
    let pub_bytes = keypair.public_key().as_ref();
    let fingerprint = format!("sha256:{}", sha256_hex(pub_bytes));

    let priv_pem = format!(
        "{PEM_PRIV_BEGIN}\n{}\n{PEM_PRIV_END}\n",
        base64_encode(pkcs8.as_ref())
    );
    let pub_pem = format!(
        "{PEM_PUB_BEGIN}\n{}\n{PEM_PUB_END}\n",
        base64_encode(pub_bytes)
    );

    fs::write(priv_path, priv_pem)
        .with_context(|| format!("write private key '{}'", priv_path.display()))?;
    fs::set_permissions(priv_path, fs::Permissions::from_mode(0o600))?;

    fs::write(pub_path, pub_pem)
        .with_context(|| format!("write public key '{}'", pub_path.display()))?;
    fs::set_permissions(pub_path, fs::Permissions::from_mode(0o644))?;

    Ok(KeypairReport {
        private_key_path: priv_path.display().to_string(),
        public_key_path: pub_path.display().to_string(),
        fingerprint,
    })
}

/// Computes a deterministic SHA-256 Merkle tree digest over every file, symlink, and
/// directory inside `rootfs` (including relative path, permissions, and file content).
pub fn compute_image_tree_digest(rootfs: &Path) -> Result<String> {
    if !rootfs.exists() {
        bail!("image rootfs '{}' does not exist", rootfs.display());
    }
    let mut entries = Vec::new();
    collect_paths(rootfs, Path::new(""), &mut entries)?;
    entries.sort();

    let mut hasher = Sha256::new();
    hasher.update(b"coral-image-tree-v1\n");
    for rel in entries {
        let full = rootfs.join(&rel);
        let meta = fs::symlink_metadata(&full)
            .with_context(|| format!("stat '{}'", full.display()))?;
        let rel_str = rel.to_string_lossy();
        let mode = meta.mode() & 0o7777;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&full)?;
            hasher.update(format!("L:{rel_str}:{mode:o}:{}\n", target.to_string_lossy()).as_bytes());
        } else if meta.is_dir() {
            hasher.update(format!("D:{rel_str}:{mode:o}\n").as_bytes());
        } else if meta.is_file() {
            let data = fs::read(&full)?;
            let file_hash = sha256_hex(&data);
            hasher.update(format!("F:{rel_str}:{mode:o}:{}:{file_hash}\n", data.len()).as_bytes());
        }
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

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

/// Generates a standards-compliant SPDX 2.3 JSON Software Bill of Materials (SBOM) by
/// inspecting Alpine APK DB (`/lib/apk/db/installed`), Debian/Ubuntu DPKG DB
/// (`/var/lib/dpkg/status`), and WebAssembly artifacts (`.wasm`/`.wat`) inside `image_ref`.
pub fn generate_spdx_sbom(images_dir: &Path, image_ref: &str) -> Result<serde_json::Value> {
    let img = resolve_stored_image(images_dir, image_ref)?;
    let rootfs = PathBuf::from(&img.rootfs_path);
    let tree_digest = compute_image_tree_digest(&rootfs)?;

    let mut packages = Vec::new();
    let apk_db = rootfs.join("lib/apk/db/installed");
    if apk_db.is_file() {
        packages.extend(parse_alpine_apk_db(&fs::read_to_string(&apk_db)?));
    }
    let dpkg_db = rootfs.join("var/lib/dpkg/status");
    if dpkg_db.is_file() {
        packages.extend(parse_dpkg_status(&fs::read_to_string(&dpkg_db)?));
    }
    if let Some(wasm_mod) = coral_wasm::find_wasm_module_in_rootfs(&rootfs)
        && let Ok(bytes) = fs::read(&wasm_mod)
    {
        let fname = wasm_mod
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("module.wasm");
        packages.push(serde_json::json!({
            "SPDXID": format!("SPDXRef-wasm-{}", sanitize_spdx_id(fname)),
            "name": fname,
            "versionInfo": "wasm32-wasi",
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": false,
            "checksums": [{
                "algorithm": "SHA256",
                "checksumValue": sha256_hex(&bytes),
            }],
            "externalRefs": [{
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": format!("pkg:generic/{fname}?type=wasm")
            }]
        }));
    }

    let created = Utc::now().to_rfc3339();
    Ok(serde_json::json!({
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": format!("coral-sbom-{image_ref}"),
        "documentNamespace": format!("https://coral.local/spdx/{}/{}", sanitize_spdx_id(image_ref), &tree_digest[7..19]),
        "creationInfo": {
            "created": created,
            "creators": [format!("Tool: coral-{}", env!("CARGO_PKG_VERSION"))]
        },
        "comment": format!("Image tree digest: {tree_digest}"),
        "packages": packages
    }))
}

fn parse_alpine_apk_db(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for block in content.split("\n\n") {
        let mut name = "";
        let mut version = "";
        let mut license = "NOASSERTION";
        let mut arch = "x86_64";
        let mut desc = "";
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("P:") {
                name = v.trim();
            } else if let Some(v) = line.strip_prefix("V:") {
                version = v.trim();
            } else if let Some(v) = line.strip_prefix("L:") {
                license = v.trim();
            } else if let Some(v) = line.strip_prefix("A:") {
                arch = v.trim();
            } else if let Some(v) = line.strip_prefix("T:") {
                desc = v.trim();
            }
        }
        if !name.is_empty() {
            out.push(serde_json::json!({
                "SPDXID": format!("SPDXRef-apk-{}", sanitize_spdx_id(&format!("{name}-{version}"))),
                "name": name,
                "versionInfo": version,
                "licenseConcluded": license,
                "summary": desc,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": false,
                "externalRefs": [{
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": format!("pkg:apk/alpine/{name}@{version}?arch={arch}")
                }]
            }));
        }
    }
    out
}

fn parse_dpkg_status(content: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for block in content.split("\n\n") {
        let mut name = "";
        let mut version = "";
        let mut arch = "amd64";
        for line in block.lines() {
            if let Some(v) = line.strip_prefix("Package:") {
                name = v.trim();
            } else if let Some(v) = line.strip_prefix("Version:") {
                version = v.trim();
            } else if let Some(v) = line.strip_prefix("Architecture:") {
                arch = v.trim();
            }
        }
        if !name.is_empty() {
            out.push(serde_json::json!({
                "SPDXID": format!("SPDXRef-deb-{}", sanitize_spdx_id(&format!("{name}-{version}"))),
                "name": name,
                "versionInfo": version,
                "downloadLocation": "NOASSERTION",
                "filesAnalyzed": false,
                "externalRefs": [{
                    "referenceCategory": "PACKAGE-MANAGER",
                    "referenceType": "purl",
                    "referenceLocator": format!("pkg:deb/debian/{name}@{version}?arch={arch}")
                }]
            }));
        }
    }
    out
}

fn resolve_stored_image(images_dir: &Path, image_ref: &str) -> Result<StoredImage> {
    let idx = load_index(images_dir);
    find_image(&idx, image_ref)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("image '{image_ref}' not found in {}", images_dir.display()))
}

fn signature_file_for_image(images_dir: &Path, img: &StoredImage) -> PathBuf {
    let safe_id = img.id.replace([':', '/'], "_");
    images_dir.join("signatures").join(format!("{safe_id}.sig.json"))
}

fn collect_paths(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let full = root.join(rel);
    for entry in fs::read_dir(&full)? {
        let entry = entry?;
        let child_rel = rel.join(entry.file_name());
        let meta = fs::symlink_metadata(entry.path())?;
        out.push(child_rel.clone());
        if meta.is_dir() && !meta.file_type().is_symlink() {
            collect_paths(root, &child_rel, out)?;
        }
    }
    Ok(())
}

fn read_pem_body(path: &Path, begin: &str, end: &str) -> Result<Vec<u8>> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read key file '{}'", path.display()))?;
    let body: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != begin && *l != end)
        .collect();
    base64_decode(&body).ok_or_else(|| anyhow::anyhow!("invalid base64 key in '{}'", path.display()))
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

fn sanitize_spdx_id(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{register_local_rootfs, save_index};
    use tempfile::tempdir;

    #[test]
    fn ed25519_sign_verify_and_tamper_detection_plus_spdx_sbom() {
        let dir = tempdir().unwrap();
        let images_dir = dir.path().join("images");
        let rootfs = dir.path().join("alpine-rootfs");
        fs::create_dir_all(rootfs.join("lib/apk/db")).unwrap();
        fs::write(rootfs.join("etc_release"), "3.20.3\n").unwrap();
        fs::write(
            rootfs.join("lib/apk/db/installed"),
            "P:busybox\nV:1.36.1-r29\nA:x86_64\nL:GPL-2.0-only\nT:Size optimized toolbox\n\n\
             P:musl\nV:1.2.5-r0\nA:x86_64\nL:MIT\nT:the musl c library\n\n",
        )
        .unwrap();

        let stored = register_local_rootfs("demo-signed:2.1", &rootfs).unwrap();
        save_index(&images_dir, &[stored]).unwrap();

        // 1. Keygen
        let priv_path = dir.path().join("cosign.key");
        let pub_path = dir.path().join("cosign.pub");
        let kp = generate_keypair(&priv_path, &pub_path, false).unwrap();
        assert!(kp.fingerprint.starts_with("sha256:"));

        // 2. Sign & Verify OK
        let env = sign_image(&images_dir, "demo-signed:2.1", &priv_path).unwrap();
        assert_eq!(env.algorithm, "Ed25519");
        let vr = verify_image(&images_dir, "demo-signed:2.1", &pub_path).unwrap();
        assert!(vr.verified);
        assert_eq!(vr.tree_digest, env.tree_digest);

        // 3. SBOM SPDX 2.3
        let sbom = generate_spdx_sbom(&images_dir, "demo-signed:2.1").unwrap();
        assert_eq!(sbom["spdxVersion"], "SPDX-2.3");
        let pkgs = sbom["packages"].as_array().unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0]["name"], "busybox");
        assert!(
            pkgs[0]["externalRefs"][0]["referenceLocator"]
                .as_str()
                .unwrap()
                .contains("pkg:apk/alpine/busybox@1.36.1-r29")
        );

        // 4. Tamper with 1 byte -> verify MUST reject!
        fs::write(rootfs.join("etc_release"), "3.20.4\n").unwrap();
        let err = verify_image(&images_dir, "demo-signed:2.1", &pub_path).unwrap_err();
        assert!(
            err.to_string().contains("TAMPERING DETECTED"),
            "unexpected error: {err}"
        );
    }
}
