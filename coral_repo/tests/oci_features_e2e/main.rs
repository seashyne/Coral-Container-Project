//! Production Docker-parity E2E tests: Bind & Named Volumes (`coral volume`), DNS (`--dns`, `--add-host`),
//! Interactive PTY (`-t`), Seccomp-BPF + Cap Drop, TCP Port Forwarding (`-p`), Isolated Bridge NetNS
//! (`--network bridge` Cross-NetNS multi-container port proxy), `coral diff` + `coral commit`, and `coral build`.
//!
//! The suite is split thematically into submodules. Every test lives in exactly one
//! submodule, so the set of executed tests is identical to the previous flat
//! `oci_features_e2e.rs` file.

mod volumes_dns_pty_security;
mod port_forwarding;
mod overlayfs_diff_commit;
mod bridge_network;
mod dockerfile_build;
mod cli_compat;
mod multi_uid_rootless;

use coral_e2e_tests::{alpine_rootfs, ensure_coral_binary};
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;
