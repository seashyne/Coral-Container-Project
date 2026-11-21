//! Supply-chain security for Coral OCI images:
//! - Ed25519 keypair generation (`coral keygen`, mode `0600` private key)
//! - Deterministic SHA-256 rootfs tree Merkle digest + Cosign-style Ed25519 signing & verification
//!   (`coral sign`, `coral verify`) detecting any single-byte post-signing tampering
//! - Standards-compliant SPDX 2.3 JSON SBOM generation (`coral sbom`) from Alpine APK,
//!   Debian/Ubuntu DPKG, and WebAssembly (`.wasm`/`.wat`) workloads

mod digest;
mod keys;
mod model;
mod sbom;
mod sign;
#[cfg(test)]
mod tests;
mod util;

pub use digest::compute_image_tree_digest;
pub use keys::{default_key_paths, generate_keypair};
pub use model::{ImageSignatureEnvelope, KeypairReport, VerifyReport};
pub use sbom::generate_spdx_sbom;
pub use sign::{sign_image, verify_image};
