use coral_oci::{LinuxIdMapping, LinuxNamespaceType, OciSpec};
use nix::sched::CloneFlags;
use std::fs;
use std::path::PathBuf;

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

/// Writes `/proc/<pid>/setgroups`, `/proc/<pid>/uid_map`, and `/proc/<pid>/gid_map`
/// from the parent process after the child has called `unshare(CLONE_NEWUSER)`.
pub fn write_userns_mappings(pid: i32, plan: &NamespacePlan) -> Result<(), RuntimeError> {
    if !plan.use_user_ns {
        return Ok(());
    }

    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let is_root = unsafe { libc::geteuid() } == 0;

    // Unprivileged kernels require writing "deny" to setgroups before writing gid_map.
    if !is_root {
        fs::write(proc_dir.join("setgroups"), "deny")
            .map_err(|e| RuntimeError::sys("write /proc/<pid>/setgroups", e))?;
    }

    let uid_map = format_id_mappings(&plan.uid_mappings);
    fs::write(proc_dir.join("uid_map"), uid_map)
        .map_err(|e| RuntimeError::sys("write /proc/<pid>/uid_map", e))?;

    let gid_map = format_id_mappings(&plan.gid_mappings);
    fs::write(proc_dir.join("gid_map"), gid_map)
        .map_err(|e| RuntimeError::sys("write /proc/<pid>/gid_map", e))?;

    Ok(())
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
