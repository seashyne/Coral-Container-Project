//! Image build entry point: runs the Dockerfile steps and installs the built rootfs.

use super::dockerfile::parse_dockerfile;
use super::steps::execute_build_steps;
use crate::images::{self, OciImageConfig, StoredImage};
use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Options for building a container image from a `Dockerfile`.
pub struct BuildOptions<'a> {
    pub tag: &'a str,
    pub dockerfile_content: &'a str,
    pub context_dir: Option<&'a Path>,
    pub images_dir: &'a Path,
    pub state_dir: &'a Path,
    pub coral_bin: &'a Path,
    pub no_cache: bool,
    pub target: Option<&'a str>,
    pub build_args: &'a [String],
}

pub(super) struct BuiltStage {
    pub(super) rootfs_dir: PathBuf,
    pub(super) chain_key: String,
    pub(super) envs: Vec<String>,
    pub(super) workdir: String,
    pub(super) img_cfg: OciImageConfig,
}

/// Builds a new image from a `Dockerfile`, persists it into `images_dir/images.json`,
/// and returns its `StoredImage` record.
pub async fn build_image(opts: BuildOptions<'_>) -> Result<StoredImage> {
    let tag = opts.tag.trim();
    if tag.is_empty() {
        bail!("target image tag must not be empty");
    }
    let display_tag = if tag.contains(':') || tag == "scratch" {
        tag.to_string()
    } else {
        format!("{tag}:latest")
    };

    let instructions = parse_dockerfile(opts.dockerfile_content)?;
    if instructions.is_empty() {
        bail!("Dockerfile contains no instructions");
    }

    let ts = chrono::Utc::now().timestamp_millis();
    let staging_dir = opts
        .images_dir
        .join(".tmp")
        .join(format!("build-{ts}"));
    let staging_rootfs = staging_dir.join("rootfs");
    fs::create_dir_all(&staging_rootfs)
        .with_context(|| format!("create build staging rootfs '{}'", staging_rootfs.display()))?;

    let build_res = execute_build_steps(
        &instructions,
        &staging_dir,
        &staging_rootfs,
        &opts,
        ts,
    )
    .await;

    let (digest_hex, built_img_cfg) = match build_res {
        Ok(res) => res,
        Err(err) => {
            let _ = images::remove_dir_force(&staging_dir);
            return Err(err);
        }
    };

    // Atomically install the built rootfs into `<images_dir>/<slug>/rootfs`.
    let slug = format!(
        "{}-{}",
        display_tag
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            })
            .collect::<String>(),
        &digest_hex[..8]
    );

    let final_parent = opts.images_dir.join(&slug);
    let final_rootfs = final_parent.join("rootfs");
    fs::create_dir_all(&final_parent)?;
    if final_rootfs.exists() {
        let _ = images::remove_dir_force(&final_rootfs);
    }
    fs::rename(&staging_rootfs, &final_rootfs)
        .with_context(|| format!("install built rootfs to '{}'", final_rootfs.display()))?;
    let _ = images::remove_dir_force(&staging_dir);

    let abs_rootfs = fs::canonicalize(&final_rootfs)?;
    let _ = built_img_cfg.save_for_rootfs(&abs_rootfs);
    let size = images::dir_size(&abs_rootfs);
    let stored = StoredImage {
        id: format!("sha256:{digest_hex}"),
        repo_tags: vec![display_tag.clone()],
        size,
        rootfs_path: abs_rootfs.to_string_lossy().into_owned(),
        managed: true,
    };

    let mut index = images::load_index(opts.images_dir);
    let orphaned = images::upsert_image(&mut index, stored.clone());
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(opts.images_dir, &index, &old);
    }
    images::save_index(opts.images_dir, &index)?;

    info!(tag = %display_tag, size_bytes = size, "completed Dockerfile image build");
    Ok(stored)
}
