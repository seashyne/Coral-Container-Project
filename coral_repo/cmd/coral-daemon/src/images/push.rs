//! OCI Distribution v1.1 / Docker Registry v2 image pusher (`coral push`).

use super::auth::{load_registry_auth, registry_scheme};
use super::reference::{ImageRef, parse_bearer_challenge};
use super::StoredImage;
use anyhow::{Context, Result, anyhow, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION, WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing::info;

const MEDIA_OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const MEDIA_OCI_CONFIG: &str = "application/vnd.oci.image.config.v1+json";
const MEDIA_OCI_LAYER_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

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
enum RegistryAuthHeader {
    None,
    Bearer(String),
    Basic(String, String),
}

impl RegistryAuthHeader {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            Self::None => req,
            Self::Bearer(tok) => req.header(AUTHORIZATION, format!("Bearer {tok}")),
            Self::Basic(u, p) => req.basic_auth(u, Some(p)),
        }
    }
}

/// Packages `stored.rootfs_path` into an OCI gzipped layer + config JSON + OCI manifest
/// and pushes it to the registry specified by `image_ref_str`.
pub async fn push_image(image_ref_str: &str, stored: &StoredImage) -> Result<PushResult> {
    let parsed = ImageRef::parse(image_ref_str)?;
    let (registry, repository, reference, display_tag) = match parsed {
        ImageRef::Scratch => bail!("cannot push the special 'scratch' base image"),
        ImageRef::Oci {
            registry,
            repository,
            reference,
            display_tag,
        } => (registry, repository, reference, display_tag),
    };

    let rootfs = PathBuf::from(&stored.rootfs_path);
    if !rootfs.is_dir() {
        bail!("image rootfs '{}' does not exist", rootfs.display());
    }

    info!(
        image = %display_tag,
        registry = %registry,
        repository = %repository,
        reference = %reference,
        "packaging and pushing OCI image"
    );

    // 1. Pack rootfs into gzipped tar blob + compute uncompressed diff_id & compressed digest.
    let (layer_bytes, diff_id, layer_digest) =
        tokio::task::spawn_blocking(move || pack_rootfs_layer_gzip(&rootfs))
            .await
            .context("join rootfs packaging task")??;
    let layer_size = layer_bytes.len() as u64;

    // 2. Build OCI v1 image config JSON.
    let config_json = serde_json::json!({
        "created": chrono::Utc::now().to_rfc3339(),
        "architecture": host_oci_arch(),
        "os": "linux",
        "config": {
            "WorkingDir": "/",
            "Env": ["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"],
            "Cmd": ["/bin/sh"],
        },
        "rootfs": {
            "type": "layers",
            "diff_ids": [diff_id],
        }
    });
    let config_bytes = serde_json::to_vec(&config_json)?;
    let config_digest = sha256_digest(&config_bytes);
    let config_size = config_bytes.len() as u64;

    // 3. Build OCI v1 image manifest JSON.
    let manifest_json = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": MEDIA_OCI_MANIFEST,
        "config": {
            "mediaType": MEDIA_OCI_CONFIG,
            "digest": config_digest,
            "size": config_size,
        },
        "layers": [{
            "mediaType": MEDIA_OCI_LAYER_GZIP,
            "digest": layer_digest,
            "size": layer_size,
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest_json)?;
    let local_manifest_digest = sha256_digest(&manifest_bytes);

    let scheme = registry_scheme(&registry);
    let base_url = format!("{scheme}://{registry}");
    let creds = load_registry_auth(&registry);
    let client = reqwest::Client::builder()
        .user_agent("coral-daemon/1.3")
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build HTTP client for push")?;

    let mut auth = RegistryAuthHeader::None;

    // 4. Upload config blob and layer blob.
    upload_blob_if_missing(
        &client,
        &base_url,
        &repository,
        &config_digest,
        config_bytes,
        &mut auth,
        creds.as_ref(),
    )
    .await
    .context("upload OCI image config blob")?;

    upload_blob_if_missing(
        &client,
        &base_url,
        &repository,
        &layer_digest,
        layer_bytes,
        &mut auth,
        creds.as_ref(),
    )
    .await
    .context("upload OCI image layer blob")?;

    // 5. PUT OCI image manifest.
    let manifest_url = format!("{base_url}/v2/{repository}/manifests/{reference}");
    let resp = send_with_auth(
        &client,
        &mut auth,
        creds.as_ref(),
        |c, a| {
            a.apply(
                c.put(&manifest_url)
                    .header(CONTENT_TYPE, MEDIA_OCI_MANIFEST)
                    .body(manifest_bytes.clone()),
            )
        },
    )
    .await
    .with_context(|| format!("push manifest to {manifest_url}"))?;

    let remote_digest = resp
        .headers()
        .get("docker-content-digest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or(local_manifest_digest);

    Ok(PushResult {
        image: display_tag,
        registry,
        repository,
        reference,
        manifest_digest: remote_digest,
        layer_digest,
        layer_size,
    })
}

async fn upload_blob_if_missing(
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

async fn send_with_auth<F>(
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

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
}

impl<W: Write> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn finish(self) -> (W, String) {
        (self.inner, format!("sha256:{:x}", self.hasher.finalize()))
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn pack_rootfs_layer_gzip(rootfs: &Path) -> Result<(Vec<u8>, String, String)> {
    let gz = GzEncoder::new(Vec::new(), Compression::default());
    let hw = HashingWriter::new(gz);
    let mut tar_builder = tar::Builder::new(hw);
    tar_builder.follow_symlinks(false);
    tar_builder
        .append_dir_all(".", rootfs)
        .with_context(|| format!("tar rootfs '{}'", rootfs.display()))?;
    let hw = tar_builder.into_inner()?;
    let (gz, diff_id) = hw.finish();
    let compressed = gz.finish().context("finish gzip layer stream")?;
    let layer_digest = sha256_digest(&compressed);
    Ok((compressed, diff_id, layer_digest))
}

fn sha256_digest(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("sha256:{:x}", h.finalize())
}

fn host_oci_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "arm",
        "riscv64" => "riscv64",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::auth::{base64_encode, login_registry_at};
    use crate::images::pull_image;
    use std::fs;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Default)]
    struct MockRegistry {
        blobs: HashMap<String, Vec<u8>>,
        manifests: HashMap<String, Vec<u8>>,
    }

    #[tokio::test]
    async fn push_and_pull_authenticated_roundtrip_against_local_oci_registry() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(MockRegistry::default()));
        let expected_basic = format!("Basic {}", base64_encode(b"coraluser:coralpass"));

        let srv_state = state.clone();
        let srv_auth = expected_basic.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let st = srv_state.clone();
                let auth_ok = srv_auth.clone();
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut buf = [0u8; 4096];
                    let header_end;
                    loop {
                        let Ok(n) = sock.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        raw.extend_from_slice(&buf[..n]);
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            header_end = pos + 4;
                            break;
                        }
                    }
                    let head_str = String::from_utf8_lossy(&raw[..header_end]).to_string();
                    let mut lines = head_str.lines();
                    let req_line = lines.next().unwrap_or("");
                    let mut parts = req_line.split_whitespace();
                    let method = parts.next().unwrap_or("");
                    let target = parts.next().unwrap_or("");
                    let mut content_len = 0usize;
                    let mut auth_hdr = String::new();
                    for line in lines {
                        if let Some((k, v)) = line.split_once(':') {
                            if k.eq_ignore_ascii_case("content-length") {
                                content_len = v.trim().parse().unwrap_or(0);
                            } else if k.eq_ignore_ascii_case("authorization") {
                                auth_hdr = v.trim().to_string();
                            }
                        }
                    }
                    while raw.len() < header_end + content_len {
                        let Ok(n) = sock.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&buf[..n]);
                    }
                    let body = raw[header_end..header_end + content_len].to_vec();

                    if auth_hdr != auth_ok {
                        let resp = "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"coral-mock\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = sock.write_all(resp.as_bytes()).await;
                        return;
                    }

                    let (path_only, query) = target.split_once('?').unwrap_or((target, ""));
                    if method == "HEAD" && path_only.contains("/blobs/") {
                        let digest = path_only.rsplit('/').next().unwrap_or("");
                        let exists = st.lock().await.blobs.contains_key(digest);
                        let code = if exists { "200 OK" } else { "404 Not Found" };
                        let resp = format!("HTTP/1.1 {code}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if method == "POST" && path_only.ends_with("/blobs/uploads/") {
                        let resp = "HTTP/1.1 202 Accepted\r\nLocation: /v2/demo/app/blobs/uploads/u1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if method == "PUT" && path_only.contains("/blobs/uploads/") {
                        let digest = query.strip_prefix("digest=").unwrap_or("").to_string();
                        st.lock().await.blobs.insert(digest, body);
                        let resp = "HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if method == "PUT" && path_only.contains("/manifests/") {
                        let tag = path_only.rsplit('/').next().unwrap_or("").to_string();
                        let d = sha256_digest(&body);
                        st.lock().await.manifests.insert(tag, body);
                        let resp = format!("HTTP/1.1 201 Created\r\nDocker-Content-Digest: {d}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        let _ = sock.write_all(resp.as_bytes()).await;
                    } else if method == "GET" && path_only.contains("/manifests/") {
                        let tag = path_only.rsplit('/').next().unwrap_or("");
                        if let Some(m) = st.lock().await.manifests.get(tag).cloned() {
                            let d = sha256_digest(&m);
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {MEDIA_OCI_MANIFEST}\r\nDocker-Content-Digest: {d}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                m.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(&m).await;
                        } else {
                            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        }
                    } else if method == "GET" && path_only.contains("/blobs/") {
                        let digest = path_only.rsplit('/').next().unwrap_or("");
                        if let Some(b) = st.lock().await.blobs.get(digest).cloned() {
                            let head = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                b.len()
                            );
                            let _ = sock.write_all(head.as_bytes()).await;
                            let _ = sock.write_all(&b).await;
                        } else {
                            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        }
                    }
                });
            }
        });

        let work = tempdir().unwrap();
        let auth_file = work.path().join("auth.json");
        let reg_host = format!("127.0.0.1:{}", addr.port());
        login_registry_at(&auth_file, &reg_host, "coraluser", "coralpass").unwrap();
        // SAFETY: isolated test auth file path
        unsafe { std::env::set_var("CORAL_AUTH_FILE", &auth_file) };

        let src_rootfs = work.path().join("src-rootfs");
        fs::create_dir_all(src_rootfs.join("etc")).unwrap();
        fs::write(src_rootfs.join("etc/coral-push-test"), "PUSH_PULL_ROUNDTRIP_OK\n").unwrap();

        let img_ref = format!("{reg_host}/demo/app:v1");
        let stored = StoredImage {
            id: "sha256:local".into(),
            repo_tags: vec![img_ref.clone()],
            size: 24,
            rootfs_path: src_rootfs.to_string_lossy().into_owned(),
            managed: false,
        };

        let pushed = push_image(&img_ref, &stored).await.unwrap();
        assert!(pushed.manifest_digest.starts_with("sha256:"));
        assert!(pushed.layer_size > 0);

        let pull_dir = work.path().join("pulled-images");
        let pulled = pull_image(&img_ref, &pull_dir).await.unwrap();
        let content =
            fs::read_to_string(Path::new(&pulled.rootfs_path).join("etc/coral-push-test")).unwrap();
        assert_eq!(content, "PUSH_PULL_ROUNDTRIP_OK\n");

        unsafe { std::env::remove_var("CORAL_AUTH_FILE") };
        server.abort();
    }
}
