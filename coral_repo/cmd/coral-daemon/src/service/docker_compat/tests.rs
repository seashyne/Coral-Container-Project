use super::*;
use crate::service::{CoralDaemonConfig, CoralDaemonService};
use std::path::PathBuf;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::watch;

async fn send_unix_http(sock_path: &std::path::Path, req: &str) -> String {
    let mut stream = UnixStream::connect(sock_path).await.unwrap();
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    stream.read_to_string(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn docker_compat_unix_socket_serves_ping_version_info_images_and_container_lifecycle() {
    let dir = tempdir().unwrap();
    let rootfs = dir.path().join("fake-alpine");
    std::fs::create_dir_all(rootfs.join("bin")).unwrap();
    let sock_path = dir.path().join("docker.sock");

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("state"),
        images_dir: Some(dir.path().join("images")),
        coral_bin: PathBuf::from("coral"),
        default_rootfs: Some(rootfs),
    });

    let listener = UnixListener::bind(&sock_path).unwrap();
    let (tx, rx) = watch::channel(false);
    let server = tokio::spawn(serve_docker_unix(svc, listener, rx));

    // 1. GET /_ping
    let ping = send_unix_http(&sock_path, "GET /_ping HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
    assert!(ping.starts_with("HTTP/1.1 200 OK"));
    assert!(ping.ends_with("\r\n\r\nOK"));

    // 2. GET /v1.45/version & /v1.45/info
    let ver = send_unix_http(
        &sock_path,
        "GET /v1.45/version HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(ver.contains("\"ApiVersion\":\"1.45\""));

    let info = send_unix_http(
        &sock_path,
        "GET /v1.45/info HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(info.contains("\"Images\":1"));

    // 3. POST /v1.45/containers/create?name=test-compat
    let body = r#"{"Image":"alpine:3.20","Cmd":["/bin/echo","hi"],"Env":["FOO=bar"]}"#;
    let create_req = format!(
        "POST /v1.45/containers/create?name=test-compat HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let created = send_unix_http(&sock_path, &create_req).await;
    assert!(created.starts_with("HTTP/1.1 201 Created"), "got: {created}");

    // 4. GET /v1.45/containers/test-compat/json
    let inspected = send_unix_http(
        &sock_path,
        "GET /v1.45/containers/test-compat/json HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(inspected.contains("\"/test-compat\""));
    assert!(inspected.contains("\"Status\":\"created\""));

    // 5. GET /v1.45/containers/json?all=1, /images/alpine:3.20/json, /events, /networks, /volumes
    let listed = send_unix_http(
        &sock_path,
        "GET /v1.45/containers/json?all=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(listed.contains("\"/test-compat\""));

    let img_info = send_unix_http(
        &sock_path,
        "GET /v1.45/images/alpine:3.20/json HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(img_info.starts_with("HTTP/1.1 200 OK"), "got: {img_info}");
    assert!(img_info.contains("\"alpine:3.20\""));

    // Exec create + inspect
    let exec_body = r#"{"Cmd":["/bin/echo","hi"],"AttachStdout":true}"#;
    let exec_req = format!(
        "POST /v1.45/containers/test-compat/exec HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
        exec_body.len(),
        exec_body
    );
    let exec_resp = send_unix_http(&sock_path, &exec_req).await;
    assert!(exec_resp.starts_with("HTTP/1.1 201 Created"), "got: {exec_resp}");
    let exec_json_str = exec_resp.split("\r\n\r\n").nth(1).unwrap();
    let exec_id = serde_json::from_str::<serde_json::Value>(exec_json_str).unwrap()["Id"]
        .as_str()
        .unwrap()
        .to_string();
    let exec_inspect = send_unix_http(
        &sock_path,
        &format!("GET /v1.45/exec/{exec_id}/json HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    )
    .await;
    assert!(exec_inspect.starts_with("HTTP/1.1 200 OK"));

    let events = send_unix_http(
        &sock_path,
        "GET /v1.45/events HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(events.contains("\"Action\":\"create\""));

    let nets = send_unix_http(
        &sock_path,
        "GET /v1.45/networks HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(nets.starts_with("HTTP/1.1 200 OK"));

    let vols = send_unix_http(
        &sock_path,
        "GET /v1.45/volumes HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(vols.starts_with("HTTP/1.1 200 OK"));

    // 6. DELETE /v1.45/containers/test-compat
    let deleted = send_unix_http(
        &sock_path,
        "DELETE /v1.45/containers/test-compat HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(deleted.starts_with("HTTP/1.1 204 No Content"));

    let _ = tx.send(true);
    let _ = server.await;
}

async fn send_unix_http_bytes(sock_path: &std::path::Path, req: &[u8]) -> Vec<u8> {
    let mut stream = UnixStream::connect(sock_path).await.unwrap();
    stream.write_all(req).await.unwrap();
    let mut out = Vec::new();
    stream.read_to_end(&mut out).await.unwrap();
    out
}

#[tokio::test]
async fn docker_compat_supports_archive_build_rename_update_and_chunked_logs() {
    let dir = tempdir().unwrap();
    let rootfs = dir.path().join("fake-alpine");
    std::fs::create_dir_all(rootfs.join("bin")).unwrap();
    let sock_path = dir.path().join("docker2.sock");

    let svc = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: dir.path().join("state"),
        images_dir: Some(dir.path().join("images")),
        coral_bin: PathBuf::from("coral"),
        default_rootfs: Some(rootfs),
    });

    let listener = UnixListener::bind(&sock_path).unwrap();
    let (tx, rx) = watch::channel(false);
    let server = tokio::spawn(serve_docker_unix(svc, listener, rx));

    // 1. POST /build with an in-memory tar build context (`FROM scratch` + `COPY`)
    let mut tar_ctx = Vec::new();
    {
        let mut b = tar::Builder::new(&mut tar_ctx);
        let df = b"FROM scratch\nCOPY hello.txt /hello.txt\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(df.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        b.append_data(&mut hdr, "Dockerfile", &df[..]).unwrap();

        let hi = b"built-over-docker-sock\n";
        let mut hdr2 = tar::Header::new_gnu();
        hdr2.set_size(hi.len() as u64);
        hdr2.set_mode(0o644);
        hdr2.set_cksum();
        b.append_data(&mut hdr2, "hello.txt", &hi[..]).unwrap();
        b.finish().unwrap();
    }
    let mut build_req = format!(
        "POST /v1.45/build?t=sock-built:v1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-tar\r\nContent-Length: {}\r\n\r\n",
        tar_ctx.len()
    )
    .into_bytes();
    build_req.extend_from_slice(&tar_ctx);
    let build_resp = String::from_utf8_lossy(&send_unix_http_bytes(&sock_path, &build_req).await).into_owned();
    assert!(build_resp.starts_with("HTTP/1.1 200 OK"), "{build_resp}");
    assert!(build_resp.contains("Successfully tagged sock-built:v1"));

    // 2. Create a container, PUT/GET /archive, rename, update, and chunked logs?follow=1
    let body = r#"{"Image":"alpine:latest","Cmd":["/bin/echo","hi"]}"#;
    let create_req = format!(
        "POST /v1.45/containers/create?name=orig-box HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let created = send_unix_http(&sock_path, &create_req).await;
    let created_json: serde_json::Value =
        serde_json::from_str(created.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let cid = created_json["Id"].as_str().unwrap();

    // PUT /containers/{id}/archive?path=/etc
    let mut arch_tar = Vec::new();
    {
        let mut b = tar::Builder::new(&mut arch_tar);
        let data = b"archive-payload";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(data.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        b.append_data(&mut hdr, "sample.conf", &data[..]).unwrap();
        b.finish().unwrap();
    }
    let mut put_req = format!(
        "PUT /v1.45/containers/{cid}/archive?path=/etc HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        arch_tar.len()
    )
    .into_bytes();
    put_req.extend_from_slice(&arch_tar);
    let put_resp = String::from_utf8_lossy(&send_unix_http_bytes(&sock_path, &put_req).await).into_owned();
    assert!(put_resp.starts_with("HTTP/1.1 200 OK"), "{put_resp}");

    // GET /containers/{id}/archive?path=/etc/sample.conf
    let get_resp = send_unix_http_bytes(
        &sock_path,
        format!("GET /v1.45/containers/{cid}/archive?path=/etc/sample.conf HTTP/1.1\r\nHost: localhost\r\n\r\n").as_bytes(),
    )
    .await;
    assert!(get_resp.starts_with(b"HTTP/1.1 200 OK"));
    assert!(String::from_utf8_lossy(&get_resp).contains("archive-payload"));

    // POST /containers/{id}/rename?name=renamed-box
    let ren = send_unix_http(
        &sock_path,
        &format!("POST /v1.45/containers/{cid}/rename?name=renamed-box HTTP/1.1\r\nHost: localhost\r\n\r\n"),
    )
    .await;
    assert!(ren.starts_with("HTTP/1.1 204 No Content"), "{ren}");

    // POST /containers/{id}/update
    let upd_body = r#"{"Memory":134217728,"PidsLimit":128}"#;
    let upd = send_unix_http(
        &sock_path,
        &format!(
            "POST /v1.45/containers/renamed-box/update HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{upd_body}",
            upd_body.len()
        ),
    )
    .await;
    assert!(upd.starts_with("HTTP/1.1 200 OK"), "{upd}");

    // GET /containers/renamed-box/logs?follow=1 -> chunked transfer
    let logs = send_unix_http(
        &sock_path,
        "GET /v1.45/containers/renamed-box/logs?follow=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    )
    .await;
    assert!(logs.starts_with("HTTP/1.1 200 OK"), "{logs}");
    assert!(logs.contains("Transfer-Encoding: chunked"));

    let _ = tx.send(true);
    let _ = server.await;
}
