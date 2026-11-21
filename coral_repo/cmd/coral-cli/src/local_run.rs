//! Local single-threaded image execution (`coral run --image`), OverlayFS CoW inspection
//! (`coral diff`), and local image commit (`coral commit`).

use crate::catalog;
use crate::port_proxy::ForkedPortProxies;
use anyhow::{Context, Result};
use coral_daemon::images::{self, OciImageConfig, merge_env_vars, resolve_entrypoint_and_cmd};
use coral_oci::{
    LinuxCpu, LinuxMemory, LinuxNamespace, LinuxNamespaceType, LinuxPids, LinuxResources, OciSpec,
};
use coral_runtime::overlay::commit_overlay_to_rootfs;
use coral_runtime::rootfs::{parse_volume_spec, prepare_dns_and_hosts_mounts};
use coral_runtime::{ContainerManager, RunOptions, StateStore};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub fn auto_container_id(image: &str) -> String {
    let base = image
        .rsplit('/')
        .next()
        .unwrap_or("ctr")
        .split(':')
        .next()
        .unwrap_or("ctr");
    let slug: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let prefix = if slug.is_empty() { "ctr" } else { slug };
    let ts = chrono::Utc::now().timestamp_millis();
    format!("{prefix}-{ts}")
}

pub struct LocalImageRunArgs {
    pub image: String,
    pub container_id: String,
    pub cmd: Vec<String>,
    pub envs: Vec<String>,
    pub volumes: Vec<String>,
    pub ports: Vec<String>,
    pub network: String,
    pub dns: Vec<String>,
    pub extra_hosts: Vec<String>,
    pub no_overlay: bool,
    pub privileged: bool,
    pub tty: bool,
    pub interactive: bool,
    pub memory_mb: i64,
    pub cpu_percent: i64,
    pub pids_limit: i64,
    pub workdir: Option<String>,
    pub user: Option<String>,
    pub entrypoint: Option<String>,
    pub hostname: Option<String>,
    pub keep: bool,
}

/// Runs a container directly from an installed image in `images_dir` WITHOUT spawning Tokio,
/// preserving Coral's single-threaded POSIX-safe fork/unshare pipeline.
pub fn run_from_image_local(
    mgr: &ContainerManager,
    state_root: &Path,
    images_dir: &Path,
    args: LocalImageRunArgs,
) -> Result<ExitCode> {
    let rootfs_path = if Path::new(&args.image).is_dir() {
        fs::canonicalize(&args.image)
            .with_context(|| format!("resolve rootfs directory '{}'", args.image))?
    } else {
        let index = images::load_index(images_dir);
        let img = images::find_image(&index, &args.image).ok_or_else(|| {
            anyhow::anyhow!(
                "image '{}' is not installed in {} — pull it first with:\n  coral pull {}",
                args.image,
                images_dir.display(),
                args.image
            )
        })?;
        PathBuf::from(&img.rootfs_path)
    };

    let bundle_dir = state_root.join(".cli-bundles").join(&args.container_id);
    fs::create_dir_all(&bundle_dir)
        .with_context(|| format!("create temporary bundle dir '{}'", bundle_dir.display()))?;

    let host_uid = unsafe { libc::geteuid() };
    let host_gid = unsafe { libc::getegid() };
    let mut spec = OciSpec::default_spec(host_uid != 0, host_uid, host_gid);
    if let Some(r) = spec.root.as_mut() {
        r.path = rootfs_path.to_string_lossy().into_owned();
    }
    let hostname = args.hostname.unwrap_or_else(|| args.container_id.clone());
    spec.hostname = Some(hostname.clone());

    if !args.no_overlay {
        spec.annotations
            .insert("coral.overlay".into(), "true".into());
    }
    if args.privileged {
        spec.annotations
            .insert("coral.privileged".into(), "true".into());
    }

    let is_custom_net = !args.network.eq_ignore_ascii_case("host")
        && !args.network.eq_ignore_ascii_case("none")
        && !args.network.eq_ignore_ascii_case("bridge")
        && !args.network.eq_ignore_ascii_case("isolated")
        && coral_net::find_network(&args.network).is_some();
    let is_bridge_net = args.network.eq_ignore_ascii_case("bridge")
        || args.network.eq_ignore_ascii_case("isolated")
        || is_custom_net;

    if args.network.eq_ignore_ascii_case("none") {
        if let Some(linux) = spec.linux.as_mut() {
            linux.namespaces.push(LinuxNamespace {
                typ: LinuxNamespaceType::Network,
                path: None,
            });
        }
    } else if is_bridge_net {
        if let Some(linux) = spec.linux.as_mut() {
            linux.namespaces.push(LinuxNamespace {
                typ: LinuxNamespaceType::Network,
                path: None,
            });
        }
        spec.mounts.extend(prepare_dns_and_hosts_mounts(
            &bundle_dir,
            &hostname,
            &args.dns,
            &args.extra_hosts,
        )?);
    } else if args.network.eq_ignore_ascii_case("host") {
        spec.mounts.extend(prepare_dns_and_hosts_mounts(
            &bundle_dir,
            &hostname,
            &args.dns,
            &args.extra_hosts,
        )?);
    } else {
        anyhow::bail!(
            "unsupported --network '{}' (expected 'host', 'bridge', or 'none')",
            args.network
        );
    }

    for vol in &args.volumes {
        spec.mounts.push(parse_volume_spec(vol)?);
    }

    let img_cfg = OciImageConfig::load_for_rootfs(&rootfs_path);
    let effective_user = args
        .user
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| (!img_cfg.user.trim().is_empty()).then(|| img_cfg.user.clone()));
    if let Some(u) = effective_user {
        spec.annotations.insert("coral.user".into(), u);
    }

    let ep_override = args.entrypoint.map(|s| {
        if s.trim().is_empty() {
            vec![String::new()]
        } else {
            vec![s]
        }
    });
    let resolved_args = resolve_entrypoint_and_cmd(&img_cfg, ep_override.as_deref(), &args.cmd);

    if let Some(proc_cfg) = spec.process.as_mut() {
        proc_cfg.terminal = args.tty;
        proc_cfg.args = resolved_args;
        if let Some(wd) = args.workdir
            && !wd.trim().is_empty()
        {
            proc_cfg.cwd = wd;
        } else if !img_cfg.working_dir.trim().is_empty() {
            proc_cfg.cwd = img_cfg.working_dir.clone();
        }
        proc_cfg.env = merge_env_vars(&proc_cfg.env, &img_cfg.env, &args.envs);
    }

    if (args.memory_mb > 0 || args.cpu_percent > 0 || args.pids_limit > 0)
        && let Some(linux) = spec.linux.as_mut()
    {
        let mut res = LinuxResources::default();
        if args.memory_mb > 0 {
            res.memory = Some(LinuxMemory {
                limit: Some(args.memory_mb * 1024 * 1024),
                reservation: None,
                swap: None,
            });
        }
        if args.cpu_percent > 0 {
            res.cpu = Some(LinuxCpu {
                shares: Some(1024),
                quota: Some(args.cpu_percent * 1_000),
                period: Some(100_000),
            });
        }
        if args.pids_limit > 0 {
            res.pids = Some(LinuxPids {
                limit: args.pids_limit,
            });
        }
        linux.resources = Some(res);
    }

    let _proxies =
        ForkedPortProxies::start(&args.ports, is_bridge_net, state_root, &args.container_id)?;

    spec.to_file(bundle_dir.join("config.json"))
        .context("write temporary OCI config.json")?;

    let run_res = mgr.run(
        &args.container_id,
        &bundle_dir,
        &RunOptions {
            keep_state: args.keep,
            tty: args.tty,
            interactive: args.interactive,
        },
    );
    if !args.keep {
        let _ = images::remove_dir_force(&bundle_dir);
    }

    let exit = run_res.with_context(|| format!("run container '{}'", args.container_id))?;
    let code = u8::try_from(exit.exit_code.clamp(0, 255)).unwrap_or(1);
    Ok(ExitCode::from(code))
}

pub fn resolve_local_container_overlay(state_root: &Path, id: &str) -> Result<(PathBuf, PathBuf)> {
    let bundle_dir = if let Ok(st) = StateStore::new(state_root).load(id) {
        st.bundle
    } else if let Ok(st) = StateStore::new(state_root.join("daemon/runtime-state")).load(id) {
        st.bundle
    } else {
        let daemon_bundle = state_root.join("daemon/bundles").join(id);
        let cli_bundle = state_root.join(".cli-bundles").join(id);
        if daemon_bundle.join("config.json").is_file() {
            daemon_bundle
        } else if cli_bundle.join("config.json").is_file() {
            cli_bundle
        } else {
            anyhow::bail!(
                "container '{id}' not found in local runtime state or coral-daemon (tip: pass `--keep` when using local `coral run`, or run via `--daemon`)"
            );
        }
    };

    let spec = OciSpec::from_file(bundle_dir.join("config.json"))
        .with_context(|| format!("read OCI config.json from {}", bundle_dir.display()))?;
    let raw_root = spec
        .root
        .as_ref()
        .map(|r| r.path.as_str())
        .unwrap_or("rootfs");
    let lower_rootfs = if Path::new(raw_root).is_absolute() {
        PathBuf::from(raw_root)
    } else {
        bundle_dir.join(raw_root)
    };
    let upperdir = bundle_dir.join("overlay/upper");
    Ok((lower_rootfs, upperdir))
}

pub fn commit_local_container(
    state_root: &Path,
    images_dir: &Path,
    container_id: &str,
    new_image: &str,
) -> Result<()> {
    let (lower_rootfs, upperdir) = resolve_local_container_overlay(state_root, container_id)?;
    let slug: String = new_image
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dest_rootfs = images_dir.join(&slug).join("rootfs");
    commit_overlay_to_rootfs(&lower_rootfs, &upperdir, &dest_rootfs).with_context(|| {
        format!(
            "commit container '{container_id}' to '{}'",
            dest_rootfs.display()
        )
    })?;

    let abs_rootfs = fs::canonicalize(&dest_rootfs).unwrap_or(dest_rootfs);
    let size = images::dir_size(&abs_rootfs);
    let id = format!(
        "sha256:commit-{}-{}-{}",
        container_id,
        slug,
        chrono::Utc::now().timestamp_millis()
    );
    let rootfs_str = abs_rootfs.to_string_lossy().into_owned();
    let stored = images::StoredImage {
        id: id.clone(),
        repo_tags: vec![new_image.to_string()],
        size,
        rootfs_path: rootfs_str.clone(),
        managed: true,
    };

    let mut index = images::load_index(images_dir);
    let orphaned = images::upsert_image(&mut index, stored);
    for old in orphaned {
        let _ = images::cleanup_unused_managed_dir(images_dir, &index, &old);
    }
    images::save_index(images_dir, &index)?;
    println!(
        "Committed {container_id} -> {new_image}\n  ID:     {id}\n  Size:   {}\n  Rootfs: {rootfs_str}",
        catalog::format_bytes(size)
    );
    Ok(())
}
