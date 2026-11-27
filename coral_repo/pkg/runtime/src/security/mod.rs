//! Container security hardening: Linux Capabilities (`--cap-drop`/`--cap-add`),
//! Default-Deny Seccomp-BPF Allowlist + custom JSON profiles, Rootless Landlock LSM,
//! and optional AppArmor exec profile attachment.

pub mod caps;
pub mod landlock;
pub mod seccomp;
mod seccomp_allowlist;

pub use caps::{DEFAULT_KEEP_CAPS, apply_capset, compute_cap_mask, drop_bounding_set, drop_capabilities_with, parse_cap_bit};
pub use landlock::{LandlockMode, apply_apparmor_profile, apply_landlock, probe_landlock_abi};
pub use seccomp::{SeccompMode, apply_default_seccomp, apply_seccomp};

use crate::RuntimeError;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Parsed per-container security configuration from OCI spec annotations.
#[derive(Debug, Clone, Default)]
pub struct SecurityConfig {
    pub privileged: bool,
    pub no_new_privileges: bool,
    pub cap_drop: Vec<String>,
    pub cap_add: Vec<String>,
    pub seccomp: SeccompMode,
    pub landlock: LandlockMode,
    pub apparmor: Option<String>,
}

impl SecurityConfig {
    /// Parses `coral.privileged`, `coral.cap_drop`, `coral.cap_add`, and `coral.security_opt`
    /// annotations from an OCI spec.
    pub fn from_annotations(
        annotations: &BTreeMap<String, String>,
        spec_no_new_privs: bool,
    ) -> Result<Self, RuntimeError> {
        let privileged = annotations
            .get("coral.privileged")
            .is_some_and(|v| v == "true");
        let mut cfg = Self {
            privileged,
            no_new_privileges: spec_no_new_privs || !privileged,
            cap_drop: parse_csv(annotations.get("coral.cap_drop")),
            cap_add: parse_csv(annotations.get("coral.cap_add")),
            seccomp: if privileged {
                SeccompMode::Unconfined
            } else {
                SeccompMode::DefaultAllowlist
            },
            landlock: LandlockMode::Off,
            apparmor: None,
        };

        for opt in parse_csv(annotations.get("coral.security_opt")) {
            let Some((k, v)) = opt.split_once('=').or_else(|| opt.split_once(':')) else {
                continue;
            };
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            match key.as_str() {
                "seccomp" => {
                    cfg.seccomp = if val.eq_ignore_ascii_case("unconfined") {
                        SeccompMode::Unconfined
                    } else if val.eq_ignore_ascii_case("default") || val.is_empty() {
                        SeccompMode::DefaultAllowlist
                    } else {
                        SeccompMode::CustomProfile(PathBuf::from(val))
                    };
                }
                "no-new-privileges" => {
                    cfg.no_new_privileges = !val.eq_ignore_ascii_case("false");
                }
                "landlock" => {
                    cfg.landlock = if val.eq_ignore_ascii_case("enforce")
                        || val.eq_ignore_ascii_case("strict")
                        || val.eq_ignore_ascii_case("true")
                    {
                        LandlockMode::Enforce
                    } else {
                        LandlockMode::Off
                    };
                }
                "apparmor" => {
                    cfg.apparmor = Some(val.to_string());
                }
                _ => {}
            }
        }
        Ok(cfg)
    }
}

fn parse_csv(raw: Option<&String>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(ToString::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Backward-compatible helper dropping to default OCI capability mask.
pub fn drop_capabilities() -> Result<(), RuntimeError> {
    drop_capabilities_with(&[], &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_security_opt_cap_drop_cap_add_and_landlock_annotations() {
        let mut ann = BTreeMap::new();
        ann.insert("coral.cap_drop".into(), "ALL".into());
        ann.insert("coral.cap_add".into(), "NET_BIND_SERVICE".into());
        ann.insert(
            "coral.security_opt".into(),
            "landlock=enforce,seccomp=/tmp/custom.json,apparmor=docker-default".into(),
        );
        let cfg = SecurityConfig::from_annotations(&ann, true).unwrap();
        assert!(!cfg.privileged);
        assert!(cfg.no_new_privileges);
        assert_eq!(cfg.cap_drop, vec!["ALL"]);
        assert_eq!(cfg.cap_add, vec!["NET_BIND_SERVICE"]);
        assert_eq!(cfg.landlock, LandlockMode::Enforce);
        assert_eq!(cfg.seccomp, SeccompMode::CustomProfile(PathBuf::from("/tmp/custom.json")));
        assert_eq!(cfg.apparmor.as_deref(), Some("docker-default"));
    }
}
