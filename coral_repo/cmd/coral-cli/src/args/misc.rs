//! Image signing, SBOM, CRI status and shell-completion subcommands
//! (`keygen`, `sign`, `verify`, `sbom`, `cri-status`, `completion`).

use clap::{Args, Subcommand};
use clap_complete::Shell;
use std::path::PathBuf;

/// Arguments for `coral security`.
#[derive(Debug, Args)]
pub struct SecurityArgs {
    #[command(subcommand)]
    pub action: SecurityAction,
}

#[derive(Debug, Subcommand)]
pub enum SecurityAction {
    /// Run a CIS container runtime & host security posture audit
    Audit {
        /// Output structured JSON report
        #[arg(long)]
        json: bool,
    },
}

/// Arguments for `coral keygen`.
#[derive(Debug, Args)]
pub struct KeygenArgs {
    /// Private key output path (mode 0600)
    #[arg(long)]
    pub key: Option<PathBuf>,

    /// Public key output path
    #[arg(long = "pub")]
    pub pub_key: Option<PathBuf>,

    /// Overwrite existing key files
    #[arg(long)]
    pub force: bool,

    /// Output structured JSON report
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `coral sign`.
#[derive(Debug, Args)]
pub struct SignArgs {
    /// Image tag or ID (e.g. `alpine:3.20`)
    pub image: String,

    /// Ed25519 private key path (defaults to `~/.config/coral/cosign.key`)
    #[arg(long)]
    pub key: Option<PathBuf>,

    /// Output structured JSON signature envelope
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `coral verify`.
#[derive(Debug, Args)]
pub struct VerifyArgs {
    /// Image tag or ID (e.g. `alpine:3.20`)
    pub image: String,

    /// Ed25519 public key path (defaults to `~/.config/coral/cosign.pub`)
    #[arg(long)]
    pub key: Option<PathBuf>,

    /// Output structured JSON verification report
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `coral sbom`.
#[derive(Debug, Args)]
pub struct SbomArgs {
    /// Image tag or ID (e.g. `alpine:3.20`)
    pub image: String,

    /// Optional output file path (prints JSON to stdout if omitted)
    #[arg(short = 'o', long)]
    pub output: Option<PathBuf>,
}

/// Arguments for `coral scan` (OSV.dev CVE vulnerability scanner).
#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Image tag or ID (e.g. `alpine:3.20`)
    pub image: String,

    /// Output structured JSON report
    #[arg(long)]
    pub json: bool,

    /// Inventory packages offline without calling OSV.dev
    #[arg(long)]
    pub offline: bool,

    /// Exit with non-zero code if vulnerabilities meet or exceed severity (`critical|high|medium|low`)
    #[arg(long = "fail-on")]
    pub fail_on: Option<String>,
}

/// Arguments for `coral cri-status`.
#[derive(Debug, Args)]
pub struct CriStatusArgs {
    /// Include verbose runtime configuration JSON (same as `crictl info`)
    #[arg(short, long)]
    pub verbose: bool,
}

/// Arguments for `coral completion`.
#[derive(Debug, Args)]
pub struct CompletionArgs {
    /// Target shell (`bash`, `zsh`, `fish`, `elvish`, `powershell`)
    pub shell: Shell,
}
