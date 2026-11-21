//! `coral compose` up / ps / logs / down lifecycle.

use super::*;

#[tokio::test]
async fn cri_compose_up_ps_logs_and_down_e2e() {
    let rootfs = alpine_rootfs();
    let coral_bin = ensure_coral_binary().clone();
    let daemon_dir = tempdir().unwrap();
    let volumes_dir = daemon_dir.path().join("volumes");

    let service = CoralDaemonService::new(CoralDaemonConfig {
        state_dir: daemon_dir.path().to_path_buf(),
        images_dir: None,
        coral_bin: coral_bin.clone(),
        default_rootfs: Some(rootfs),
    });

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_handle = tokio::spawn(async move {
        Server::builder()
            .add_service(RuntimeServiceServer::new(service.clone()))
            .add_service(ImageServiceServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let compose_file = daemon_dir.path().join("coral-compose.yml");
    std::fs::write(
        &compose_file,
        r#"
name: e2e-stack
services:
  app:
    image: alpine:3.20
    depends_on:
      - db
    environment:
      DB_HOST: db
    volumes:
      - stack-shared:/shared
    command: /bin/sh -c "echo APP_SEES_DB=$(grep -c db /etc/hosts) ENV=$DB_HOST"
  db:
    image: alpine:3.20
    volumes:
      - stack-shared:/shared
    command: /bin/sh -c "echo DB_BOOTED > /shared/db.flag && echo DB_READY"
volumes:
  stack-shared:
"#,
    )
    .unwrap();

    let endpoint = format!("http://{addr}");

    // 1. `coral compose up`
    let up = tokio::process::Command::new(&coral_bin)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("up")
        .output()
        .await
        .unwrap();
    let up_stdout = String::from_utf8_lossy(&up.stdout);
    let up_stderr = String::from_utf8_lossy(&up.stderr);
    assert!(
        up.status.success(),
        "coral compose up failed:\nstdout:\n{up_stdout}\nstderr:\n{up_stderr}"
    );
    assert!(up_stdout.contains("[db] DB_READY"), "stdout:\n{up_stdout}");
    assert!(
        up_stdout.contains("APP_SEES_DB=1 ENV=db"),
        "expected sibling service host + env in compose up output:\n{up_stdout}"
    );

    // 2. `coral compose ps`
    let ps = tokio::process::Command::new(&coral_bin)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("ps")
        .output()
        .await
        .unwrap();
    assert!(ps.status.success());
    let ps_stdout = String::from_utf8_lossy(&ps.stdout);
    assert!(ps_stdout.contains("db"), "ps:\n{ps_stdout}");
    assert!(ps_stdout.contains("app"), "ps:\n{ps_stdout}");

    // 3. `coral compose logs`
    let logs = tokio::process::Command::new(&coral_bin)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .arg("logs")
        .output()
        .await
        .unwrap();
    assert!(logs.status.success());
    let logs_stdout = String::from_utf8_lossy(&logs.stdout);
    assert!(logs_stdout.contains("DB_READY"), "logs:\n{logs_stdout}");

    // 4. `coral compose down -v`
    let down = tokio::process::Command::new(&coral_bin)
        .env("CORAL_VOLUMES_DIR", &volumes_dir)
        .args(["--addr", &endpoint, "compose", "-f"])
        .arg(&compose_file)
        .args(["down", "-v"])
        .output()
        .await
        .unwrap();
    assert!(
        down.status.success(),
        "coral compose down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    let down_stdout = String::from_utf8_lossy(&down.stdout);
    assert!(
        down_stdout.contains("2 container(s), 2 pod(s)"),
        "down:\n{down_stdout}"
    );
    assert!(!volumes_dir.join("stack-shared").exists());

    server_handle.abort();
}
