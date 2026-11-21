use super::*;
use std::fs;

#[test]
fn renders_bars_at_boundaries() {
    assert_eq!(render_bar(0, 10), "[----------]");
    assert_eq!(render_bar(100, 10), "[##########]");
    assert_eq!(render_bar(50, 10), "[#####-----]");
    // Percentages above 100 are clamped, never overflowed.
    assert_eq!(render_bar(250, 10), "[##########]");
}

#[test]
fn formats_human_bytes() {
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(512), "512 B");
    assert_eq!(human_bytes(1024), "1.0 KiB");
    assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
}

#[test]
fn reporter_writes_atomic_snapshots_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let rep = PullProgressReporter::new(dir.path(), "pull-1", "alpine:3.20");
    rep.set_layers_total(3);
    rep.set_bytes_total(1000);
    rep.begin_layer(1, 400);
    rep.add_bytes(250);
    rep.flush();

    let raw = fs::read_to_string(rep.path()).unwrap();
    let parsed: PullProgress = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed.phase, PullPhase::Downloading);
    assert_eq!(parsed.layers_total, 3);
    assert_eq!(parsed.bytes_downloaded, 250);
    assert_eq!(parsed.percent, 25);
    assert!(!dir.path().join(PROGRESS_DIR).join("pull-1.json.tmp").exists());

    rep.finish_layer();
    rep.set_phase(PullPhase::Done);
    rep.flush();
    let done: PullProgress =
        serde_json::from_str(&fs::read_to_string(rep.path()).unwrap()).unwrap();
    assert_eq!(done.percent, 100);
    assert_eq!(done.layers_done, 1);

    rep.finish();
    assert!(!rep.path().exists());
}

#[test]
fn percent_falls_back_to_layers_when_byte_total_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let rep = PullProgressReporter::new(dir.path(), "pull-2", "busybox:musl");
    rep.set_bytes_total(0);
    rep.set_layers_total(4);
    rep.finish_layer();
    rep.finish_layer();
    assert_eq!(rep.snapshot().percent, 50);
}

#[test]
fn sweep_removes_only_expired_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let rep = PullProgressReporter::new(dir.path(), "stale", "alpine:3.20");
    rep.flush();
    // `ZERO` age sweeps everything; `1h` keeps the freshly written file.
    assert_eq!(PullProgressReporter::sweep_stale(dir.path(), std::time::Duration::from_secs(3600)), 0);
    assert!(rep.path().exists());
    assert_eq!(PullProgressReporter::sweep_stale(dir.path(), std::time::Duration::ZERO), 1);
    assert!(!rep.path().exists());
}

#[test]
fn renders_human_readable_line() {
    let mut p = PullProgress::new("id", "debian:bookworm-slim");
    p.phase = PullPhase::Downloading;
    p.layers_done = 1;
    p.layers_total = 2;
    p.bytes_downloaded = 1024 * 1024;
    p.bytes_total = 4 * 1024 * 1024;
    p.percent = 25;
    p.elapsed_ms = 2000;
    let line = render_line(&p);
    assert!(line.contains("25%"), "{line}");
    assert!(line.contains("1/2 layers"), "{line}");
    assert!(line.contains("1.0 MiB / 4.0 MiB"), "{line}");
    assert!(line.contains("/s"), "{line}");
}
