//! `coral` image management subcommands (`catalog`, `pull`, `images`, `rmi`, `build`,
//! `tag`, `save`, `load`, `push`).

use clap::Args;
use std::path::PathBuf;

/// Arguments for `coral catalog`.
#[derive(Debug, Args)]
pub struct CatalogArgs {
    /// Read local images store directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral pull`.
#[derive(Debug, Args)]
pub struct PullArgs {
    /// Image reference (e.g. alpine:3.20, debian:bookworm-slim, ubuntu:24.04, busybox:musl, rockylinux:9-minimal, registry.access.redhat.com/ubi9/ubi-minimal:latest, scratch)
    pub image: String,

    /// Register an existing local rootfs directory offline instead of pulling from a registry
    #[arg(long)]
    pub rootfs: Option<PathBuf>,

    /// Target OCI platform (e.g. `linux/amd64`, `linux/arm64`)
    #[arg(long)]
    pub platform: Option<String>,

    /// Pull directly into local --images-dir without going through coral-daemon
    #[arg(long)]
    pub local: bool,

    /// Image download progress reporting (`auto` bar, `json` lines, or `none`)
    #[arg(long, value_enum, default_value_t = crate::daemon_client::ProgressMode::Auto)]
    pub progress: crate::daemon_client::ProgressMode,
}

/// Arguments for `coral images`.
#[derive(Debug, Args)]
pub struct ImagesArgs {
    /// Read local images store directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral rmi`.
#[derive(Debug, Args)]
pub struct RmiArgs {
    /// Image tag or ID to remove
    pub image: String,

    /// Modify local images store directly without going through coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral build`.
#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Name and optional tag for the built image (e.g. `my-app:v1`)
    #[arg(short = 't', long = "tag")]
    pub tag: String,

    /// Path to the Dockerfile (defaults to `<CONTEXT>/Dockerfile`)
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,

    /// Stop building at the named Multi-Stage Dockerfile target (`FROM ... AS <target>`)
    #[arg(long)]
    pub target: Option<String>,

    /// Set build-time variable (`KEY=VAL`) passed to Dockerfile `ARG`, repeatable
    #[arg(long = "build-arg")]
    pub build_args: Vec<String>,

    /// Target OCI platform(s) for cross-arch BuildKit builds (e.g. `linux/arm64` or `linux/amd64,linux/arm64`)
    #[arg(long)]
    pub platform: Option<String>,

    /// Remote registry image(s) to pull first as warm layer/build cache (`--cache-from`)
    #[arg(long = "cache-from")]
    pub cache_from: Vec<String>,

    /// Push the built image (or multi-arch OCI Image Index when multiple --platform targets are given)
    #[arg(long)]
    pub push: bool,

    /// Build context directory for `COPY`/`ADD` instructions
    #[arg(default_value = ".")]
    pub context: PathBuf,

    /// Build directly into local `--images-dir` without going through coral-daemon
    #[arg(long)]
    pub local: bool,

    /// Do not use cached step layers when building the image
    #[arg(long)]
    pub no_cache: bool,
}

/// Arguments for `coral tag`.
#[derive(Debug, Args)]
pub struct TagArgs {
    /// Existing source image tag or ID
    pub source: String,

    /// Target image tag (e.g. `my-app:stable`)
    pub target: String,

    /// Modify local images store directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral save`.
#[derive(Debug, Args)]
pub struct SaveArgs {
    /// Output archive path (format inferred from extension: `.tar`, `.tar.gz`, `.tar.zst`)
    #[arg(short = 'o', long = "output")]
    pub output: PathBuf,

    /// Image tag or ID to export
    pub image: String,

    /// Read local images store directly without querying coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral load`.
#[derive(Debug, Args)]
pub struct LoadArgs {
    /// Input archive file path
    #[arg(short = 'i', long = "input")]
    pub input: PathBuf,

    /// Optional image tag override (defaults to tags embedded in archive metadata)
    #[arg(short = 't', long = "tag")]
    pub tag: Option<String>,

    /// Import directly into local `--images-dir` without going through coral-daemon
    #[arg(long)]
    pub local: bool,
}

/// Arguments for `coral push`.
#[derive(Debug, Args)]
pub struct PushArgs {
    /// Target image reference (e.g. `ghcr.io/user/app:v1`, `localhost:5000/demo:latest`)
    pub image: String,

    /// Read local images store directly without going through coral-daemon
    #[arg(long)]
    pub local: bool,
}
