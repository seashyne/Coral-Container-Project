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
