//! Daemon-level housekeeping (`system df`, `system prune`), remote/WSL2-compatible
//! Named Volume management (`volume ls/create/rm`), and Boot Auto-Resume for
//! `--restart always` / `--restart unless-stopped` containers.

mod actions;
mod autogc;
mod df;
mod model;
mod prune;
mod resume;
#[cfg(test)]
mod tests;
mod volumes;

pub use model::{AutoGcReport, PruneReport, SystemDiskUsage, VolumeSummary};
