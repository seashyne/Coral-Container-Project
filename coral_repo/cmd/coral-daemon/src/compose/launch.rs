//! Conversion of a Compose service into CRI pod + container configurations.

use super::model::{ComposeLaunchSpec, ComposeService};
use coral_api::{
    ContainerConfig, ContainerMetadata, KeyValue, LinuxContainerConfig, LinuxContainerResources,
    PodSandboxConfig, PodSandboxMetadata,
};
use std::collections::HashMap;

impl ComposeService {
    /// Builds aligned CRI `PodSandboxConfig` + `ContainerConfig` payloads for this service.
    pub fn to_launch_spec(&self, project: &str, image_name: &str) -> ComposeLaunchSpec {
        let mut labels = self.labels.clone();
        labels.insert("managed-by".into(), "coral-compose".into());
        labels.insert("coral.compose.project".into(), project.to_string());
        labels.insert("coral.compose.service".into(), self.name.clone());

        let pod_config = PodSandboxConfig {
            metadata: Some(PodSandboxMetadata {
                name: format!("{project}-{}-pod", self.name),
                uid: format!("{project}-{}", self.name),
                namespace: project.to_string(),
                attempt: 0,
            }),
            hostname: self.hostname.clone().unwrap_or_else(|| self.name.clone()),
            log_directory: String::new(),
            labels: labels.clone(),
            annotations: HashMap::new(),
        };

        let envs = self
            .envs
            .iter()
            .filter_map(|e| {
                e.split_once('=').map(|(k, v)| KeyValue {
                    key: k.trim().to_string(),
                    value: v.to_string(),
                })
            })
            .filter(|kv| !kv.key.is_empty())
            .collect();

        // Keep `command`/`args` empty unless the service overrides them so image ENTRYPOINT/CMD work natively.
        let (command, args) = if self.command.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            (vec![self.command[0].clone()], self.command[1..].to_vec())
        };

        let mut annotations = HashMap::new();
        if !self.entrypoint.is_empty()
            && let Ok(ep_json) = serde_json::to_string(&self.entrypoint)
        {
            annotations.insert("coral.entrypoint".into(), ep_json);
        }
        if !self.volumes.is_empty() {
            annotations.insert("coral.mounts".into(), self.volumes.join("\n"));
        }
        if !self.ports.is_empty() {
            annotations.insert("coral.ports".into(), self.ports.join(","));
        }
        if !self.network.trim().is_empty() {
            annotations.insert("coral.network".into(), self.network.trim().to_string());
        }
        if !self.dns.is_empty() {
            annotations.insert("coral.dns".into(), self.dns.join(","));
        }
        if !self.extra_hosts.is_empty() {
            annotations.insert("coral.extra_hosts".into(), self.extra_hosts.join("\n"));
        }
        if let Some(u) = &self.user
            && !u.trim().is_empty()
        {
            annotations.insert("coral.user".into(), u.trim().to_string());
        }
        if self.privileged {
            annotations.insert("coral.privileged".into(), "true".into());
        }
        if !self.restart.trim().is_empty() && !self.restart.eq_ignore_ascii_case("no") {
            annotations.insert("coral.restart".into(), self.restart.trim().to_string());
        }
        if let Some(hc) = &self.health_cmd
            && !hc.trim().is_empty()
        {
            annotations.insert("coral.health.cmd".into(), hc.trim().to_string());
            annotations.insert(
                "coral.health.interval_ms".into(),
                self.health_interval_ms.to_string(),
            );
            annotations.insert(
                "coral.health.retries".into(),
                self.health_retries.to_string(),
            );
        }

        let linux = if self.memory_mb > 0 || self.cpu_percent > 0 || self.pids_limit > 0 {
            Some(LinuxContainerConfig {
                resources: Some(LinuxContainerResources {
                    cpu_period: if self.cpu_percent > 0 { 100_000 } else { 0 },
                    cpu_quota: self.cpu_percent * 1_000,
                    cpu_shares: if self.cpu_percent > 0 { 1024 } else { 0 },
                    memory_limit_in_bytes: self.memory_mb * 1024 * 1024,
                    pids_limit: self.pids_limit,
                }),
            })
        } else {
            None
        };

        let container_config = ContainerConfig {
            metadata: Some(ContainerMetadata {
                name: format!("{project}_{}", self.name),
                attempt: 0,
            }),
            image: Some(ImageSpecShim {
                image: image_name.to_string(),
            }
            .into()),
            command,
            args,
            working_dir: self.working_dir.clone(),
            envs,
            labels,
            annotations,
            log_path: String::new(),
            linux,
        };

        ComposeLaunchSpec {
            pod_config,
            container_config,
        }
    }
}

struct ImageSpecShim {
    image: String,
}

impl From<ImageSpecShim> for coral_api::ImageSpec {
    fn from(s: ImageSpecShim) -> Self {
        Self {
            image: s.image,
            annotations: HashMap::new(),
        }
    }
}
