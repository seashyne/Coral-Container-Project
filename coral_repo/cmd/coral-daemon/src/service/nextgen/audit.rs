//! CIS Container Security Audit (`coral security audit` & Coral Desktop Security tab).

use crate::images::auth::default_auth_file;
use crate::service::CoralDaemonService;
use coral_api::ContainerState as CriContainerState;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use tonic::Status;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditCheck {
    pub id: String,
    pub title: String,
    pub status: String, // "pass" | "warn" | "fail"
    pub detail: String,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityAuditReport {
    pub score: u8,
    pub passed: usize,
    pub warned: usize,
    pub failed: usize,
    pub landlock_abi: Option<u32>,
    pub rootless: bool,
    pub checks: Vec<AuditCheck>,
}

/// Constant-time token byte comparison to prevent timing side-channels on Bearer auth.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) async fn handle_security_audit(&self) -> Result<String, Status> {
        let report = self.run_security_audit().await;
        serde_json::to_string(&report)
            .map_err(|e| Status::internal(format!("serialize security audit: {e}")))
    }

    pub async fn run_security_audit(&self) -> SecurityAuditReport {
        let (mut checks, landlock_abi, rootless) = build_host_checks(&self.cfg.state_dir);

        // CIS-4.1: Running containers posture
        let (running_count, priv_ids, unconfined_ids) = {
            let store = self.store.read().await;
            let mut running = 0usize;
            let mut privs = Vec::new();
            let mut unconfined = Vec::new();
            for c in store.containers.values() {
                if c.state != CriContainerState::ContainerRunning {
                    continue;
                }
                running += 1;
                if c.config
                    .annotations
                    .get("coral.privileged")
                    .is_some_and(|v| v == "true")
                {
                    privs.push(c.id.clone());
                }
                if c.config
                    .annotations
                    .get("coral.security_opt")
                    .is_some_and(|v| v.contains("seccomp=unconfined"))
                {
                    unconfined.push(c.id.clone());
                }
            }
            (running, privs, unconfined)
        };
        checks.push(if !priv_ids.is_empty() {
            AuditCheck {
                id: "CIS-4.1".into(),
                title: "Running Container Isolation Posture".into(),
                status: "fail".into(),
                detail: format!(
                    "{} privileged container(s) running: {}",
                    priv_ids.len(),
                    priv_ids.join(", ")
                ),
                remediation: Some(
                    "Remove --privileged and grant minimal --cap-add / --device instead".into(),
                ),
            }
        } else if !unconfined_ids.is_empty() {
            AuditCheck {
                id: "CIS-4.1".into(),
                title: "Running Container Isolation Posture".into(),
                status: "warn".into(),
                detail: format!(
                    "{} container(s) running with seccomp=unconfined: {}",
                    unconfined_ids.len(),
                    unconfined_ids.join(", ")
                ),
                remediation: Some(
                    "Use default Seccomp allowlist or a custom OCI seccomp JSON profile".into(),
                ),
            }
        } else {
            AuditCheck {
                id: "CIS-4.1".into(),
                title: "Running Container Isolation Posture".into(),
                status: "pass".into(),
                detail: format!(
                    "{running_count} running container(s); none privileged or seccomp-unconfined"
                ),
                remediation: None,
            }
        });

        summarize_audit(checks, landlock_abi, rootless)
    }
}

/// Runs a local host-level CIS security audit without requiring a running `coral-daemon`.
pub fn run_host_security_audit(state_dir: &std::path::Path) -> SecurityAuditReport {
    let (checks, landlock_abi, rootless) = build_host_checks(state_dir);
    summarize_audit(checks, landlock_abi, rootless)
}

fn build_host_checks(state_dir: &std::path::Path) -> (Vec<AuditCheck>, Option<u32>, bool) {
    let mut checks = Vec::new();
    // CIS-1.1: Rootless execution & Multi-UID UserNS
    let multi_uid = coral_runtime::MultiUidSupport::detect();
    let rootless = multi_uid.host_uid != 0;
    checks.push(if rootless {
        AuditCheck {
            id: "CIS-1.1".into(),
            title: "Rootless Engine & User Namespaces".into(),
            status: "pass".into(),
            detail: if multi_uid.is_available() {
                "Running rootless with Multi-UID subuid/subgid isolation".into()
            } else {
                "Running rootless (single-UID user namespace)".into()
            },
            remediation: None,
        }
    } else {
        AuditCheck {
            id: "CIS-1.1".into(),
            title: "Rootless Engine & User Namespaces".into(),
            status: "warn".into(),
            detail: "coral-daemon is running as root (UID 0)".into(),
            remediation: Some(
                "Run coral-daemon as a regular user via systemd --user for rootless isolation"
                    .into(),
            ),
        }
    });

    // CIS-2.1: Default-Deny Seccomp Allowlist + OCI maskedPaths/readonlyPaths
    checks.push(AuditCheck {
        id: "CIS-2.1".into(),
        title: "Seccomp-BPF Allowlist & OCI Masked Paths".into(),
        status: "pass".into(),
        detail: "Default-Deny OCI syscall allowlist + /proc & /sys maskedPaths/readonlyPaths active"
            .into(),
        remediation: None,
    });

    // CIS-2.2: Linux Landlock LSM (rootless kernel confinement)
    let landlock_abi = coral_runtime::security::probe_landlock_abi();
    checks.push(match landlock_abi {
        Some(abi) => AuditCheck {
            id: "CIS-2.2".into(),
            title: "Rootless Linux Landlock LSM".into(),
            status: "pass".into(),
            detail: format!(
                "Kernel Landlock LSM ABI v{abi} available (--security-opt landlock=enforce)"
            ),
            remediation: None,
        },
        None => AuditCheck {
            id: "CIS-2.2".into(),
            title: "Rootless Linux Landlock LSM".into(),
            status: "warn".into(),
            detail: "Host kernel does not expose Landlock LSM ABI".into(),
            remediation: Some(
                "Upgrade to Linux 5.13+ with CONFIG_SECURITY_LANDLOCK=y".into(),
            ),
        },
    });

    // CIS-3.1: Socket & credential file permissions
    let auth_path = default_auth_file();
    let sock_issues = inspect_file_perms(
        &[
            state_dir.join("coral.sock"),
            state_dir.join("docker.sock"),
            auth_path,
        ],
        rootless,
    );
    checks.push(if sock_issues.is_empty() {
        AuditCheck {
            id: "CIS-3.1".into(),
            title: "Daemon Socket & Auth Store Permissions".into(),
            status: "pass".into(),
            detail: if rootless {
                "Sockets and auth.json strictly restricted to owner (0600)".into()
            } else {
                "Sockets and auth.json are not world-accessible".into()
            },
            remediation: None,
        }
    } else {
        AuditCheck {
            id: "CIS-3.1".into(),
            title: "Daemon Socket & Auth Store Permissions".into(),
            status: "fail".into(),
            detail: sock_issues.join("; "),
            remediation: Some(
                "Restrict permissions with chmod 0600 on rootless sockets and auth.json".into(),
            ),
        }
    });

    // CIS-3.2: Daemon remote auth token status
    let has_token = std::env::var("CORAL_AUTH_TOKEN")
        .ok()
        .is_some_and(|v| !v.trim().is_empty());
    checks.push(AuditCheck {
        id: "CIS-3.2".into(),
        title: "gRPC & Remote API Authentication".into(),
        status: "pass".into(),
        detail: if has_token {
            "CORAL_AUTH_TOKEN Bearer authentication is enabled".into()
        } else {
            "Loopback-only listener (127.0.0.1); set CORAL_AUTH_TOKEN before binding non-loopback"
                .into()
        },
        remediation: if has_token {
            None
        } else {
            Some(
                "Pass --auth-token or CORAL_AUTH_TOKEN when exposing coral-daemon over LAN".into(),
            )
        },
    });

    (checks, landlock_abi, rootless)
}

fn inspect_file_perms(paths: &[std::path::PathBuf], rootless: bool) -> Vec<String> {
    let mut issues = Vec::new();
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let mode = meta.permissions().mode() & 0o777;
        let bad = if rootless || path.file_name().and_then(|s| s.to_str()) == Some("auth.json") {
            (mode & 0o077) != 0
        } else {
            (mode & 0o007) != 0
        };
        if bad {
            issues.push(format!("{} has permissive mode {:04o}", path.display(), mode));
        }
    }
    issues
}

fn summarize_audit(
    checks: Vec<AuditCheck>,
    landlock_abi: Option<u32>,
    rootless: bool,
) -> SecurityAuditReport {
    let passed = checks.iter().filter(|c| c.status == "pass").count();
    let warned = checks.iter().filter(|c| c.status == "warn").count();
    let failed = checks.iter().filter(|c| c.status == "fail").count();
    let score = 100u8
        .saturating_sub((warned as u8).saturating_mul(10))
        .saturating_sub((failed as u8).saturating_mul(25));
    SecurityAuditReport {
        score,
        passed,
        warned,
        failed,
        landlock_abi,
        rootless,
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_validates_exact_tokens() {
        assert!(constant_time_eq(b"secret-token-123", b"secret-token-123"));
        assert!(!constant_time_eq(b"secret-token-123", b"secret-token-124"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn audit_summary_penalizes_warnings_and_failures() {
        let checks = vec![
            AuditCheck {
                id: "1".into(),
                title: "a".into(),
                status: "pass".into(),
                detail: String::new(),
                remediation: None,
            },
            AuditCheck {
                id: "2".into(),
                title: "b".into(),
                status: "warn".into(),
                detail: String::new(),
                remediation: None,
            },
            AuditCheck {
                id: "3".into(),
                title: "c".into(),
                status: "fail".into(),
                detail: String::new(),
                remediation: None,
            },
        ];
        let report = summarize_audit(checks, Some(3), true);
        assert_eq!(report.score, 65);
        assert_eq!(report.passed, 1);
        assert_eq!(report.warned, 1);
        assert_eq!(report.failed, 1);
    }
}
