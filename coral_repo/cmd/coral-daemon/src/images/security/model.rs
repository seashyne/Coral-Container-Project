//! Serialized report types returned by the supply-chain security commands.

use serde::{Deserialize, Serialize};

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
