//! Core OCI Runtime v1.0.2 E2E tests: `spec`, namespace/rootfs isolation, split `create`/`start`/`state`/`list`/`delete`,
//! Base Image Catalog + `pull --local` + `run --image`, OverlayFS Copy-on-Write isolation, and live `setns()` `exec`.
//!
//! The suite is split thematically into submodules. Every test lives in exactly one
//! submodule, so the set of executed tests is identical to the previous flat
//! `oci_e2e.rs` file.

mod spec_and_isolation;
mod lifecycle;
mod image_catalog;
mod overlayfs_cow;
mod exec_setns;

use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use coral_oci::{ContainerState, ContainerStatus, OciSpec};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;
