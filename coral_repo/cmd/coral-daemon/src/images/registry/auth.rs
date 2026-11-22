//! Registry HTTP authorization (Bearer token challenge / basic auth).

use crate::images::push::fetch_bearer_token_with_creds;
use crate::images::reference::parse_bearer_challenge;
use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderValue, WWW_AUTHENTICATE};

#[derive(Debug, Clone)]
pub(super) enum PullAuth {
    Bearer(String),
    Basic(String, String),
}

pub(super) async fn authorized_get(
    client: &reqwest::Client,
    url: &str,
    accept: Option<&str>,
    auth_header: &mut Option<PullAuth>,
    creds: Option<&(String, String)>,
) -> Result<reqwest::Response> {
    let send = |auth: Option<&PullAuth>| {
        let mut req = client.get(url);
        if let Some(a) = accept {
            req = req.header(ACCEPT, a);
        }
        match auth {
            Some(PullAuth::Bearer(tok)) => {
                req = req.header(AUTHORIZATION, format!("Bearer {tok}"));
            }
            Some(PullAuth::Basic(u, p)) => {
                req = req.basic_auth(u, Some(p));
            }
            None => {}
        }
        req.send()
    };

    let resp = send(auth_header.as_ref()).await.context("HTTP GET failed")?;
    if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
        return resp
            .error_for_status()
            .with_context(|| format!("registry returned error for {url}"));
    }

    let www_auth = resp
        .headers()
        .get(WWW_AUTHENTICATE)
        .and_then(|v: &HeaderValue| v.to_str().ok())
        .unwrap_or("");

    if let Some(challenge) = parse_bearer_challenge(www_auth) {
        let token = fetch_bearer_token_with_creds(client, &challenge, creds).await?;
        *auth_header = Some(PullAuth::Bearer(token));
    } else if let Some((u, p)) = creds {
        *auth_header = Some(PullAuth::Basic(u.clone(), p.clone()));
    } else {
        bail!(
            "401 Unauthorized from {url} (WWW-Authenticate: '{www_auth}') — run `coral login` first"
        );
    }

    let retry = send(auth_header.as_ref())
        .await
        .context("authenticated HTTP GET failed")?;
    retry
        .error_for_status()
        .with_context(|| format!("registry returned error for {url} after auth"))
}
