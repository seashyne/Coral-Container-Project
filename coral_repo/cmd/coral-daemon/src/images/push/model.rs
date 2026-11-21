//! Push result payload and registry authorization header model.

use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResult {
    pub image: String,
    pub registry: String,
    pub repository: String,
    pub reference: String,
    pub manifest_digest: String,
    pub layer_digest: String,
    pub layer_size: u64,
}

#[derive(Debug, Clone)]
pub(super) enum RegistryAuthHeader {
    None,
    Bearer(String),
    Basic(String, String),
}

impl RegistryAuthHeader {
    pub(super) fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::None => req,
            Self::Bearer(tok) => req.header(AUTHORIZATION, format!("Bearer {tok}")),
            Self::Basic(u, p) => req.basic_auth(u, Some(p)),
        }
    }
}
