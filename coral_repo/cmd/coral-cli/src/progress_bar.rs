//! Terminal progress bar for `coral pull` / `coral run` image downloads.
//!
//! `coral-daemon` writes `<state_dir>/progress/<pull_id>.json` snapshots while it streams registry
//! layers (see `coral_daemon::images::progress`). This module tails that file on the CLI side and
//! renders a live bar — no CRI protobuf change, so `PullImageRequest`/`PullImageResponse` stay
//! wire-compatible with upstream `kubelet`/`crictl`.

use coral_daemon::images::progress::{PullPhase, PullProgress, render_bar, render_line};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Renders the pull bar on `stderr` while a `PullImage` RPC is in flight.
pub struct ProgressFollower {
    path: PathBuf,
    state_dir: PathBuf,
    enabled: bool,
    json: bool,
    tty: bool,
    started: Instant,
    last_render: Option<Instant>,
    last_percent: Option<u8>,
    last_phase: Option<PullPhase>,
    drew_anything: bool,
}

impl ProgressFollower {
    /// Creates a follower for `pull_id`. `json` mirrors every snapshot to `stderr` as JSON lines
    /// instead of drawing a bar (for CI and the Desktop GUI).
    pub fn new(state_dir: &Path, pull_id: &str, json: bool) -> Self {
        let state_dir = state_dir.to_path_buf();
        // `json` and the bar are mutually exclusive renderers, but both are "enabled" in the
        // sense that they emit something; `enabled` means "a bar may be drawn".
        Self {
            path: state_dir.join("progress").join(format!("{pull_id}.json")),
            state_dir,
            enabled: !json,
            json,
            tty: std::io::stderr().is_terminal(),
            started: Instant::now(),
            last_render: None,
            last_percent: None,
            last_phase: None,
            drew_anything: false,
        }
    }

    fn read_snapshot(&self) -> Option<PullProgress> {
        let raw = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Polls the snapshot file once; returns `true` while the pull is still in progress.
    pub fn tick(&mut self) -> bool {
        let Some(p) = self.read_snapshot() else {
            return true;
        };
        if p.phase.is_terminal() {
            self.render(&p, true);
            return false;
        }
        self.render(&p, false);
        true
    }

    fn render(&mut self, p: &PullProgress, force: bool) {
        // JSON mode: emit one line per distinct snapshot, no throttling and no bar.
        if self.json {
            if let Ok(line) = serde_json::to_string(p)
                && writeln!(std::io::stderr().lock(), "{line}").is_ok()
            {
                self.drew_anything = true;
            }
            return;
        }
        if !self.enabled {
            return;
        }
        // Throttle redraws: 100 ms on a TTY, once per 2% progress when piped.
        let due = match self.last_render {
            None => true,
            Some(t) if self.tty => t.elapsed() >= Duration::from_millis(100),
            Some(_) => {
                self.last_percent
                    .is_none_or(|prev| p.percent.saturating_sub(prev) >= 2)
                    || self.last_phase != Some(p.phase)
            }
        };
        if !due && !force {
            return;
        }
        self.last_render = Some(Instant::now());
        self.last_percent = Some(p.percent);
        self.last_phase = Some(p.phase);
        self.drew_anything = true;

        let line = render_line(p);
        let mut err = std::io::stderr().lock();
        if self.tty {
            // `\r` + `\x1b[K` redraws in place without scrolling the terminal.
            let _ = write!(err, "\r{line}\x1b[K");
        } else if force || p.percent != 0 {
            let _ = writeln!(err, "{line}");
        }
        let _ = err.flush();
    }

    /// Reads the `message` field of the snapshot left behind by a failed pull.
    pub fn failure_reason(&self) -> Option<String> {
        let mut snap = self.read_snapshot()?;
        snap.message = snap.message.trim().to_string();
        (!snap.message.is_empty()).then_some(snap.message)
    }

    /// Clears the in-place bar and prints a final summary line after the RPC completes.
    pub fn finish(&mut self, image: &str, ok: bool) {
        if self.enabled && self.tty && self.drew_anything {
            let mut err = std::io::stderr().lock();
            let _ = write!(err, "\r\x1b[K");
            let _ = err.flush();
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        if ok {
            if !self.drew_anything || !self.enabled {
                eprintln!("Pulled {image} in {elapsed:.1}s");
            } else {
                eprintln!(
                    "{} 100%  {image} ready in {elapsed:.1}s",
                    render_bar(100, 28)
                );
            }
        }
        // Remove any snapshot the daemon could not clean up (crash/abort path).
        let _ = std::fs::remove_file(&self.path);
        if let Some(stem) = self.path.file_stem().and_then(|s| s.to_str()) {
            let _ = std::fs::remove_file(
                self.state_dir
                    .join("progress")
                    .join(format!("{stem}.json.tmp")),
            );
        }
    }
}

/// Unique, filesystem-safe pull identifier (`<pid>-<nanos>-<seq>`).
pub fn next_pull_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}-{n}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coral_daemon::images::progress::PullProgressReporter;

    #[test]
    fn pull_ids_are_unique_and_filesystem_safe() {
        let a = next_pull_id();
        let b = next_pull_id();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn follower_returns_false_once_pull_reaches_a_terminal_phase() {
        let dir = tempfile::tempdir().unwrap();
        let id = next_pull_id();
        let rep = PullProgressReporter::new(dir.path(), &id, "alpine:3.20");
        rep.set_layers_total(1);
        rep.set_bytes_total(1024);
        rep.add_bytes(1024);
        rep.finish_layer();
        rep.flush();

        let mut f = ProgressFollower::new(dir.path(), &id, false);
        assert!(
            f.tick(),
            "a non-terminal snapshot must keep the follower alive"
        );

        rep.set_phase(PullPhase::Done);
        rep.flush();
        assert!(!f.tick(), "a `done` snapshot must stop the follower");
        f.finish("alpine:3.20", true);
        assert!(
            !dir.path()
                .join("progress")
                .join(format!("{id}.json"))
                .exists()
        );
    }

    #[test]
    fn follower_tolerates_a_missing_snapshot_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = ProgressFollower::new(dir.path(), "absent", false);
        assert!(f.tick(), "a missing snapshot must not abort the pull");
    }

    #[test]
    fn json_mode_emits_lines_instead_of_a_bar() {
        let dir = tempfile::tempdir().unwrap();
        let id = "json-mode";
        let rep = PullProgressReporter::new(dir.path(), id, "busybox:musl");
        rep.set_phase(PullPhase::Downloading);
        rep.flush();
        let mut f = ProgressFollower::new(dir.path(), id, true);
        assert!(
            f.tick(),
            "a downloading snapshot must keep the follower alive"
        );
        assert!(f.drew_anything, "json mode must emit a snapshot line");
        assert!(!f.enabled, "json mode must never draw the bar");
    }

    #[test]
    fn none_mode_emits_nothing() {
        // `ProgressMode::None` never constructs a follower, so this guards the invariant that a
        // disabled follower stays silent: with `json == false` and `enabled == false` there is
        // no renderer at all.
        let mut f = ProgressFollower::new(std::path::Path::new("/nonexistent"), "x", false);
        f.enabled = false;
        let mut p = PullProgress::new("x", "img");
        p.phase = PullPhase::Downloading;
        p.percent = 40;
        f.render(&p, true);
        assert!(!f.drew_anything);
    }
}
