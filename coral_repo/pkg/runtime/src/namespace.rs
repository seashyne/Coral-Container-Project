use coral_oci::{LinuxIdMapping, LinuxNamespaceType, OciSpec};
use nix::sched::CloneFlags;

use crate::RuntimeError;

/// Resolved namespace plan for spawning a container process.
#[derive(Debug, Clone)]
pub struct NamespacePlan {
    pub flags: CloneFlags,
    pub use_user_ns: bool,
    pub use_net_ns: bool,
    pub uid_mappings: Vec<LinuxIdMapping>,
    pub gid_mappings: Vec<LinuxIdMapping>,
}

/// Builds the Linux `CloneFlags` and UID/GID mapping plan from an OCI spec and host privileges.
pub fn build_namespace_plan(spec: &OciSpec) -> NamespacePlan {
    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let rootless = host_uid != 0;

    let mut flags = CloneFlags::empty();
    let mut use_user_ns = rootless;
    let mut use_net_ns = false;

    if let Some(linux) = &spec.linux {
        if linux.namespaces.is_empty() {
            flags |= CloneFlags::CLONE_NEWPID
                | CloneFlags::CLONE_NEWUTS
                | CloneFlags::CLONE_NEWNS
                | CloneFlags::CLONE_NEWIPC;
        } else {
            for ns in &linux.namespaces {
                match ns.typ {
                    LinuxNamespaceType::Pid => flags |= CloneFlags::CLONE_NEWPID,
                    LinuxNamespaceType::Uts => flags |= CloneFlags::CLONE_NEWUTS,
                    LinuxNamespaceType::Mount => flags |= CloneFlags::CLONE_NEWNS,
                    LinuxNamespaceType::Ipc => flags |= CloneFlags::CLONE_NEWIPC,
                    LinuxNamespaceType::Network => {
                        flags |= CloneFlags::CLONE_NEWNET;
                        use_net_ns = true;
                    }
                    LinuxNamespaceType::User => {
                        flags |= CloneFlags::CLONE_NEWUSER;
                        use_user_ns = true;
                    }
                    LinuxNamespaceType::Cgroup => flags |= CloneFlags::CLONE_NEWCGROUP,
                }
            }
        }
    } else {
        flags |= CloneFlags::CLONE_NEWPID
            | CloneFlags::CLONE_NEWUTS
            | CloneFlags::CLONE_NEWNS
            | CloneFlags::CLONE_NEWIPC;
    }

    if use_user_ns {
        flags |= CloneFlags::CLONE_NEWUSER;
    }

    let uid_mappings = spec
        .linux
        .as_ref()
        .filter(|l| !l.uid_mappings.is_empty())
        .map(|l| l.uid_mappings.clone())
        .unwrap_or_else(|| {
            vec![LinuxIdMapping {
                container_id: 0,
                host_id: host_uid,
                size: 1,
            }]
        });

    let gid_mappings = spec
        .linux
        .as_ref()
        .filter(|l| !l.gid_mappings.is_empty())
        .map(|l| l.gid_mappings.clone())
        .unwrap_or_else(|| {
            vec![LinuxIdMapping {
                container_id: 0,
                host_id: host_gid,
                size: 1,
            }]
        });

    NamespacePlan {
        flags,
        use_user_ns,
        use_net_ns,
        uid_mappings,
        gid_mappings,
    }
}

/// Expands default single-UID rootless mappings (`[0 -> host_id, size 1]`) into
/// full Multi-UID rootless subordinate ranges (`0 -> host_id, 1..65535 -> subid...`)
/// when `/etc/subuid`, `/etc/subgid`, `newuidmap`, and `newgidmap` are available and
/// `"coral.userns.single_uid"` is not `"true"`. Returns `true` when expanded.
pub fn expand_multi_uid_mappings(plan: &mut NamespacePlan, spec: &OciSpec) -> bool {
    if !plan.use_user_ns {
        return false;
    }
    if spec
        .annotations
        .get("coral.userns.single_uid")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
    {
        return false;
    }

    let support = crate::multi_uid::MultiUidSupport::detect();
    if !support.is_available() {
        return false;
    }

    let default_uid = plan.uid_mappings.len() == 1
        && plan.uid_mappings[0].container_id == 0
        && plan.uid_mappings[0].host_id == support.host_uid
        && plan.uid_mappings[0].size == 1;
    let default_gid = plan.gid_mappings.len() == 1
        && plan.gid_mappings[0].container_id == 0
        && plan.gid_mappings[0].host_id == support.host_gid
        && plan.gid_mappings[0].size == 1;

    if !default_uid || !default_gid {
        return false;
    }

    let expanded_uid = support.uid_mappings();
    let expanded_gid = support.gid_mappings();
    if expanded_uid.len() <= 1 || expanded_gid.len() <= 1 {
        return false;
    }

    plan.uid_mappings = expanded_uid;
    plan.gid_mappings = expanded_gid;
    true
}

/// Writes `/proc/<pid>/setgroups`, `/proc/<pid>/uid_map`, and `/proc/<pid>/gid_map`
/// from the parent process after the child has called `unshare(CLONE_NEWUSER)`, using
/// `newuidmap`/`newgidmap` automatically for Multi-UID subordinate ranges.
pub fn write_userns_mappings(pid: i32, plan: &NamespacePlan) -> Result<(), RuntimeError> {
    if !plan.use_user_ns {
        return Ok(());
    }

    crate::multi_uid::apply_userns_mappings(
        pid,
        &plan.uid_mappings,
        &plan.gid_mappings,
        format_id_mappings,
    )
}

fn format_id_mappings(mappings: &[LinuxIdMapping]) -> String {
    let mut out = String::new();
    for m in mappings {
        out.push_str(&format!("{} {} {}\n", m.container_id, m.host_id, m.size));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_uid_gid_mappings_correctly() {
        let mappings = vec![LinuxIdMapping {
            container_id: 0,
            host_id: 1000,
            size: 1,
        }];
        assert_eq!(format_id_mappings(&mappings), "0 1000 1\n");
    }

    #[test]
    fn default_spec_includes_core_namespaces() {
        let spec = OciSpec::default_spec(true, 1000, 1000);
        let plan = build_namespace_plan(&spec);
        assert!(plan.flags.contains(CloneFlags::CLONE_NEWPID));
        assert!(plan.flags.contains(CloneFlags::CLONE_NEWUTS));
        assert!(plan.flags.contains(CloneFlags::CLONE_NEWNS));
        assert!(plan.flags.contains(CloneFlags::CLONE_NEWUSER));
        assert!(plan.use_user_ns);
    }
}
