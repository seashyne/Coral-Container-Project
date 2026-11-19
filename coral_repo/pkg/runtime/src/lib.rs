//! Coral Core Container Runtime Engine.
//!
//! Provides an OCI-compatible Linux container lifecycle (`create`, `start`, `run`, `state`,
//! `list`, `kill`, `delete`) supporting both privileged root execution and unprivileged
//! rootless execution via Linux User Namespaces.

pub mod container;
pub mod exec;
pub mod namespace;
pub mod overlay;
pub mod pty;
pub mod rootfs;
pub mod security;
pub mod store;

pub use container::{ContainerExit, ContainerManager, RunOptions};
pub use exec::ExecOptions;
pub use store::{StateStore, default_state_root};

use coral_cgroups::CgroupError;
use coral_oci::OciError;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("OCI specification error: {0}")]
    Oci(#[from] OciError),
    #[error("cgroup error: {0}")]
    Cgroup(#[from] CgroupError),
    #[error("system error ({context}): {source}")]
    Sys {
        context: String,
        #[source]
        source: io::Error,
    },
    #[error("container '{0}' already exists")]
    AlreadyExists(String),
    #[error("container '{0}' not found")]
    NotFound(String),
    #[error("invalid container state for '{id}': {message}")]
    InvalidState { id: String, message: String },
    #[error("container initialization failed: {0}")]
    InitFailed(String),
}

impl RuntimeError {
    pub(crate) fn sys(context: impl Into<String>, source: io::Error) -> Self {
        Self::Sys {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn nix(context: impl Into<String>, err: nix::Error) -> Self {
        Self::Sys {
            context: context.into(),
            source: io::Error::from_raw_os_error(err as i32),
        }
    }
}
