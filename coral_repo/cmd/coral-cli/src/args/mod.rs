//! Clap command-line interface definitions for `coral` (`Cli`, `Commands`, `PodAction`, `VolumeAction`).

mod compose;
mod container;
mod images;
mod misc;
mod network;
mod oci;
mod pod;
mod registry;
mod run;
mod sandbox;
mod system;
mod vm;
mod volume;
mod wasm;

pub use compose::{ComposeAction, ComposeArgs};
pub use container::{
    CommitArgs, CpArgs, DiffArgs, ExecArgs, InspectArgs, LogsArgs, RmArgs, StatsArgs, StopArgs,
};
pub use images::{
    BuildArgs, CatalogArgs, ImagesArgs, LoadArgs, PullArgs, PushArgs, RmiArgs, SaveArgs, TagArgs,
};
pub use misc::{CompletionArgs, CriStatusArgs, KeygenArgs, SbomArgs, SignArgs, VerifyArgs};
pub use network::{NetworkAction, NetworkArgs};
pub use oci::{CniConfigArgs, CreateArgs, DeleteArgs, KillArgs, SpecArgs, StartArgs, StateArgs};
pub use pod::{PodAction, PodsArgs};
pub use registry::{LoginArgs, LogoutArgs};
pub use run::RunArgs;
pub use sandbox::{SandboxAction, SandboxArgs};
pub use system::{SystemAction, SystemArgs};
pub use vm::{VmAction, VmArgs, VmDiskAction};
pub use volume::{VolumeAction, VolumeArgs};
pub use wasm::{WasmAction, WasmArgs};

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "coral",
    version,
    about = "Coral — Rust OCI Container Runtime, Linux Base Image Manager & CRI v1 CLI"
)]
pub struct Cli {
    /// Root directory for storing OCI container state
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Directory where pulled OCI Linux base images and images.json are stored
    #[arg(long, global = true, env = "CORAL_IMAGES_DIR")]
    pub images_dir: Option<PathBuf>,

    /// coral-daemon CRI v1 gRPC endpoint (`host:port`, `http://host:port`, or `unix:///path/coral.sock`)
    #[arg(
        long,
        alias = "endpoint",
        global = true,
        env = "CORAL_ADDR",
        default_value = "http://127.0.0.1:50051"
    )]
    pub addr: String,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Show the catalog of popular Linux base images and their install status
    Catalog(CatalogArgs),

    /// Pull an OCI image (Docker Hub / Red Hat Registry / GHCR / scratch) or register a local rootfs
    Pull(PullArgs),

    /// List installed Linux base and OCI images
    #[command(alias = "image")]
    Images(ImagesArgs),

    /// Remove an installed image (and delete its managed rootfs directory)
    Rmi(RmiArgs),

    /// Build a new OCI image from a single-stage or Multi-Stage Dockerfile
    Build(BuildArgs),

    /// Add a new repository:tag alias to an existing image
    Tag(TagArgs),

    /// Export an installed image to a portable archive (`.tar`, `.tar.gz`, or `.tar.zst`)
    Save(SaveArgs),

    /// Import an image from a `.tar`, `.tar.gz`, or `.tar.zst` archive created by `coral save`
    Load(LoadArgs),

    /// Manage multi-container stacks defined in `coral-compose.yml` or `docker-compose.yml`
    Compose(ComposeArgs),

    /// Inspect changes (`A` added, `C` modified, `D` deleted) in a container's OverlayFS CoW upper layer
    Diff(DiffArgs),

    /// Commit a container's OverlayFS Copy-on-Write changes into a new reusable image
    Commit(CommitArgs),

    /// Manage persistent named volumes (`coral volume create / ls / inspect / rm`)
    #[command(alias = "volumes")]
    Volume(VolumeArgs),

    /// Inspect full JSON metadata for a container (config, mounts, restart, health, metrics)
    Inspect(InspectArgs),

    /// Copy files or directories between the host and a container (`CTR:PATH DEST` or `SRC CTR:PATH`)
    Cp(CpArgs),

    /// Authenticate against an OCI Registry (Docker Hub, GHCR, Harbor, localhost)
    Login(LoginArgs),

    /// Remove stored credentials for an OCI Registry
    Logout(LogoutArgs),

    /// Push a locally installed or built image to an OCI Registry v2
    Push(PushArgs),

    /// Manage built-in and user-defined container networks (`coral network create / ls / inspect / rm`)
    #[command(alias = "networks")]
    Network(NetworkArgs),

    /// Run or package microsecond-startup WebAssembly (Wasm + WASI Preview 1) workloads
    Wasm(WasmArgs),

    /// Native Linux `/dev/kvm` 64-bit MicroVM hypervisor (`coral vm probe / run`)
    Vm(VmArgs),

    /// AI Agent / Untrusted Code hardened sandbox & MicroVM runner (`coral sandbox run`)
    Sandbox(SandboxArgs),

    /// Inspect disk usage and clean up unused Coral resources (`coral system df / prune`)
    System(SystemArgs),

    /// Run a container from an OCI image (`coral run alpine:3.20`) or OCI bundle (`--bundle`)
    Run(RunArgs),

    /// List containers managed by coral-daemon (CRI v1)
    Ps,

    /// Show or stream live logs and status for a coral-daemon container
    Logs(LogsArgs),

    /// Execute a command inside a running container (via `coral-daemon`, or `--local` `setns`)
    Exec(ExecArgs),

    /// Stop a running container in coral-daemon
    Stop(StopArgs),

    /// Remove a container from coral-daemon
    Rm(RmArgs),

    /// Manage CRI pod sandboxes in coral-daemon (defaults to listing pods)
    Pods(PodsArgs),

    /// Show Coral CLI version and coral-daemon CRI v1 status
    Version,

    /// Generate a standard OCI Runtime Specification v1.0.2 config.json
    Spec(SpecArgs),

    /// Create a container (paused in 'created' state until 'coral start')
    Create(CreateArgs),

    /// Start a previously created container
    Start(StartArgs),

    /// Print OCI state.json for a container
    State(StateArgs),

    /// List local OCI containers managed by Coral
    List,

    /// Send a POSIX signal to a running local OCI container (default: SIGTERM)
    Kill(KillArgs),

    /// Delete a stopped local OCI container (or force-kill a running container)
    Delete(DeleteArgs),

    /// Read live resource metrics (Cgroups v2 or rootless `/proc/<pid>`) for a container
    Stats(StatsArgs),

    /// Generate Coral's default CNI v1.0.0 bridge network configuration
    CniConfig(CniConfigArgs),

    /// Generate an Ed25519 signing keypair (`~/.config/coral/cosign.{key,pub}`) for image signing
    Keygen(KeygenArgs),

    /// Cryptographically sign a Coral image with Ed25519 (tamper-evident Merkle tree digest)
    Sign(SignArgs),

    /// Verify an image's Ed25519 signature and live rootfs integrity against a public key
    Verify(VerifyArgs),

    /// Generate a standards-compliant SPDX 2.3 JSON Software Bill of Materials (SBOM) for an image
    Sbom(SbomArgs),

    /// Query Kubernetes CRI v1 RuntimeService Status (`RuntimeReady`, `NetworkReady`, verbose info)
    CriStatus(CriStatusArgs),

    /// Generate shell completion script (`coral completion bash|zsh|fish|elvish|powershell`)
    Completion(CompletionArgs),

    /// Query a running coral-daemon CRI v1 gRPC endpoint for version info
    DaemonVersion,
}
