use super::layer::sha256_digest;
use super::pusher::push_image;
use super::MEDIA_OCI_MANIFEST;
use crate::images::StoredImage;
use crate::images::auth::{base64_encode, login_registry_at};
use crate::images::pull_image;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
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
    // `None`: this roundtrip test asserts content, not progress reporting.
    let pulled = pull_image(&img_ref, &pull_dir, None).await.unwrap();
    let content =
        fs::read_to_string(Path::new(&pulled.rootfs_path).join("etc/coral-push-test")).unwrap();
    assert_eq!(content, "PUSH_PULL_ROUNDTRIP_OK\n");

    unsafe { std::env::remove_var("CORAL_AUTH_FILE") };
    server.abort();
}
