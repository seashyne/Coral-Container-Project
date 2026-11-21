//! AI Agent / untrusted code hardened sandbox runner hook.

use crate::service::CoralDaemonService;
use crate::images;
use coral_vmm::{SandboxPolicy, run_ai_sandbox};
use std::collections::HashMap;
use std::path::PathBuf;
use tonic::Status;

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_sandbox_run(
        &self,
        annotations: &HashMap<String, String>,
    ) -> Result<String, Status> {
        let image = annotations
            .get("coral.sandbox.image")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "alpine:latest".into());
        let cmd_str = annotations
            .get("coral.sandbox.cmd")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Status::invalid_argument("coral.sandbox.cmd is required"))?;
        let timeout_secs = annotations
            .get("coral.sandbox.timeout")
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(10)
            .clamp(1, 300);
        let memory_mb = annotations
            .get("coral.sandbox.memory_mb")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(64);
        let cpu_percent = annotations
            .get("coral.sandbox.cpu_percent")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(50);
        let pids_limit = annotations
            .get("coral.sandbox.pids_limit")
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(32);
        let allow_network = annotations
            .get("coral.sandbox.allow_network")
            .is_some_and(|v| v == "true" || v == "1");
        let kernel_path = annotations
            .get("coral.sandbox.kernel")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);

        let rootfs_override = {
            let store = self.store.read().await;
            images::find_image(&store.images, &image).map(|i| PathBuf::from(&i.rootfs_path))
        };
        let target_image = rootfs_override
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(image);

        let policy = SandboxPolicy {
            coral_bin: self.cfg.coral_bin.clone(),
            images_dir: self.images_dir.clone(),
            state_root: self.runtime_root(),
            image: target_image,
            cmd: vec!["/bin/sh".into(), "-c".into(), cmd_str],
            envs: Vec::new(),
            memory_mb,
            cpu_percent,
            pids_limit,
            timeout_secs,
            allow_network,
            kernel_path,
            rootfs_override,
        };

        let report = tokio::task::spawn_blocking(move || run_ai_sandbox(&policy))
            .await
            .map_err(|e| Status::internal(format!("join sandbox_run: {e}")))?
            .map_err(|e| Status::internal(format!("sandbox execution failed: {e:#}")))?;

        serde_json::to_string(&report)
            .map_err(|e| Status::internal(format!("serialize sandbox report: {e}")))
    }
}
