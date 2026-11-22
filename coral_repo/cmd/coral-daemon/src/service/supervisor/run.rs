//! Runs the container binary while streaming stdout/stderr into the rotating log file.

use super::log_writer::{LogDriver, LogRotationPolicy, RotatingLogWriter};
use coral_oci::OciSpec;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

pub(super) async fn run_container_streaming_logs(
    coral_bin: &PathBuf,
    runtime_root: &PathBuf,
    bundle_dir: &PathBuf,
    cid: &str,
    log_path: &Path,
    log_policy: LogRotationPolicy,
) -> std::io::Result<(i32, String)> {
    // Clean up any stale OCI state left behind after an unclean host reboot or SIGKILL crash.
    if runtime_root.join(cid).exists() {
        let _ = Command::new(coral_bin)
            .arg("--root")
            .arg(runtime_root)
            .arg("delete")
            .arg("--force")
            .arg(cid)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }

    let driver = OciSpec::from_file(bundle_dir.join("config.json"))
        .ok()
        .and_then(|s| s.annotations.get("coral.log.driver").cloned())
        .map(|d| LogDriver::from_name(&d))
        .unwrap_or(LogDriver::Raw);
    let log_writer = Arc::new(StdMutex::new(RotatingLogWriter::open_with_driver(
        log_path.to_path_buf(),
        log_policy,
        driver,
        cid,
    )?));

    let mut child = Command::new(coral_bin)
        .arg("--root")
        .arg(runtime_root)
        .arg("run")
        .arg("--bundle")
        .arg(bundle_dir)
        .arg(cid)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let w_out = log_writer.clone();
    let out_task = tokio::spawn(async move {
        if let Some(mut reader) = stdout {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut w) = w_out.lock() {
                    let _ = w.write_chunk(&buf[..n]);
                }
            }
        }
    });

    let w_err = log_writer;
    let err_task = tokio::spawn(async move {
        let mut tail = Vec::new();
        if let Some(mut reader) = stderr {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                if let Ok(mut w) = w_err.lock() {
                    let _ = w.write_stream_chunk(&buf[..n], "stderr");
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 2048 {
                    let drop_n = tail.len() - 2048;
                    tail.drain(..drop_n);
                }
            }
        }
        String::from_utf8_lossy(&tail).trim().to_string()
    });

    let status = child.wait().await?;
    let _ = out_task.await;
    let stderr_tail = err_task.await.unwrap_or_default();
    Ok((status.code().unwrap_or(1), stderr_tail))
}
