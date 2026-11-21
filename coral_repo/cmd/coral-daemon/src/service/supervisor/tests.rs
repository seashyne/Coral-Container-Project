use super::log_writer::{LogRotationPolicy, RotatingLogWriter, rotated_log_path};
use super::policy::RestartPolicy;

#[test]
fn evaluates_restart_policies_correctly() {
    assert!(!RestartPolicy::parse("no").should_restart(1, 0, false));
    assert!(RestartPolicy::parse("always").should_restart(0, 5, false));
    assert!(!RestartPolicy::parse("always").should_restart(0, 0, true));
    assert!(RestartPolicy::parse("unless-stopped").should_restart(0, 1, false));
    assert!(!RestartPolicy::parse("unless-stopped").should_restart(0, 1, true));
    assert!(!RestartPolicy::parse("on-failure:3").should_restart(0, 0, false));
    assert!(RestartPolicy::parse("on-failure:3").should_restart(1, 2, false));
    assert!(!RestartPolicy::parse("on-failure:3").should_restart(1, 3, false));
}

#[test]
fn rotating_log_writer_rotates_and_caps_archive_count() {
    let dir = tempfile::tempdir().unwrap();
    let log_path = dir.path().join("container.log");
    let policy = LogRotationPolicy {
        max_size_bytes: 20,
        max_files: 2,
    };
    let mut w = RotatingLogWriter::open(log_path.clone(), policy).unwrap();
    for i in 0..6 {
        w.write_chunk(format!("line-{i:02}-payload!!\n").as_bytes())
            .unwrap();
    }
    assert!(w.rotations() >= 4);
    assert!(log_path.exists());
    assert!(rotated_log_path(&log_path, 1).exists());
    assert!(rotated_log_path(&log_path, 2).exists());
    assert!(!rotated_log_path(&log_path, 3).exists());
    assert!(std::fs::metadata(&log_path).unwrap().len() <= 20);
}
