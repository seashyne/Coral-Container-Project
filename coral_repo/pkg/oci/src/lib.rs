//! OCI Runtime Specification v1.0.2 data structures, validation, and state management.

pub mod spec;
pub mod state;

pub use spec::{
    Linux, LinuxCpu, LinuxIdMapping, LinuxMemory, LinuxNamespace, LinuxNamespaceType, LinuxPids,
    LinuxResources, Mount, OciError, OciSpec, Process, Root, User, OCI_SPEC_VERSION,
};
pub use state::{ContainerState, ContainerStatus, validate_container_id};
