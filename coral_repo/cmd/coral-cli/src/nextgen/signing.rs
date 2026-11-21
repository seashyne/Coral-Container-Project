//! `coral keygen`, `coral sign`, `coral verify` and `coral sbom` — Ed25519 image signing
//! and SPDX SBOM generation.

use anyhow::{Context, Result};
use coral_daemon::images;
use std::fs;
use std::path::{Path, PathBuf};

pub fn cmd_keygen(
    key: Option<PathBuf>,
    pub_key: Option<PathBuf>,
    force: bool,
    as_json: bool,
) -> Result<()> {
    let (def_priv, def_pub) = images::default_key_paths();
    let priv_path = key.unwrap_or(def_priv);
    let pub_path = pub_key.unwrap_or(def_pub);
    let rep = images::generate_keypair(&priv_path, &pub_path, force)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rep)?);
    } else {
        println!("Generated Coral Ed25519 signing keypair:");
        println!("  Private Key (0600): {}", rep.private_key_path);
        println!("  Public Key:         {}", rep.public_key_path);
        println!("  Fingerprint:        {}", rep.fingerprint);
    }
    Ok(())
}

pub fn cmd_sign(images_dir: &Path, image: &str, key: Option<PathBuf>, as_json: bool) -> Result<()> {
    let (def_priv, _) = images::default_key_paths();
    let priv_path = key.unwrap_or(def_priv);
    let env = images::sign_image(images_dir, image, &priv_path)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&env)?);
    } else {
        println!("Signed image '{}' ({})", env.image_ref, env.algorithm);
        println!("  Tree Digest: {}", env.tree_digest);
        println!("  Signer:      {}", env.signer_fingerprint);
        println!("  Signed At:   {}", env.signed_at);
    }
    Ok(())
}

pub fn cmd_verify(
    images_dir: &Path,
    image: &str,
    key: Option<PathBuf>,
    as_json: bool,
) -> Result<()> {
    let (_, def_pub) = images::default_key_paths();
    let pub_path = key.unwrap_or(def_pub);
    let rep = images::verify_image(images_dir, image, &pub_path)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rep)?);
    } else {
        println!(
            "VERIFIED '{}' (Ed25519 + SHA-256 rootfs Merkle tree intact)",
            rep.image_ref
        );
        println!("  Tree Digest: {}", rep.tree_digest);
        println!("  Signer:      {}", rep.signer_fingerprint);
        println!("  Signed At:   {}", rep.signed_at);
    }
    Ok(())
}

pub fn cmd_sbom(images_dir: &Path, image: &str, output: Option<PathBuf>) -> Result<()> {
    let sbom = images::generate_spdx_sbom(images_dir, image)?;
    let pretty = serde_json::to_string_pretty(&sbom)?;
    if let Some(out) = output {
        fs::write(&out, &pretty).with_context(|| format!("write SBOM '{}'", out.display()))?;
        let count = sbom["packages"].as_array().map(|a| a.len()).unwrap_or(0);
        println!(
            "Wrote SPDX 2.3 SBOM ({count} packages) to {}",
            out.display()
        );
    } else {
        println!("{pretty}");
    }
    Ok(())
}
