//! Compose stack data model: services, dependencies and launch specs.

use coral_api::{ContainerConfig, PodSandboxConfig};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Condition under which a `depends_on` dependency satisfies startup readiness.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DependsCondition {
    #[default]
    ServiceStarted,
    ServiceHealthy,
    ServiceCompletedSuccessfully,
}

/// Dependency edge (`depends_on`) with readiness condition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeDependency {
    pub service: String,
    #[serde(default)]
    pub condition: DependsCondition,
}

/// Parsed multi-container stack specification with services ordered topologically by `depends_on`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeSpec {
    pub project_name: Option<String>,
    pub services: Vec<ComposeService>,
    pub volumes: Vec<String>,
}

/// Ready-to-submit CRI pod + container configuration derived from a `ComposeService`.
pub struct ComposeLaunchSpec {
    pub pod_config: PodSandboxConfig,
    pub container_config: ContainerConfig,
}

/// Configuration for a single service inside a Compose stack.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComposeService {
    pub name: String,
    pub image: Option<String>,
    pub build_context: Option<String>,
    pub dockerfile: Option<String>,
    #[serde(default)]
    pub build_target: Option<String>,
    #[serde(default)]
    pub build_args: Vec<String>,
    #[serde(default)]
    pub entrypoint: Vec<String>,
    pub command: Vec<String>,
    #[serde(default)]
    pub env_files: Vec<String>,
    pub envs: Vec<String>,
    pub ports: Vec<String>,
    #[serde(default)]
    pub expose: Vec<String>,
    pub volumes: Vec<String>,
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub depends_on_conditions: Vec<ComposeDependency>,
    pub network: String,
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub network_aliases: Vec<String>,
    #[serde(default)]
    pub devices: Vec<String>,
    #[serde(default)]
    pub gpus: Option<String>,
    #[serde(default)]
    pub log_driver: Option<String>,
    pub dns: Vec<String>,
    pub extra_hosts: Vec<String>,
    pub working_dir: String,
    pub hostname: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub privileged: bool,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub restart: String,
    pub health_cmd: Option<String>,
    pub health_interval_ms: u64,
    pub health_retries: u32,
}

impl Default for ComposeService {
    fn default() -> Self {
        Self {
            name: String::new(),
            image: None,
            build_context: None,
            dockerfile: None,
            build_target: None,
            build_args: Vec::new(),
            entrypoint: Vec::new(),
            command: Vec::new(),
            env_files: Vec::new(),
            envs: Vec::new(),
            ports: Vec::new(),
            expose: Vec::new(),
            volumes: Vec::new(),
            depends_on: Vec::new(),
            depends_on_conditions: Vec::new(),
            network: "host".into(),
            networks: Vec::new(),
            network_aliases: Vec::new(),
            devices: Vec::new(),
            gpus: None,
            log_driver: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            working_dir: "/".into(),
            hostname: None,
            user: None,
            labels: HashMap::new(),
            profiles: Vec::new(),
            privileged: false,
            memory_mb: 0,
            cpu_percent: 0,
            pids_limit: 0,
            restart: "no".into(),
            health_cmd: None,
            health_interval_ms: 5_000,
            health_retries: 3,
        }
    }
}
