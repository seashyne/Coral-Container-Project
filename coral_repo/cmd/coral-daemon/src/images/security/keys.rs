//! Ed25519 keypair generation for Cosign-style image signing.

use super::model::KeypairReport;
use super::util::{PEM_PRIV_BEGIN, PEM_PRIV_END, PEM_PUB_BEGIN, PEM_PUB_END, sha256_hex};
use crate::images::auth::base64_encode;
use anyhow::{Context, Result, bail};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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
