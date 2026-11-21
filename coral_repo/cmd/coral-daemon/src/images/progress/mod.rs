//! Live OCI image pull progress reporting (`coral pull --progress=json`).
//!
//! `coral-daemon` publishes phase/byte counters to a JSON file inside `<state_dir>/progress/`
//! while `pull_image` streams registry layers. The `coral` CLI tails that file and renders a
//! terminal progress bar — no CRI contract change is required, so `PullImageResponse` stays
//! binary-compatible with upstream `kubelet`/`crictl`.

mod model;
mod render;
mod reporter;
#[cfg(test)]
mod tests;

pub use model::{PullPhase, PullProgress};
pub use render::{human_bytes, render_bar, render_line};
pub use reporter::PullProgressReporter;

/// Directory inside `<state_dir>` holding one `<pull_id>.json` per active pull.
pub const PROGRESS_DIR: &str = "progress";
