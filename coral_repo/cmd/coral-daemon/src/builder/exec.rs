//! Shared execution context for individual Dockerfile build steps.

use super::build::BuiltStage;
use crate::builder::BuildOptions;
use std::collections::HashMap;
use std::path::Path;

/// Read-only view of the build state that an individual Dockerfile step needs.
pub(super) struct StepExec<'a, 'b> {
    pub staging_dir: &'a Path,
    pub staging_rootfs: &'a Path,
    pub opts: &'a BuildOptions<'b>,
    pub cache_root: &'a Path,
    pub stages: &'a [BuiltStage],
    pub stage_names: &'a HashMap<String, usize>,
    pub active_stage_name: Option<&'a str>,
    pub workdir: &'a str,
    pub envs: &'a [String],
    pub ts: i64,
}

/// Per-instruction metadata for a single Dockerfile step.
pub(super) struct StepInfo<'a> {
    pub verb: &'a str,
    pub arg: &'a str,
    pub step_num: usize,
    pub total_steps: usize,
    pub step_idx: usize,
}
