//! Blob upload session handling for the registry v2 push API.

use super::auth::send_with_auth;
use super::model::RegistryAuthHeader;
use anyhow::{Context, Result, anyhow};
use reqwest::header::{CONTENT_TYPE, LOCATION};

pub(super) async fn upload_blob_if_missing(
    client: &reqwest::Client,
    base_url: &str,
    repository: &str,
    digest: &str,
    bytes: Vec<u8>,
    auth: &mut RegistryAuthHeader,
    creds: Option<&(String, String)>,
) -> Result<()> {
    // Check if blob already exists on registry (`HEAD /v2/<repo>/blobs/<digest>`).
    let head_url = format!("{base_url}/v2/{repository}/blobs/{digest}");
    let head_resp = auth.apply(client.head(&head_url)).send().await;
    if let Ok(r) = head_resp
        && r.status().is_success()
    {
        return Ok(());
    }

    // Start upload session (`POST /v2/<repo>/blobs/uploads/`).
    let start_url = format!("{base_url}/v2/{repository}/blobs/uploads/");
    let start_resp = send_with_auth(client, auth, creds, |c, a| a.apply(c.post(&start_url)))
        .await
        .with_context(|| format!("initiate blob upload at {start_url}"))?;

    let raw_loc = start_resp
        .headers()
        .get(LOCATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow!("registry did not return Location header for blob upload"))?;

    let upload_base = if raw_loc.starts_with("http://") || raw_loc.starts_with("https://") {
        raw_loc.to_string()
    } else if raw_loc.starts_with('/') {
        format!("{base_url}{raw_loc}")
    } else {
        format!("{base_url}/{raw_loc}")
    };
    let sep = if upload_base.contains('?') { '&' } else { '?' };
    let put_url = format!("{upload_base}{sep}digest={digest}");

    send_with_auth(client, auth, creds, |c, a| {
        a.apply(
            c.put(&put_url)
                .header(CONTENT_TYPE, "application/octet-stream")
                .body(bytes.clone()),
        )
    })
    .await
    .with_context(|| format!("upload blob {digest} to {put_url}"))?;

    Ok(())
}
