//! Unit tests for single-UID rootless UserNS mapping adjustment.

use super::*;
use coral_oci::spec::LinuxIdMapping;

#[test]
fn adjust_single_uid_mapping_remaps_non_root_service_user() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("etc")).unwrap();
    std::fs::write(
        dir.path().join("etc/passwd"),
        "root:x:0:0:root:/root:/bin/sh\npostgres:x:70:70:Postgres:/var/lib/postgresql:/bin/sh\n",
    )
    .unwrap();

    let mut spec = OciSpec::default_spec(true, 1000, 1000);
    spec.annotations.insert("coral.user".into(), "postgres".into());
    let mut plan = NamespacePlan {
        flags: nix::sched::CloneFlags::CLONE_NEWUSER,
        use_user_ns: true,
        use_net_ns: false,
        uid_mappings: vec![LinuxIdMapping {
            container_id: 0,
            host_id: 1000,
            size: 1,
        }],
        gid_mappings: vec![LinuxIdMapping {
            container_id: 0,
            host_id: 1000,
            size: 1,
        }],
    };

    adjust_single_uid_mapping(&mut plan, &spec, dir.path());
    assert_eq!(plan.uid_mappings[0].container_id, 70);
    assert_eq!(plan.gid_mappings[0].container_id, 70);

    // Multi-UID subuid mappings remain untouched so container keeps full 0..65536 range.
    plan.uid_mappings = vec![LinuxIdMapping {
        container_id: 0,
        host_id: 100000,
        size: 65536,
    }];
    adjust_single_uid_mapping(&mut plan, &spec, dir.path());
    assert_eq!(plan.uid_mappings[0].container_id, 0);
}
