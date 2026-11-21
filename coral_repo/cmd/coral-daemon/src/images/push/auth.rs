//! Registry push authorization (Bearer token challenge / basic auth).

use super::model::RegistryAuthHeader;
use crate::images::reference::parse_bearer_challenge;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::header::WWW_AUTHENTICATE;
use std::collections::HashMap;

pub(super) async fn send_with_auth<F>(
    client: &reqwest::Client,
    auth: &mut RegistryAuthHeader,
    creds: Option<&(String, String)>,
    build: F,
) -> Result<reqwest::Response>
where
    F: Fn(&reqwest::Client, &RegistryAuthHeader) -> reqwest::RequestBuilder,
{
    let resp = build(client, auth).send().await.context("HTTP request failed")?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return resp
            .error_for_status()
            .context("registry rejected push request");
    }

    let www_auth = resp
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(challenge) = parse_bearer_challenge(www_auth) {
        let token = fetch_bearer_token_with_creds(client, &challenge, creds).await?;
        *auth = RegistryAuthHeader::Bearer(token);
    } else if let Some((u, p)) = creds {
        *auth = RegistryAuthHeader::Basic(u.clone(), p.clone());
    } else {
        bail!(
            "registry requires authentication (401 Unauthorized) — run `coral login` first"
        );
    }

    let retry = build(client, auth)
        .send()
        .await
        .context("authenticated HTTP request failed")?;
    retry
        .error_for_status()
        .context("registry rejected authenticated push request")
}

pub(crate) async fn fetch_bearer_token_with_creds(
    client: &reqwest::Client,
    challenge: &HashMap<String, String>,
    creds: Option<&(String, String)>,
) -> Result<String> {
    let realm = challenge
        .get("realm")
        .ok_or_else(|| anyhow!("bearer challenge missing realm"))?;
    let mut query: Vec<(&str, &str)> = Vec::new();
    if let Some(service) = challenge.get("service") {
        query.push(("service", service.as_str()));
    }
    if let Some(scope) = challenge.get("scope") {
        query.push(("scope", scope.as_str()));
    }
    let mut req = client.get(realm).query(&query);
    if let Some((u, p)) = creds {
        req = req.basic_auth(u, Some(p));
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("request bearer token from {realm}"))?
        .error_for_status()
        .with_context(|| format!("token server {realm} rejected credentials"))?;
    let body: serde_json::Value = resp.json().await.context("parse token JSON")?;
    body.get("token")
        .or_else(|| body.get("access_token"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("token response from {realm} missing 'token'/'access_token'"))
}
