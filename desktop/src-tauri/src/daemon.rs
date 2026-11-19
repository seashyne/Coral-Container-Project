use crate::settings::{Mode, Settings};
use coral_api::VersionRequest;
use coral_api::runtime_service_client::RuntimeServiceClient;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};

pub struct SpawnedDaemon {
    pub child: Child,
    pub mode: Mode,
}

fn endpoint_url(endpoint: &str) -> String {
    format!("http://{endpoint}")
}

pub async fn probe(endpoint: &str) -> bool {
    let Ok(mut client) = RuntimeServiceClient::connect(endpoint_url(endpoint)).await else {
        return false;
    };
    client
        .version(VersionRequest {
            version: "0.1.0".into(),
        })
        .await
        .is_ok()
}

fn local_command(settings: &Settings) -> Command {
    let mut cmd = Command::new(&settings.daemon_bin);
    cmd.arg("--listen").arg(&settings.endpoint);
    if !settings.coral_bin.trim().is_empty() {
        cmd.arg("--coral-bin").arg(&settings.coral_bin);
    }
    if !settings.state_dir.trim().is_empty() {
        cmd.arg("--state-dir").arg(&settings.state_dir);
    }
    if !settings.images_dir.trim().is_empty() {
        cmd.arg("--images-dir").arg(&settings.images_dir);
    }
    if !settings.rootfs.trim().is_empty() {
        cmd.arg("--default-rootfs").arg(&settings.rootfs);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

#[cfg(windows)]
fn wsl_command(settings: &Settings) -> Command {
    let mut cmd = Command::new("wsl.exe");
    if !settings.wsl_distro.trim().is_empty() {
        cmd.arg("-d").arg(&settings.wsl_distro);
    }
    let mut inner = format!(
        "{} --listen {} --coral-bin {}",
        shell_quote(&settings.daemon_bin),
        shell_quote(&settings.endpoint),
        shell_quote(&settings.coral_bin)
    );
    if !settings.state_dir.trim().is_empty() {
        inner.push_str(&format!(
            " --state-dir {}",
            shell_quote(&settings.state_dir)
        ));
    }
    if !settings.images_dir.trim().is_empty() {
        inner.push_str(&format!(
            " --images-dir {}",
            shell_quote(&settings.images_dir)
        ));
    }
    if !settings.rootfs.trim().is_empty() {
        inner.push_str(&format!(
            " --default-rootfs {}",
            shell_quote(&settings.rootfs)
        ));
    }
    cmd.arg("--").arg("sh").arg("-c").arg(inner);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    cmd
}

#[cfg(windows)]
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Ensures a daemon is reachable at the configured endpoint, spawning one if needed.
/// Returns the spawned child when this call started the daemon (None when attaching
/// to an already-running daemon or when the mode cannot spawn).
pub async fn ensure(settings: &Settings) -> Result<Option<SpawnedDaemon>, String> {
    if probe(&settings.endpoint).await {
        return Ok(None);
    }

    let mut cmd = match settings.mode {
        Mode::Local => {
            if cfg!(windows) {
                return Err(
                    "Local mode needs a Linux daemon. Use WSL2 mode or a remote endpoint on Windows."
                        .into(),
                );
            }
            local_command(settings)
        }
        Mode::Wsl2 => {
            #[cfg(windows)]
            {
                wsl_command(settings)
            }
            #[cfg(not(windows))]
            {
                return Err("WSL2 mode is only available on Windows.".into());
            }
        }
        Mode::Remote => {
            return Err(format!(
                "no daemon reachable at {} — start coral-daemon on the remote host first",
                settings.endpoint
            ));
        }
    };

    let child = cmd
        .spawn()
        .map_err(|e| format!("spawn daemon ({}): {e}", settings.daemon_bin))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline {
        if probe(&settings.endpoint).await {
            return Ok(Some(SpawnedDaemon {
                child,
                mode: settings.mode,
            }));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }

    let mut child = child;
    let _ = child.kill().await;
    Err(format!(
        "daemon did not become ready at {} within 20s",
        settings.endpoint
    ))
}
