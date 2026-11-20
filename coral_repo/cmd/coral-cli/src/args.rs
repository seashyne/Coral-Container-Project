//! Clap command-line interface definitions for `coral` (`Cli`, `Commands`, `PodAction`, `VolumeAction`).

use clap::{Parser, Subcommand};
use clap_complete::Shell;
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
pub enum PodAction {
    /// List pod sandboxes
    Ls,
    /// Stop a pod sandbox
    Stop {
        /// Pod sandbox ID
        id: String,
    },
    /// Remove a pod sandbox
    Rm {
        /// Pod sandbox ID
        id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum VolumeAction {
    /// Create a persistent named volume
    Create {
        /// Volume name
        name: String,
    },
    /// List persistent named volumes
    Ls,
    /// Print filesystem path and metadata for a named volume
    Inspect {
        /// Volume name
        name: String,
    },
    /// Remove a named volume and its stored data
    Rm {
        /// Volume name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ComposeAction {
    /// Build (if needed), create, and start all services in a Compose stack
    Up {
        /// Detached mode: start containers in coral-daemon and return immediately
        #[arg(short = 'd', long)]
        detach: bool,

        /// Force rebuilding images for services with a `build:` directive
        #[arg(long)]
        build: bool,
    },
    /// Stop and remove all containers and pods belonging to the Compose stack
    Down {
        /// Also delete named volumes declared in the Compose file
        #[arg(short = 'v', long)]
        volumes: bool,
    },
    /// List containers belonging to the Compose stack
    Ps,
    /// Show aggregated logs from all services (or one service) in the Compose stack
    Logs {
        /// Optional service name filter
        service: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WasmAction {
    /// Run a `.wasm` binary, `.wat` text file, or packed Wasm OCI image in microseconds
    Run {
        /// Path to `.wasm`/`.wat` file or installed Wasm image tag
        target: String,

        /// Deterministic CPU fuel budget (0 = unlimited)
        #[arg(long, default_value_t = 50_000_000)]
        fuel: u64,

        /// Linear memory ceiling in MB
        #[arg(short = 'm', long = "memory", default_value_t = 64)]
        memory_mb: usize,

        /// Environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Output structured JSON telemetry (exit code, fuel consumed, duration_usec)
        #[arg(long)]
        json: bool,

        /// Arguments passed to WASI `args_get` (pass after `--`)
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Package a `.wasm` or `.wat` file as a runnable Coral OCI Wasm image
    Pack {
        /// Source `.wasm` or `.wat` file
        file: PathBuf,

        /// Target image repository:tag (e.g. `hello-wasm:v1`)
        #[arg(short = 't', long = "tag")]
        tag: String,
    },
    /// Serve a `.wasm`/`.wat` or Wasm OCI image as a per-request isolated WASI HTTP/1.1 microservice
    Serve {
        /// Path to `.wasm`/`.wat` file or installed Wasm image tag
        target: String,

        /// TCP address to listen on (e.g. `127.0.0.1:8080`)
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,

        /// Deterministic per-request CPU fuel budget (0 = unlimited)
        #[arg(long, default_value_t = 50_000_000)]
        fuel: u64,

        /// Per-request linear memory ceiling in MB
        #[arg(short = 'm', long = "memory", default_value_t = 32)]
        memory_mb: usize,

        /// Extra environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum VmAction {
    /// Probe host `/dev/kvm` capabilities and run a live 64-bit Long Mode hardware MicroVM self-test
    Probe {
        /// Output structured JSON capabilities and self-test telemetry
        #[arg(long)]
        json: bool,
    },
    /// Download and install an Alpine `linux-virt` guest kernel (or register a local bzImage) for pure-Rust KVM boot
    SetupKernel {
        /// Optional local bzImage path to copy instead of downloading Alpine `linux-virt`
        #[arg(long)]
        from: Option<PathBuf>,

        /// Destination path (defaults to `~/.local/state/coral/vmm/vmlinuz-virt`)
        #[arg(long)]
        dest: Option<PathBuf>,

        /// Output structured JSON installation report
        #[arg(long)]
        json: bool,
    },
    /// Boot an OCI image inside a real separate Linux guest kernel on Coral's pure-Rust `/dev/kvm` hypervisor (Zero QEMU)
    Boot {
        /// Base image tag or rootfs directory (e.g. `alpine:3.20`)
        #[arg(long, default_value = "alpine:3.20")]
        image: String,

        /// Guest Linux bzImage kernel path (defaults to `~/.local/state/coral/vmm/vmlinuz-virt`)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Guest RAM in MB (64..=1024)
        #[arg(short = 'm', long = "memory", default_value_t = 256)]
        memory_mb: usize,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 15)]
        timeout: u64,

        /// Extra kernel command line arguments
        #[arg(long)]
        cmdline: Option<String>,

        /// Attach persistent raw/ext4 disk image via VirtIO-Blk (`/dev/vda`; append `:ro` for read-only)
        #[arg(long)]
        disk: Option<String>,

        /// Auto-mount `/dev/vda` onto this guest path inside the MicroVM (requires `--disk`)
        #[arg(long)]
        disk_mount: Option<String>,

        /// Enable pure-Rust VirtIO-Net networking (`eth0` 10.0.2.15/24, gateway 10.0.2.2)
        #[arg(long)]
        net: bool,

        /// Forward host TCP port into MicroVM (`HOST_PORT:GUEST_PORT`, implies `--net`)
        #[arg(short = 'p', long = "publish")]
        ports: Vec<String>,

        /// Output structured JSON report
        #[arg(long)]
        json: bool,

        /// Command to run as PID 1 inside the separate Linux kernel MicroVM (pass after `--`)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
    /// Manage persistent VirtIO-Blk raw/ext4 disk images for Coral KVM MicroVMs
    Disk {
        #[command(subcommand)]
        action: VmDiskAction,
    },
    /// Boot a raw 64-bit guest payload or Linux bzImage kernel + initramfs inside `/dev/kvm`
    Run {
        /// Path to raw x86-64 Long Mode payload binary (ignored when `--kernel` is set)
        payload: Option<PathBuf>,

        /// Guest Linux kernel bzImage path (requires `--initrd`)
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Guest compressed CPIO initramfs path (paired with `--kernel`)
        #[arg(long)]
        initrd: Option<PathBuf>,

        /// Extra Linux kernel cmdline parameters
        #[arg(long)]
        cmdline: Option<String>,

        /// Guest RAM in MB
        #[arg(short = 'm', long = "memory", default_value_t = 16)]
        memory_mb: usize,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 5)]
        timeout: u64,

        /// Maximum KVM VM-exits before terminating runaway guest
        #[arg(long, default_value_t = 100_000)]
        max_exits: u64,

        /// Output structured JSON execution report
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum VmDiskAction {
    /// Create a sparse VirtIO-Blk raw (or ext4-formatted) disk image for `coral vm boot --disk`
    Create {
        /// Output disk image path (e.g. `./data.img`)
        path: PathBuf,
        /// Disk capacity in MiB
        #[arg(long, default_value_t = 64)]
        size_mb: u64,
        /// Format the new disk image with ext4 via host `mkfs.ext4`
        #[arg(long)]
        ext4: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum SandboxAction {
    /// Execute untrusted code / AI Agent workloads inside a hardened timeout-enforced sandbox
    /// (auto-upgrades to a full Linux KVM MicroVM when `--kernel` or `CORAL_VMM_KERNEL` is provided)
    Run {
        /// Base image tag or rootfs path
        #[arg(long, default_value = "alpine:3.20")]
        image: String,

        /// Hard wall-clock timeout in seconds
        #[arg(long, default_value_t = 10)]
        timeout: u64,

        /// Memory ceiling in MB
        #[arg(short = 'm', long = "memory", default_value_t = 64)]
        memory_mb: i64,

        /// CPU quota as percentage of one core
        #[arg(long = "cpu", default_value_t = 50)]
        cpu_percent: i64,

        /// Maximum processes/threads
        #[arg(long = "pids", default_value_t = 32)]
        pids_limit: i64,

        /// Allow isolated bridge networking instead of air-gapped `--network none`
        #[arg(long)]
        allow_network: bool,

        /// Optional Linux guest kernel bzImage for full separate-kernel KVM MicroVM execution
        #[arg(long)]
        kernel: Option<PathBuf>,

        /// Environment variable (`KEY=VALUE`), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Output structured JSON report (`isolationMode`, `kvmHardwareAvailable`, `stdout`, `stderr`, `timedOut`)
        #[arg(long)]
        json: bool,

        /// Untrusted command and arguments (pass after `--`)
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum NetworkAction {
    /// Create a user-defined isolated bridge network
    Create {
        /// Network name (e.g. `backend`)
        name: String,

        /// Network driver (`bridge`)
        #[arg(long, default_value = "bridge")]
        driver: String,

        /// Optional IPv4 CIDR subnet (e.g. `10.89.10.0/24`; auto-allocated if omitted)
        #[arg(long)]
        subnet: Option<String>,
    },
    /// List built-in and user-defined networks
    Ls,
    /// Print JSON configuration for a network
    Inspect {
        /// Network name
        name: String,
    },
    /// Remove a user-defined network
    Rm {
        /// Network name
        name: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum SystemAction {
    /// Show Coral disk usage (images, containers, named volumes, build cache)
    Df,
    /// Remove stopped containers, unused pod sandboxes, and build cache
    Prune {
        /// Also remove all unused managed images
        #[arg(short = 'a', long)]
        all: bool,

        /// Also remove all unused named volumes
        #[arg(long)]
        volumes: bool,
    },
}

#[derive(Debug, Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// Show the catalog of popular Linux base images and their install status
    Catalog {
        /// Read local images store directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Pull an OCI image (Docker Hub / Red Hat Registry / GHCR / scratch) or register a local rootfs
    Pull {
        /// Image reference (e.g. alpine:3.20, debian:bookworm-slim, ubuntu:24.04, busybox:musl, rockylinux:9-minimal, registry.access.redhat.com/ubi9/ubi-minimal:latest, scratch)
        image: String,

        /// Register an existing local rootfs directory offline instead of pulling from a registry
        #[arg(long)]
        rootfs: Option<PathBuf>,

        /// Pull directly into local --images-dir without going through coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// List installed Linux base and OCI images
    #[command(alias = "image")]
    Images {
        /// Read local images store directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Remove an installed image (and delete its managed rootfs directory)
    Rmi {
        /// Image tag or ID to remove
        image: String,

        /// Modify local images store directly without going through coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Build a new OCI image from a Dockerfile (`FROM`, `RUN`, `COPY`, `ENV`, `WORKDIR`, `ARG`)
    Build {
        /// Name and optional tag for the built image (e.g. `my-app:v1`)
        #[arg(short = 't', long = "tag")]
        tag: String,

        /// Path to the Dockerfile (defaults to `<CONTEXT>/Dockerfile`)
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Build context directory for `COPY`/`ADD` instructions
        #[arg(default_value = ".")]
        context: PathBuf,

        /// Build directly into local `--images-dir` without going through coral-daemon
        #[arg(long)]
        local: bool,

        /// Do not use cached step layers when building the image
        #[arg(long)]
        no_cache: bool,
    },

    /// Add a new repository:tag alias to an existing image
    Tag {
        /// Existing source image tag or ID
        source: String,

        /// Target image tag (e.g. `my-app:stable`)
        target: String,

        /// Modify local images store directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Export an installed image to a portable archive (`.tar`, `.tar.gz`, or `.tar.zst`)
    Save {
        /// Output archive path (format inferred from extension: `.tar`, `.tar.gz`, `.tar.zst`)
        #[arg(short = 'o', long = "output")]
        output: PathBuf,

        /// Image tag or ID to export
        image: String,

        /// Read local images store directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Import an image from a `.tar`, `.tar.gz`, or `.tar.zst` archive created by `coral save`
    Load {
        /// Input archive file path
        #[arg(short = 'i', long = "input")]
        input: PathBuf,

        /// Optional image tag override (defaults to tags embedded in archive metadata)
        #[arg(short = 't', long = "tag")]
        tag: Option<String>,

        /// Import directly into local `--images-dir` without going through coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Manage multi-container stacks defined in `coral-compose.yml` or `docker-compose.yml`
    Compose {
        /// Path to compose YAML file (defaults to coral-compose.yml / docker-compose.yml)
        #[arg(short = 'f', long = "file")]
        file: Option<PathBuf>,

        /// Override the Compose project name (defaults to `name:` in YAML or directory name)
        #[arg(short = 'p', long = "project-name")]
        project_name: Option<String>,

        #[command(subcommand)]
        action: ComposeAction,
    },

    /// Inspect changes (`A` added, `C` modified, `D` deleted) in a container's OverlayFS CoW upper layer
    Diff {
        /// Container ID
        id: String,

        /// Inspect local container state directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Commit a container's OverlayFS Copy-on-Write changes into a new reusable image
    Commit {
        /// Source container ID
        container: String,

        /// Target image repository:tag (e.g. `my-custom-alpine:v1`)
        image: String,

        /// Commit directly into local `--images-dir` without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Manage persistent named volumes (`coral volume create / ls / inspect / rm`)
    #[command(alias = "volumes")]
    Volume {
        #[command(subcommand)]
        action: Option<VolumeAction>,
    },

    /// Inspect full JSON metadata for a container (config, mounts, restart, health, metrics)
    Inspect {
        /// Container ID
        id: String,

        /// Read local OCI state & config.json directly without querying coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Copy files or directories between the host and a container (`CTR:PATH DEST` or `SRC CTR:PATH`)
    Cp {
        /// Source path (`HOST_PATH` or `CONTAINER_ID:/path`)
        src: String,

        /// Destination path (`CONTAINER_ID:/path` or `HOST_PATH`)
        dst: String,
    },

    /// Authenticate against an OCI Registry (Docker Hub, GHCR, Harbor, localhost)
    Login {
        /// Registry host (defaults to `docker.io`)
        #[arg(default_value = "docker.io")]
        registry: String,

        /// Registry username
        #[arg(short = 'u', long)]
        username: String,

        /// Password or personal access token (or pass `--password-stdin`)
        #[arg(short = 'p', long)]
        password: Option<String>,

        /// Read password / token from STDIN
        #[arg(long)]
        password_stdin: bool,
    },

    /// Remove stored credentials for an OCI Registry
    Logout {
        /// Registry host (defaults to `docker.io`)
        #[arg(default_value = "docker.io")]
        registry: String,
    },

    /// Push a locally installed or built image to an OCI Registry v2
    Push {
        /// Target image reference (e.g. `ghcr.io/user/app:v1`, `localhost:5000/demo:latest`)
        image: String,

        /// Read local images store directly without going through coral-daemon
        #[arg(long)]
        local: bool,
    },

    /// Manage built-in and user-defined container networks (`coral network create / ls / inspect / rm`)
    #[command(alias = "networks")]
    Network {
        #[command(subcommand)]
        action: Option<NetworkAction>,
    },

    /// Run or package microsecond-startup WebAssembly (Wasm + WASI Preview 1) workloads
    Wasm {
        #[command(subcommand)]
        action: WasmAction,
    },

    /// Native Linux `/dev/kvm` 64-bit MicroVM hypervisor (`coral vm probe / run`)
    Vm {
        #[command(subcommand)]
        action: VmAction,
    },

    /// AI Agent / Untrusted Code hardened sandbox & MicroVM runner (`coral sandbox run`)
    Sandbox {
        #[command(subcommand)]
        action: SandboxAction,
    },

    /// Inspect disk usage and clean up unused Coral resources (`coral system df / prune`)
    System {
        #[command(subcommand)]
        action: SystemAction,
    },

    /// Run a container from an OCI bundle (--bundle) or a pulled Linux base image (--image)
    Run {
        /// Path to the OCI bundle directory containing config.json
        #[arg(short, long, default_value = ".")]
        bundle: PathBuf,

        /// Run directly from an installed image tag (e.g. debian:bookworm-slim, ubuntu:24.04) or rootfs dir
        #[arg(long)]
        image: Option<String>,

        /// Runtime backend: `oci` (default Linux container), `wasm` (WebAssembly WASI), or `vmm` (Sandbox/MicroVM)
        #[arg(long, default_value = "oci")]
        runtime: String,

        /// Container name / ID (optional when using --image)
        #[arg(long)]
        name: Option<String>,

        /// Keep STDIN open for interactive sessions (pair with -t for `-it`)
        #[arg(short = 'i', long)]
        interactive: bool,

        /// Allocate a pseudo-TTY for interactive shells (pair with -i for `-it`)
        #[arg(short = 't', long)]
        tty: bool,

        /// Mount a named volume or host directory (`VOLUME_OR_PATH:CONTAINER_PATH[:ro|:rw]`), repeatable
        #[arg(short = 'v', long = "volume")]
        volumes: Vec<String>,

        /// Publish a container TCP port (`HOST_PORT:CONTAINER_PORT` or `IP:HOST:CTR`), repeatable
        #[arg(short = 'p', long = "publish")]
        ports: Vec<String>,

        /// Network mode: `host` (default), `bridge` (isolated per-container NetNS + TCP port proxy), or `none`
        #[arg(long, default_value = "host")]
        network: String,

        /// Custom DNS nameserver IP (overrides host `/etc/resolv.conf`), repeatable
        #[arg(long)]
        dns: Vec<String>,

        /// Add a custom host-to-IP mapping (`HOST:IP`) to `/etc/hosts`, repeatable
        #[arg(long = "add-host")]
        extra_hosts: Vec<String>,

        /// Disable Copy-on-Write OverlayFS isolation and mount rootfs directly
        #[arg(long)]
        no_overlay: bool,

        /// Disable default Seccomp-BPF filter and Capabilities drop
        #[arg(long)]
        privileged: bool,

        /// Environment variable (KEY=VALUE), repeatable
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Memory limit in MB (0 = unlimited, --image mode)
        #[arg(short = 'm', long = "memory", default_value_t = 0)]
        memory_mb: i64,

        /// CPU limit as percentage of one core (0 = unlimited, --image mode)
        #[arg(long = "cpu", default_value_t = 0)]
        cpu_percent: i64,

        /// Maximum number of PIDs (0 = unlimited, --image mode)
        #[arg(long = "pids", default_value_t = 0)]
        pids_limit: i64,

        /// Working directory inside the container (defaults to image WORKDIR or `/`)
        #[arg(short = 'w', long)]
        workdir: Option<String>,

        /// Username or UID[:GID] to run as (defaults to image USER)
        #[arg(short = 'u', long)]
        user: Option<String>,

        /// Override the default ENTRYPOINT of the image (`""` clears ENTRYPOINT)
        #[arg(long)]
        entrypoint: Option<String>,

        /// Container hostname (--image mode)
        #[arg(long)]
        hostname: Option<String>,

        /// Restart policy (`no`, `always`, `unless-stopped`, `on-failure[:MAX]`; --daemon mode)
        #[arg(long, default_value = "no")]
        restart: String,

        /// Periodic healthcheck shell command run inside the container (--daemon mode)
        #[arg(long = "health-cmd")]
        health_cmd: Option<String>,

        /// Healthcheck probe interval in seconds (--daemon mode)
        #[arg(long = "health-interval", default_value_t = 5)]
        health_interval: u64,

        /// Consecutive healthcheck failures before marking unhealthy (--daemon mode)
        #[arg(long = "health-retries", default_value_t = 3)]
        health_retries: u32,

        /// Run managed inside coral-daemon (visible in `coral ps` and Coral Desktop)
        #[arg(long)]
        daemon: bool,

        /// Print container ID and return immediately (requires --daemon)
        #[arg(short = 'd', long)]
        detach: bool,

        /// Keep container state.json on disk after exit (local OCI mode)
        #[arg(long)]
        keep: bool,

        /// Automatically remove the container after it exits (Docker compatibility flag)
        #[arg(long)]
        rm: bool,

        /// Unique container ID (required in --bundle mode; optional in --image mode)
        id: Option<String>,

        /// Command and arguments to run inside the container (pass after `--` in --image mode)
        #[arg(last = true)]
        cmd: Vec<String>,
    },

    /// List containers managed by coral-daemon (CRI v1)
    Ps,

    /// Show or stream live logs and status for a coral-daemon container
    Logs {
        /// Container ID
        id: String,

        /// Include container state, exit code, health, restart count, and timestamps header
        #[arg(short, long)]
        status: bool,

        /// Stream live log output until the container exits (like `docker logs -f`)
        #[arg(short = 'f', long)]
        follow: bool,

        /// Number of trailing lines to show from the end of the logs
        #[arg(long)]
        tail: Option<usize>,
    },

    /// Execute a command inside a running container (via `coral-daemon`, or `--local` `setns`)
    Exec {
        /// Container ID
        id: String,

        /// Enter a local OCI container's live namespaces directly via `setns()` (no daemon)
        #[arg(long)]
        local: bool,

        /// Keep STDIN open for interactive `--local` exec (pair with `-t`)
        #[arg(short = 'i', long)]
        interactive: bool,

        /// Allocate a pseudo-TTY for interactive `--local` exec (pair with `-i`)
        #[arg(short = 't', long)]
        tty: bool,

        /// Additional environment variables (`KEY=VALUE`) for `--local` exec
        #[arg(short = 'e', long = "env")]
        envs: Vec<String>,

        /// Working directory override for `--local` exec
        #[arg(short = 'w', long)]
        workdir: Option<String>,

        /// Command timeout in seconds (daemon mode)
        #[arg(long, default_value_t = 30)]
        timeout: i64,

        /// Command and arguments to execute
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        cmd: Vec<String>,
    },

    /// Stop a running container in coral-daemon
    Stop {
        /// Container ID
        id: String,

        /// Stop timeout in seconds
        #[arg(short, long, default_value_t = 10)]
        timeout: i64,
    },

    /// Remove a container from coral-daemon
    Rm {
        /// Container ID
        id: String,
    },

    /// Manage CRI pod sandboxes in coral-daemon (defaults to listing pods)
    Pods {
        #[command(subcommand)]
        action: Option<PodAction>,
    },

    /// Show Coral CLI version and coral-daemon CRI v1 status
    Version,

    /// Generate a standard OCI Runtime Specification v1.0.2 config.json
    Spec {
        /// Generate a rootless OCI spec with UserNS UID/GID mappings
        #[arg(long)]
        rootless: bool,

        /// Path to the container rootfs
        #[arg(long, default_value = "rootfs")]
        rootfs: String,

        /// Output file path
        #[arg(short, long, default_value = "config.json")]
        output: PathBuf,
    },

    /// Create a container (paused in 'created' state until 'coral start')
    Create {
        /// Path to the OCI bundle directory containing config.json
        #[arg(short, long, default_value = ".")]
        bundle: PathBuf,

        /// Unique container ID
        id: String,
    },

    /// Start a previously created container
    Start {
        /// Container ID
        id: String,
    },

    /// Print OCI state.json for a container
    State {
        /// Container ID
        id: String,
    },

    /// List local OCI containers managed by Coral
    List,

    /// Send a POSIX signal to a running local OCI container (default: SIGTERM)
    Kill {
        /// Container ID
        id: String,

        /// Signal name or number (e.g. SIGTERM, KILL, 9)
        #[arg(default_value = "SIGTERM")]
        signal: String,
    },

    /// Delete a stopped local OCI container (or force-kill a running container)
    Delete {
        /// Force-kill before deleting if still running
        #[arg(short, long)]
        force: bool,

        /// Container ID
        id: String,
    },

    /// Read live resource metrics (Cgroups v2 or rootless `/proc/<pid>`) for a container
    Stats {
        /// Container ID
        id: String,

        /// Continuously watch live CPU, Memory, PIDs, and Health metrics
        #[arg(short = 'w', long)]
        watch: bool,

        /// Polling interval in milliseconds when `--watch` is enabled
        #[arg(long, default_value_t = 1000)]
        interval: u64,
    },

    /// Generate Coral's default CNI v1.0.0 bridge network configuration
    CniConfig {
        /// Optional output path (prints to stdout if omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Generate an Ed25519 signing keypair (`~/.config/coral/cosign.{key,pub}`) for image signing
    Keygen {
        /// Private key output path (mode 0600)
        #[arg(long)]
        key: Option<PathBuf>,

        /// Public key output path
        #[arg(long = "pub")]
        pub_key: Option<PathBuf>,

        /// Overwrite existing key files
        #[arg(long)]
        force: bool,

        /// Output structured JSON report
        #[arg(long)]
        json: bool,
    },

    /// Cryptographically sign a Coral image with Ed25519 (tamper-evident Merkle tree digest)
    Sign {
        /// Image tag or ID (e.g. `alpine:3.20`)
        image: String,

        /// Ed25519 private key path (defaults to `~/.config/coral/cosign.key`)
        #[arg(long)]
        key: Option<PathBuf>,

        /// Output structured JSON signature envelope
        #[arg(long)]
        json: bool,
    },

    /// Verify an image's Ed25519 signature and live rootfs integrity against a public key
    Verify {
        /// Image tag or ID (e.g. `alpine:3.20`)
        image: String,

        /// Ed25519 public key path (defaults to `~/.config/coral/cosign.pub`)
        #[arg(long)]
        key: Option<PathBuf>,

        /// Output structured JSON verification report
        #[arg(long)]
        json: bool,
    },

    /// Generate a standards-compliant SPDX 2.3 JSON Software Bill of Materials (SBOM) for an image
    Sbom {
        /// Image tag or ID (e.g. `alpine:3.20`)
        image: String,

        /// Optional output file path (prints JSON to stdout if omitted)
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },

    /// Query Kubernetes CRI v1 RuntimeService Status (`RuntimeReady`, `NetworkReady`, verbose info)
    CriStatus {
        /// Include verbose runtime configuration JSON (same as `crictl info`)
        #[arg(short, long)]
        verbose: bool,
    },

    /// Generate shell completion script (`coral completion bash|zsh|fish|elvish|powershell`)
    Completion {
        /// Target shell (`bash`, `zsh`, `fish`, `elvish`, `powershell`)
        shell: Shell,
    },

    /// Query a running coral-daemon CRI v1 gRPC endpoint for version info
    DaemonVersion,
}
