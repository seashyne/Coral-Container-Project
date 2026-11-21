//! Unit tests for Cgroups v2 limit application, stats reads, and cleanup.

use super::*;
use coral_oci::{LinuxCpu, LinuxMemory, LinuxPids};
use std::fs;
use tempfile::tempdir;

#[test]
fn converts_oci_shares_to_v2_weight() {
    assert_eq!(oci_shares_to_v2_weight(2), 1);
    assert_eq!(oci_shares_to_v2_weight(1024), 39);
    assert_eq!(oci_shares_to_v2_weight(262_144), 10_000);
}

#[test]
fn applies_limits_reads_stats_and_cleans_up_in_mock_cgroupfs() {
    let dir = tempdir().unwrap();
    let mgr = CgroupManager::with_root(dir.path());

    let resources = LinuxResources {
        memory: Some(LinuxMemory {
            limit: Some(104_857_600),
            reservation: Some(67_108_864),
            swap: None,
        }),
        cpu: Some(LinuxCpu {
            shares: Some(1024),
            quota: Some(50_000),
            period: Some(100_000),
        }),
        pids: Some(LinuxPids { limit: 64 }),
    };
    let cfg = CgroupConfig::from_oci(&resources);

    let cg_path = mgr.apply("test1", 4242, &cfg).unwrap();
    assert_eq!(
        fs::read_to_string(cg_path.join("memory.max")).unwrap(),
        "104857600"
    );
    assert_eq!(
        fs::read_to_string(cg_path.join("memory.high")).unwrap(),
        "67108864"
    );
    assert_eq!(
        fs::read_to_string(cg_path.join("cpu.max")).unwrap(),
        "50000 100000"
    );
    assert_eq!(fs::read_to_string(cg_path.join("cpu.weight")).unwrap(), "39");
    assert_eq!(fs::read_to_string(cg_path.join("pids.max")).unwrap(), "64");
    assert_eq!(
        fs::read_to_string(cg_path.join("cgroup.procs")).unwrap(),
        "4242"
    );

    // Simulate kernel-populated stats files
    fs::write(cg_path.join("memory.current"), "8388608\n").unwrap();
    fs::write(cg_path.join("pids.current"), "3\n").unwrap();
    fs::write(
        cg_path.join("cpu.stat"),
        "usage_usec 123456\nuser_usec 100000\nsystem_usec 23456\n",
    )
    .unwrap();

    let stats = mgr.read_stats("test1").unwrap();
    assert_eq!(stats.memory_current_bytes, Some(8_388_608));
    assert_eq!(stats.pids_current, Some(3));
    assert_eq!(stats.cpu_usage_usec, Some(123_456));

    // Remove mock files and clean up directory
    for entry in fs::read_dir(&cg_path).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    mgr.cleanup("test1").unwrap();
    assert!(!cg_path.exists());
}

#[test]
fn parses_proc_self_cgroup_v2_line() {
    let sample = "0::/user.slice/user-1000.slice/session-1.scope\n";
    assert_eq!(
        fs_io::parse_proc_cgroup_v2(sample).as_deref(),
        Some("/user.slice/user-1000.slice/session-1.scope")
    );
}
