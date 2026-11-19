use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::spec::{OCI_SPEC_VERSION, OciError};

/// Standard OCI container lifecycle states.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContainerStatus {
    Creating,
    Created,
    Running,
    Stopped,
}

impl std::fmt::Display for ContainerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Creating => "creating",
            Self::Created => "created",
            Self::Running => "running",
            Self::Stopped => "stopped",
        };
        write!(f, "{s}")
    }
}

/// OCI Runtime `state.json` representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContainerState {
    pub oci_version: String,
    pub id: String,
    pub status: ContainerStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    pub bundle: PathBuf,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
    pub created: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroup_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl ContainerState {
    pub fn new(
        id: impl Into<String>,
        bundle: PathBuf,
        annotations: BTreeMap<String, String>,
    ) -> Self {
        Self {
            oci_version: OCI_SPEC_VERSION.into(),
            id: id.into(),
            status: ContainerStatus::Creating,
            pid: None,
            bundle,
            annotations,
            created: Utc::now(),
            cgroup_path: None,
            exit_code: None,
        }
    }
}

/// Validates a container ID so it cannot escape state directories or cgroup paths.
pub fn validate_container_id(id: &str) -> Result<(), OciError> {
    if id.is_empty() || id.len() > 128 {
        return Err(OciError::Validation(
            "container ID must be between 1 and 128 characters".into(),
        ));
    }
    if id.starts_with('.') || id.contains("..") {
        return Err(OciError::Validation(format!(
            "container ID '{id}' must not start with '.' or contain '..'"
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(OciError::Validation(format!(
            "container ID '{id}' contains disallowed characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_container_ids_strictly() {
        assert!(validate_container_id("coral-app_01.v1").is_ok());
        assert!(validate_container_id("").is_err());
        assert!(validate_container_id("../escape").is_err());
        assert!(validate_container_id("a/b").is_err());
        assert!(validate_container_id(".hidden").is_err());
    }
}
