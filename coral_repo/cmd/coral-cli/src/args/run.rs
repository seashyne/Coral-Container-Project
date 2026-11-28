//! `coral run` — run a container from an OCI image or OCI bundle.

use crate::units::{PullPolicy, parse_memory_mb};
use clap::Args;
use std::path::PathBuf;

/// Arguments for `coral run`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Path to the OCI bundle directory containing config.json
    #[arg(short, long)]
    pub bundle: Option<PathBuf>,

    /// Image tag or local rootfs dir (optional; can also be passed positionally like `docker run`)
    #[arg(long)]
    pub image: Option<String>,

    /// Image pull policy (`missing` auto-pulls missing images, `always`, or `never`)
    #[arg(long, value_enum, default_value_t = PullPolicy::Missing)]
    pub pull: PullPolicy,

    /// Runtime backend: `oci` (default Linux container), `wasm` (WebAssembly WASI), or `vmm` (Sandbox/MicroVM)
    #[arg(long, default_value = "oci")]
    pub runtime: String,

    /// Container name / ID (optional when using --image)
    #[arg(long)]
    pub name: Option<String>,

    /// Keep STDIN open for interactive sessions (pair with -t for `-it`)
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Allocate a pseudo-TTY for interactive shells (pair with -i for `-it`)
    #[arg(short = 't', long)]
    pub tty: bool,

    /// Mount a named volume or host directory (`VOLUME_OR_PATH:CONTAINER_PATH[:ro|:rw]`), repeatable
    #[arg(short = 'v', long = "volume")]
    pub volumes: Vec<String>,

    /// Publish a container TCP or UDP port (`HOST:CTR[/tcp|/udp]` or `IP:HOST:CTR[/udp]`), repeatable
    #[arg(short = 'p', long = "publish")]
    pub ports: Vec<String>,

    /// Network mode: `host` (default), `bridge` (isolated per-container NetNS + TCP/UDP proxy), or `none`
    #[arg(long, default_value = "host")]
    pub network: String,

    /// Custom DNS nameserver IP (overrides host `/etc/resolv.conf`), repeatable
    #[arg(long)]
    pub dns: Vec<String>,

    /// Add a custom host-to-IP mapping (`HOST:IP`) to `/etc/hosts`, repeatable
    #[arg(long = "add-host")]
    pub extra_hosts: Vec<String>,

    /// Add network-scoped service alias (`--network-alias db`), repeatable
    #[arg(long = "network-alias", visible_alias = "alias")]
    pub network_aliases: Vec<String>,

    /// Pass a host device node into the container (`HOST_DEV[:CTR_DEV[:rwm]]`), repeatable
    #[arg(long = "device")]
    pub devices: Vec<String>,

    /// Pass host GPUs into the container (`all` or device index e.g. `0`)
    #[arg(long)]
    pub gpus: Option<String>,

    /// Container log driver (`raw`, `json-file`, `syslog`, `journald`)
    #[arg(long = "log-driver", default_value = "raw")]
    pub log_driver: String,

    /// Disable Copy-on-Write OverlayFS isolation and mount rootfs directly
    #[arg(long)]
    pub no_overlay: bool,

    /// Disable default Seccomp-BPF filter and Capabilities drop
    #[arg(long)]
    pub privileged: bool,

    /// Drop Linux capabilities (`ALL` or capability name e.g. `NET_RAW`), repeatable
    #[arg(long = "cap-drop")]
    pub cap_drop: Vec<String>,

    /// Add Linux capabilities (e.g. `NET_BIND_SERVICE`), repeatable
    #[arg(long = "cap-add")]
    pub cap_add: Vec<String>,

    /// Security options (`seccomp=unconfined|/path.json`, `landlock=enforce|off`, `apparmor=<profile>`), repeatable
    #[arg(long = "security-opt")]
    pub security_opt: Vec<String>,

    /// Mount the container's root filesystem as read-only
    #[arg(long = "read-only")]
    pub read_only: bool,

    /// Environment variable (KEY=VALUE), repeatable
    #[arg(short = 'e', long = "env")]
    pub envs: Vec<String>,

    /// Memory limit (supports `128`, `128m`, `4G`, or `0` for unlimited)
    #[arg(short = 'm', long = "memory", default_value = "0", value_parser = parse_memory_mb)]
    pub memory_mb: i64,

    /// CPU limit as percentage of one core (0 = unlimited, --image mode)
    #[arg(long = "cpu", default_value_t = 0)]
    pub cpu_percent: i64,

    /// Maximum number of PIDs (0 = unlimited, --image mode)
    #[arg(long = "pids", visible_alias = "pids-limit", default_value_t = 0)]
    pub pids_limit: i64,

    /// Working directory inside the container (defaults to image WORKDIR or `/`)
    #[arg(short = 'w', long)]
    pub workdir: Option<String>,

    /// Username or UID[:GID] to run as (defaults to image USER)
    #[arg(short = 'u', long)]
    pub user: Option<String>,

    /// Override the default ENTRYPOINT of the image (`""` clears ENTRYPOINT)
    #[arg(long)]
    pub entrypoint: Option<String>,

    /// Container hostname (--image mode)
    #[arg(long)]
    pub hostname: Option<String>,

    /// Restart policy (`no`, `always`, `unless-stopped`, `on-failure[:MAX]`; --daemon mode)
    #[arg(long, default_value = "no")]
    pub restart: String,

    /// Periodic healthcheck shell command run inside the container (--daemon mode)
    #[arg(long = "health-cmd")]
    pub health_cmd: Option<String>,

    /// Healthcheck probe interval in seconds (--daemon mode)
    #[arg(long = "health-interval", default_value_t = 5)]
    pub health_interval: u64,

    /// Consecutive healthcheck failures before marking unhealthy (--daemon mode)
    #[arg(long = "health-retries", default_value_t = 3)]
    pub health_retries: u32,

    /// Run managed inside coral-daemon (visible in `coral ps` and Coral Desktop)
    #[arg(long)]
    pub daemon: bool,

    /// Run container in background via coral-daemon and print container ID (implies `--daemon`)
    #[arg(short = 'd', long)]
    pub detach: bool,

    /// Keep container state.json on disk after exit (local OCI mode)
    #[arg(long)]
    pub keep: bool,

    /// Automatically remove the container after it exits
    #[arg(long)]
    pub rm: bool,

    /// Image reference (`alpine:3.20`, `itzg/minecraft-server`), or container ID when `--bundle` is set
    pub target: Option<String>,

    /// Command and arguments to run inside the container
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub cmd: Vec<String>,
}
