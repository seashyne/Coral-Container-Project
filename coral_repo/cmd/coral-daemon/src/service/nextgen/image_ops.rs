//! Image signing / verification / SBOM hooks.

use crate::service::CoralDaemonService;
use crate::images;
use std::collections::HashMap;
use std::path::PathBuf;
use tonic::Status;

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_sign(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let custom_priv = annotations
            .get("coral.image.key")
            .map(PathBuf::from);
        let custom_pub = annotations
            .get("coral.image.pub_key")
            .map(PathBuf::from);
        let images_dir = self.images_dir.clone();
        let out = tokio::task::spawn_blocking(move || {
            let (def_priv, def_pub) = images::security::default_key_paths();
            let priv_key = custom_priv.unwrap_or(def_priv);
            let pub_key = custom_pub.unwrap_or(def_pub);
            if !priv_key.exists() {
                images::security::generate_keypair(&priv_key, &pub_key, false)?;
            }
            images::security::sign_image(&images_dir, &image_ref, &priv_key)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_sign: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("sign image failed: {e:#}")))?;
        serde_json::to_string(&out)
            .map_err(|e| Status::internal(format!("serialize sign report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_verify(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let custom_pub = annotations
            .get("coral.image.pub_key")
            .map(PathBuf::from);
        let images_dir = self.images_dir.clone();
        let out = tokio::task::spawn_blocking(move || {
            let (_def_priv, def_pub) = images::security::default_key_paths();
            let pub_key = custom_pub.unwrap_or(def_pub);
            images::security::verify_image(&images_dir, &image_ref, &pub_key)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_verify: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("verify image failed: {e:#}")))?;
        serde_json::to_string(&out)
            .map_err(|e| Status::internal(format!("serialize verify report: {e}")))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_image_sbom(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image_ref = require_image_ref(annotations)?;
        let images_dir = self.images_dir.clone();
        let sbom = tokio::task::spawn_blocking(move || {
            images::security::generate_spdx_sbom(&images_dir, &image_ref)
        })
        .await
        .map_err(|e| Status::internal(format!("join image_sbom: {e}")))?
        .map_err(|e| Status::invalid_argument(format!("generate SBOM failed: {e:#}")))?;
        serde_json::to_string(&sbom)
            .map_err(|e| Status::internal(format!("serialize SBOM: {e}")))
    }
}

#[allow(clippy::result_large_err)]
fn require_image_ref(annotations: &HashMap<String, String>) -> Result<String, Status> {
    annotations
        .get("coral.image.ref")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Status::invalid_argument("coral.image.ref is required"))
}
