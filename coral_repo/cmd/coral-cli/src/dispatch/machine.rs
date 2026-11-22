//! Cross-platform Coral Machine Manager (`coral machine init|start|stop|status|env`)
//! supporting Linux native (`systemd`/KVM), macOS Lima + VirtioFS, and Windows WSL2.

use super::Ctx;
use crate::args::{MachineAction, MachineArgs};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MachineConfig {
    pub name: String,
    pub backend: String,
    pub cpus: u32,
    pub memory_mib: u32,
    pub disk_gib: u32,
    pub virtiofs: bool,
    pub coral_addr: String,
    pub docker_host: String,
    pub created_at: i64,
}

pub(crate) fn machine(ctx: &Ctx, args: MachineArgs) -> Result<ExitCode> {
    let dir = machines_dir();
    match args.action {
        MachineAction::Init {
            name,
            cpus,
            memory,
            disk,
            backend,
        } => {
            let cfg = init_machine_in(&dir, &name, cpus, memory, disk, &backend, &ctx.addr)?;
            println!(
                "Initialized Coral machine '{}' (backend={}, cpus={}, memory={}MiB, disk={}GiB, virtiofs={})",
                cfg.name, cfg.backend, cfg.cpus, cfg.memory_mib, cfg.disk_gib, cfg.virtiofs
            );
            println!("  CORAL_ADDR:  {}", cfg.coral_addr);
            println!("  DOCKER_HOST: {}", cfg.docker_host);
            if cfg.backend == "lima" {
                println!(
                    "  Lima YAML:   {}",
                    dir.join(format!("{}-lima.yaml", cfg.name)).display()
                );
            }
            println!("Run `coral machine start {}` and `eval \"$(coral machine env {})\"`.", cfg.name, cfg.name);
        }
        MachineAction::Start { name } => {
            let cfg = load_or_default(&dir, &name, &ctx.addr);
            start_machine(&dir, &cfg)?;
        }
        MachineAction::Stop { name } => {
            let cfg = load_or_default(&dir, &name, &ctx.addr);
            stop_machine(&cfg)?;
        }
        MachineAction::Status { name, json } => {
            let cfg = load_or_default(&dir, &name, &ctx.addr);
            let running = probe_backend_running(&cfg);
            if json {
                let out = serde_json::json!({
                    "name": cfg.name,
                    "backend": cfg.backend,
                    "running": running,
                    "cpus": cfg.cpus,
                    "memoryMiB": cfg.memory_mib,
                    "diskGiB": cfg.disk_gib,
                    "virtiofs": cfg.virtiofs,
                    "coralAddr": cfg.coral_addr,
                    "dockerHost": cfg.docker_host,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                println!("Machine:     {}", cfg.name);
                println!("Backend:     {}", cfg.backend);
                println!("State:       {}", if running { "running" } else { "stopped" });
                println!("Resources:   {} vCPU / {} MiB RAM / {} GiB disk", cfg.cpus, cfg.memory_mib, cfg.disk_gib);
                println!("VirtioFS:    {}", if cfg.virtiofs { "enabled" } else { "native-fs" });
                println!("CORAL_ADDR:  {}", cfg.coral_addr);
                println!("DOCKER_HOST: {}", cfg.docker_host);
            }
        }
        MachineAction::Env { name, shell } => {
            let cfg = load_or_default(&dir, &name, &ctx.addr);
            print!("{}", format_env_exports(&cfg, &shell)?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn machines_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral").join("machines");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join(".config").join("coral").join("machines");
    }
    PathBuf::from("/tmp/coral-machines")
}

fn detect_default_backend() -> &'static str {
    if cfg!(target_os = "macos") {
        "lima"
    } else if cfg!(target_os = "windows") {
        "wsl2"
    } else {
        "native"
    }
}

fn default_docker_host() -> String {
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.trim().is_empty()
    {
        return format!("unix://{xdg}/coral/docker.sock");
    }
    "unix:///run/coral/docker.sock".to_string()
}

pub(crate) fn init_machine_in(
    dir: &Path,
    name: &str,
    cpus: u32,
    memory_mib: u32,
    disk_gib: u32,
    backend_arg: &str,
    default_addr: &str,
) -> Result<MachineConfig> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!("invalid machine name '{trimmed}'");
    }
    let backend = match backend_arg.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => detect_default_backend().to_string(),
        "native" | "lima" | "wsl2" => backend_arg.trim().to_ascii_lowercase(),
        other => bail!("unsupported backend '{other}' (expected auto, native, lima, or wsl2)"),
    };
    let virtiofs = backend == "lima";
    let cfg = MachineConfig {
        name: trimmed.to_string(),
        backend: backend.clone(),
        cpus: cpus.max(1),
        memory_mib: memory_mib.max(512),
        disk_gib: disk_gib.max(8),
        virtiofs,
        coral_addr: default_addr.to_string(),
        docker_host: default_docker_host(),
        created_at: chrono::Utc::now().timestamp(),
    };

    fs::create_dir_all(dir)
        .with_context(|| format!("create machines dir '{}'", dir.display()))?;
    let cfg_path = dir.join(format!("{trimmed}.json"));
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg)?)?;

    if backend == "lima" {
        let yaml_path = dir.join(format!("{trimmed}-lima.yaml"));
        fs::write(&yaml_path, render_lima_virtiofs_yaml(&cfg))?;
    }
    Ok(cfg)
}

pub(crate) fn render_lima_virtiofs_yaml(cfg: &MachineConfig) -> String {
    format!(
        "# Auto-generated by Coral Machine Manager (VirtioFS + coral-daemon)\n\
vmType: \"vz\"\n\
mountType: \"virtiofs\"\n\
cpus: {}\n\
memory: \"{}MiB\"\n\
disk: \"{}GiB\"\n\
images:\n\
  - location: \"https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-arm64.img\"\n\
    arch: \"aarch64\"\n\
  - location: \"https://cloud-images.ubuntu.com/releases/24.04/release/ubuntu-24.04-server-cloudimg-amd64.img\"\n\
    arch: \"x86_64\"\n\
mounts:\n\
  - location: \"~\"\n\
    writable: true\n\
portForwards:\n\
  - guestPort: 50051\n\
    hostPort: 50051\n\
  - guestSocket: \"/run/coral/docker.sock\"\n\
    hostSocket: \"{{{{.Dir}}}}/sock/docker.sock\"\n",
        cfg.cpus, cfg.memory_mib, cfg.disk_gib
    )
}

pub(crate) fn format_env_exports(cfg: &MachineConfig, shell: &str) -> Result<String> {
    match shell.trim().to_ascii_lowercase().as_str() {
        "sh" | "bash" | "zsh" => Ok(format!(
            "export CORAL_ADDR=\"{}\"\nexport DOCKER_HOST=\"{}\"\n",
            cfg.coral_addr, cfg.docker_host
        )),
        "fish" => Ok(format!(
            "set -gx CORAL_ADDR \"{}\";\nset -gx DOCKER_HOST \"{}\";\n",
            cfg.coral_addr, cfg.docker_host
        )),
        "powershell" | "pwsh" => Ok(format!(
            "$Env:CORAL_ADDR = \"{}\"\n$Env:DOCKER_HOST = \"{}\"\n",
            cfg.coral_addr, cfg.docker_host
        )),
        other => bail!("unsupported shell '{other}' (expected sh, fish, or powershell)"),
    }
}

fn load_or_default(dir: &Path, name: &str, default_addr: &str) -> MachineConfig {
    let path = dir.join(format!("{}.json", name.trim()));
    if let Ok(raw) = fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str::<MachineConfig>(&raw)
    {
        return cfg;
    }
    let backend = detect_default_backend().to_string();
    MachineConfig {
        name: name.trim().to_string(),
        virtiofs: backend == "lima",
        backend,
        cpus: 4,
        memory_mib: 4096,
        disk_gib: 40,
        coral_addr: default_addr.to_string(),
        docker_host: default_docker_host(),
        created_at: 0,
    }
}

fn probe_backend_running(cfg: &MachineConfig) -> bool {
    match cfg.backend.as_str() {
        "native" => {
            if let Some(sock) = cfg.docker_host.strip_prefix("unix://")
                && Path::new(sock).exists()
            {
                return true;
            }
            Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", "coral-daemon.service"])
                .status()
                .is_ok_and(|s| s.success())
        }
        "lima" => Command::new("limactl")
            .args(["list", "-q"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l == cfg.name)),
        "wsl2" => Command::new("wsl")
            .args(["--list", "--running", "--quiet"])
            .output()
            .is_ok_and(|o| o.status.success()),
        _ => false,
    }
}

fn start_machine(dir: &Path, cfg: &MachineConfig) -> Result<()> {
    match cfg.backend.as_str() {
        "native" => {
            let status = Command::new("systemctl")
                .args(["--user", "start", "coral-daemon.service"])
                .status();
            if status.is_ok_and(|s| s.success()) {
                println!("Started native Coral daemon service (coral-daemon.service).");
            } else {
                println!(
                    "Native Linux host detected — run `coral-daemon` directly or install systemd user unit."
                );
            }
        }
        "lima" => {
            let yaml = dir.join(format!("{}-lima.yaml", cfg.name));
            let st = Command::new("limactl")
                .args(["start", "--name", &cfg.name, &yaml.to_string_lossy()])
                .status()
                .context("invoke `limactl` (install Lima via `brew install lima`)")?;
            if !st.success() {
                bail!("limactl start failed with status {st}");
            }
            println!("Started macOS Lima+VirtioFS machine '{}'.", cfg.name);
        }
        "wsl2" => {
            let st = Command::new("wsl")
                .args(["-d", "Ubuntu", "--", "systemctl", "start", "coral-daemon"])
                .status()
                .context("invoke `wsl` (install WSL2 via `wsl --install`)")?;
            if !st.success() {
                bail!("wsl start failed with status {st}");
            }
            println!("Started Windows WSL2 Coral machine '{}'.", cfg.name);
        }
        other => bail!("unsupported machine backend '{other}'"),
    }
    Ok(())
}

fn stop_machine(cfg: &MachineConfig) -> Result<()> {
    match cfg.backend.as_str() {
        "native" => {
            let _ = Command::new("systemctl")
                .args(["--user", "stop", "coral-daemon.service"])
                .status();
            println!("Stopped native Coral daemon service.");
        }
        "lima" => {
            let _ = Command::new("limactl").args(["stop", &cfg.name]).status();
            println!("Stopped Lima machine '{}'.", cfg.name);
        }
        "wsl2" => {
            let _ = Command::new("wsl")
                .args(["-d", "Ubuntu", "--", "systemctl", "stop", "coral-daemon"])
                .status();
            println!("Stopped WSL2 Coral machine '{}'.", cfg.name);
        }
        other => bail!("unsupported machine backend '{other}'"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn initializes_lima_virtiofs_profile_and_formats_shell_env() {
        let dir = tempdir().unwrap();
        let cfg = init_machine_in(
            dir.path(),
            "devbox",
            6,
            8192,
            60,
            "lima",
            "http://127.0.0.1:50051",
        )
        .unwrap();
        assert_eq!(cfg.backend, "lima");
        assert!(cfg.virtiofs);
        let yaml = fs::read_to_string(dir.path().join("devbox-lima.yaml")).unwrap();
        assert!(yaml.contains("mountType: \"virtiofs\""));
        assert!(yaml.contains("cpus: 6"));
        assert!(yaml.contains("8192MiB"));

        let sh_env = format_env_exports(&cfg, "sh").unwrap();
        assert!(sh_env.contains("export CORAL_ADDR=\"http://127.0.0.1:50051\""));
        assert!(sh_env.contains("export DOCKER_HOST=\"unix://"));
        let fish_env = format_env_exports(&cfg, "fish").unwrap();
        assert!(fish_env.contains("set -gx DOCKER_HOST"));
        let ps_env = format_env_exports(&cfg, "powershell").unwrap();
        assert!(ps_env.contains("$Env:DOCKER_HOST"));
    }
}
