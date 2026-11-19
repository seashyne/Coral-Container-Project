use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use thiserror::Error;

/// OCI Runtime Specification version implemented by Coral v1.
pub const OCI_SPEC_VERSION: &str = "1.0.2";

#[derive(Debug, Error)]
pub enum OciError {
    #[error("I/O error reading/writing OCI file: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid OCI JSON format: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid OCI specification: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OciSpec {
    pub oci_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<Process>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<Root>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<Mount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux: Option<Linux>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub annotations: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Process {
    #[serde(default)]
    pub terminal: bool,
    #[serde(default)]
    pub user: User,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env: Vec<String>,
    pub cwd: String,
    #[serde(default)]
    pub no_new_privileges: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    #[serde(default)]
    pub uid: u32,
    #[serde(default)]
    pub gid: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_gids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Root {
    pub path: String,
    #[serde(default)]
    pub readonly: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub destination: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub typ: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Linux {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uid_mappings: Vec<LinuxIdMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gid_mappings: Vec<LinuxIdMapping>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub namespaces: Vec<LinuxNamespace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<LinuxResources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cgroups_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxIdMapping {
    #[serde(rename = "containerID")]
    pub container_id: u32,
    #[serde(rename = "hostID")]
    pub host_id: u32,
    pub size: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum LinuxNamespaceType {
    Pid,
    Network,
    Mount,
    Ipc,
    Uts,
    User,
    Cgroup,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxNamespace {
    #[serde(rename = "type")]
    pub typ: LinuxNamespaceType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<LinuxMemory>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<LinuxCpu>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pids: Option<LinuxPids>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxMemory {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reservation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxCpu {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shares: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LinuxPids {
    pub limit: i64,
}

impl OciSpec {
    /// Load and validate an OCI `config.json` from disk.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, OciError> {
        let content = fs::read_to_string(path)?;
        let spec: Self = serde_json::from_str(&content)?;
        spec.validate()?;
        Ok(spec)
    }

    /// Serialize and write an OCI `config.json` to disk.
    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), OciError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, format!("{json}\n"))?;
        Ok(())
    }

    /// Generate a standard OCI Runtime Spec v1.0.2 template.
    pub fn default_spec(rootless: bool, host_uid: u32, host_gid: u32) -> Self {
        let mut namespaces = vec![
            LinuxNamespace {
                typ: LinuxNamespaceType::Pid,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Ipc,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Uts,
                path: None,
            },
            LinuxNamespace {
                typ: LinuxNamespaceType::Mount,
                path: None,
            },
        ];

        let (uid_mappings, gid_mappings) = if rootless {
            namespaces.push(LinuxNamespace {
                typ: LinuxNamespaceType::User,
                path: None,
            });
            (
                vec![LinuxIdMapping {
                    container_id: 0,
                    host_id: host_uid,
                    size: 1,
                }],
                vec![LinuxIdMapping {
                    container_id: 0,
                    host_id: host_gid,
                    size: 1,
                }],
            )
        } else {
            (Vec::new(), Vec::new())
        };

        let mounts = vec![
            Mount {
                destination: "/proc".into(),
                typ: Some("proc".into()),
                source: Some("proc".into()),
                options: vec!["nosuid".into(), "noexec".into(), "nodev".into()],
            },
            Mount {
                destination: "/tmp".into(),
                typ: Some("tmpfs".into()),
                source: Some("tmpfs".into()),
                options: vec!["nosuid".into(), "nodev".into(), "mode=1777".into()],
            },
        ];

        Self {
            oci_version: OCI_SPEC_VERSION.into(),
            process: Some(Process {
                terminal: false,
                user: User::default(),
                args: vec!["/bin/sh".into()],
                env: vec![
                    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                    "TERM=xterm".into(),
                ],
                cwd: "/".into(),
                no_new_privileges: true,
            }),
            root: Some(Root {
                path: "rootfs".into(),
                readonly: false,
            }),
            hostname: Some("coral-container".into()),
            mounts,
            linux: Some(Linux {
                uid_mappings,
                gid_mappings,
                namespaces,
                resources: Some(LinuxResources {
                    memory: Some(LinuxMemory {
                        limit: Some(128 * 1024 * 1024), // 128 MiB default
                        reservation: None,
                        swap: None,
                    }),
                    cpu: Some(LinuxCpu {
                        shares: Some(1024),
                        quota: Some(100_000),
                        period: Some(100_000),
                    }),
                    pids: Some(LinuxPids { limit: 128 }),
                }),
                cgroups_path: None,
            }),
            annotations: BTreeMap::new(),
        }
    }

    /// Validate required OCI fields and invariants.
    pub fn validate(&self) -> Result<(), OciError> {
        if self.oci_version.trim().is_empty() {
            return Err(OciError::Validation("ociVersion must not be empty".into()));
        }

        let root = self
            .root
            .as_ref()
            .ok_or_else(|| OciError::Validation("root section is required".into()))?;
        if root.path.trim().is_empty() {
            return Err(OciError::Validation("root.path must not be empty".into()));
        }

        let process = self
            .process
            .as_ref()
            .ok_or_else(|| OciError::Validation("process section is required".into()))?;
        if process.args.is_empty() || process.args[0].trim().is_empty() {
            return Err(OciError::Validation(
                "process.args must contain at least one executable".into(),
            ));
        }
        if !process.cwd.starts_with('/') {
            return Err(OciError::Validation(format!(
                "process.cwd must be an absolute path inside the container, got '{}'",
                process.cwd
            )));
        }

        if let Some(linux) = &self.linux
            && let Some(resources) = &linux.resources
        {
            if let Some(mem) = &resources.memory
                && let Some(limit) = mem.limit
                && limit <= 0
            {
                return Err(OciError::Validation(
                    "linux.resources.memory.limit must be > 0".into(),
                ));
            }
            if let Some(pids) = &resources.pids
                && pids.limit <= 0
            {
                return Err(OciError::Validation(
                    "linux.resources.pids.limit must be > 0".into(),
                ));
            }
        }

        Ok(())
    }

    /// Convenience helper returning whether a given namespace type is requested.
    pub fn has_namespace(&self, ns_type: LinuxNamespaceType) -> bool {
        self.linux
            .as_ref()
            .map(|l| l.namespaces.iter().any(|ns| ns.typ == ns_type))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn default_rootless_spec_is_valid_and_roundtrips() {
        let spec = OciSpec::default_spec(true, 1000, 1000);
        spec.validate().expect("default rootless spec must be valid");
        assert!(spec.has_namespace(LinuxNamespaceType::User));
        assert!(spec.has_namespace(LinuxNamespaceType::Pid));

        let tmp = NamedTempFile::new().unwrap();
        spec.to_file(tmp.path()).unwrap();
        let loaded = OciSpec::from_file(tmp.path()).unwrap();
        assert_eq!(spec, loaded);
    }

    #[test]
    fn rejects_relative_cwd_and_empty_args() {
        let mut spec = OciSpec::default_spec(false, 0, 0);
        spec.process.as_mut().unwrap().cwd = "relative/path".into();
        assert!(matches!(spec.validate(), Err(OciError::Validation(_))));

        let mut spec2 = OciSpec::default_spec(false, 0, 0);
        spec2.process.as_mut().unwrap().args.clear();
        assert!(matches!(spec2.validate(), Err(OciError::Validation(_))));
    }
}
