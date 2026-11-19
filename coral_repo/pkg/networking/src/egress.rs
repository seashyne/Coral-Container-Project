//! Rootless user-mode outbound internet egress integration (`pasta` / `slirp4netns`)
//! for isolated `CLONE_NEWNET` bridge containers.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressBackend {
    Pasta,
    Slirp4netns,
}

impl EgressBackend {
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Pasta => "pasta",
            Self::Slirp4netns => "slirp4netns",
        }
    }
}

/// Detects whether `pasta` (passt) or `slirp4netns` is installed in `$PATH` on the Linux host.
pub fn detect_egress_backend() -> Option<(EgressBackend, PathBuf)> {
    let path_var = std::env::var_os("PATH")?;
    for backend in [EgressBackend::Pasta, EgressBackend::Slirp4netns] {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join(backend.binary_name());
            if candidate.is_file() {
                return Some((backend, candidate));
            }
        }
    }
    None
}

/// RAII handle owning a rootless egress helper process (`pasta` or `slirp4netns`) attached
/// to a container's isolated network namespace. Terminates cleanly on drop.
pub struct RootlessEgressGuard {
    pub backend: EgressBackend,
    child: Child,
}

impl RootlessEgressGuard {
    /// Attaches a user-mode virtual network interface (`tap0`) into `container_pid`'s
    /// isolated `CLONE_NEWNET` if `pasta` or `slirp4netns` is installed.
    /// Returns `None` gracefully when no helper binary is present on the host.
    pub fn attach_best_effort(container_pid: i32) -> Option<Self> {
        let (backend, bin) = detect_egress_backend()?;
        let mut cmd = Command::new(bin);
        match backend {
            EgressBackend::Pasta => {
                cmd.arg("--config-net")
                    .arg("--foreground")
                    .arg("--quiet")
                    .arg(container_pid.to_string());
            }
            EgressBackend::Slirp4netns => {
                cmd.arg("--configure")
                    .arg("--mtu=65520")
                    .arg("--disable-host-loopback")
                    .arg(container_pid.to_string())
                    .arg("tap0");
            }
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = cmd.spawn().ok()?;
        Some(Self { backend, child })
    }
}

impl Drop for RootlessEgressGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
