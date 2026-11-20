//! Translates CRI `ContainerConfig` + `PodSandboxConfig` + Coral annotations into an OCI `OciSpec`.

use super::{CoralDaemonService, DaemonStore};
use crate::images::{self, OciImageConfig, merge_env_vars, resolve_entrypoint_and_cmd};
use coral_api::{ContainerConfig, PodSandboxConfig};
use coral_oci::{
    LinuxCpu, LinuxMemory, LinuxNamespace, LinuxNamespaceType, LinuxPids, LinuxResources, OciSpec,
};
use coral_runtime::rootfs::{parse_volume_spec, prepare_dns_and_hosts_mounts};
use std::fs;
use std::path::{Path, PathBuf};
use tonic::Status;

impl CoralDaemonService {
    #[allow(clippy::result_large_err)]
    pub(crate) fn resolve_rootfs_for_image(
        &self,
        store: &DaemonStore,
        image_ref: &str,
    ) -> Result<PathBuf, Status> {
        let trimmed = image_ref.trim();
        if let Some(img) = images::find_image(&store.images, trimmed) {
            return Ok(PathBuf::from(&img.rootfs_path));
        }

        if !trimmed.is_empty() {
            let direct = PathBuf::from(trimmed);
            if let Ok(abs) = fs::canonicalize(&direct)
                && abs.is_dir()
            {
                return Ok(abs);
            }
        } else if let Some(default_rootfs) = &self.cfg.default_rootfs
            && let Ok(abs) = fs::canonicalize(default_rootfs)
            && abs.is_dir()
        {
            return Ok(abs);
        }

        Err(Status::failed_precondition(format!(
            "image '{image_ref}' is not installed — pull it first or register a local rootfs"
        )))
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn build_oci_spec(
        &self,
        cfg: &ContainerConfig,
        sandbox: &PodSandboxConfig,
        rootfs: &Path,
        bundle_dir: &Path,
    ) -> Result<OciSpec, Status> {
        let host_uid = unsafe { libc::geteuid() };
        let host_gid = unsafe { libc::getegid() };
        let rootless = host_uid != 0;

        let mut spec = OciSpec::default_spec(rootless, host_uid, host_gid);
        if let Some(root) = spec.root.as_mut() {
            root.path = rootfs.to_string_lossy().into_owned();
        }
        if !sandbox.hostname.trim().is_empty() {
            spec.hostname = Some(sandbox.hostname.clone());
        } else if let Some(meta) = &sandbox.metadata
            && !meta.name.trim().is_empty()
        {
            spec.hostname = Some(meta.name.clone());
        }

        let img_cfg = OciImageConfig::load_for_rootfs(rootfs);

        let ep_override: Option<Vec<String>> = cfg
            .annotations
            .get("coral.entrypoint")
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .or_else(|| {
                cfg.annotations
                    .get("coral.entrypoint")
                    .map(|s| vec![s.clone()])
            });

        let mut user_cmd = Vec::new();
        user_cmd.extend(cfg.command.iter().cloned());
        user_cmd.extend(cfg.args.iter().cloned());
        let final_args =
            resolve_entrypoint_and_cmd(&img_cfg, ep_override.as_deref(), &user_cmd);

        let user_envs: Vec<String> = cfg
            .envs
            .iter()
            .map(|kv| format!("{}={}", kv.key, kv.value))
            .collect();

        if let Some(proc_cfg) = spec.process.as_mut() {
            proc_cfg.args = final_args;
            if !cfg.working_dir.trim().is_empty() {
                proc_cfg.cwd = cfg.working_dir.clone();
            } else if !img_cfg.working_dir.trim().is_empty() {
                proc_cfg.cwd = img_cfg.working_dir.clone();
            }
            proc_cfg.env = merge_env_vars(&proc_cfg.env, &img_cfg.env, &user_envs);
        }

        if !cfg.annotations.contains_key("coral.user") && !img_cfg.user.trim().is_empty() {
            spec.annotations
                .insert("coral.user".into(), img_cfg.user.clone());
        }

        let custom_dns: Vec<String> = cfg
            .annotations
            .get("coral.dns")
            .map(|raw| {
                raw.split(['\n', ','])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let extra_hosts: Vec<String> = cfg
            .annotations
            .get("coral.extra_hosts")
            .map(|raw| {
                raw.split(['\n', ','])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        // Network mode:
        // - "none": isolated CLONE_NEWNET (loopback only, no DNS/hosts mounts)
        // - "host" (default): shared host network + DNS/hosts
        // - "bridge" / "isolated" / <custom-network-name>: isolated CLONE_NEWNET per container + DNS/hosts + cross-netns port forwarding
        let net_mode = cfg
            .annotations
            .get("coral.network")
            .map(String::as_str)
            .unwrap_or("host");
        if net_mode.eq_ignore_ascii_case("none") {
            if let Some(spec_linux) = spec.linux.as_mut() {
                spec_linux.namespaces.push(LinuxNamespace {
                    typ: LinuxNamespaceType::Network,
                    path: None,
                });
            }
        } else if !net_mode.eq_ignore_ascii_case("host") {
            if let Some(spec_linux) = spec.linux.as_mut() {
                spec_linux.namespaces.push(LinuxNamespace {
                    typ: LinuxNamespaceType::Network,
                    path: None,
                });
            }
            let dns_mounts = prepare_dns_and_hosts_mounts(
                bundle_dir,
                spec.hostname.as_deref().unwrap_or("coral"),
                &custom_dns,
                &extra_hosts,
            )
            .map_err(|e| Status::invalid_argument(format!("prepare DNS/hosts mounts: {e}")))?;
            spec.mounts.extend(dns_mounts);
        } else {
            let dns_mounts = prepare_dns_and_hosts_mounts(
                bundle_dir,
                spec.hostname.as_deref().unwrap_or("coral"),
                &custom_dns,
                &extra_hosts,
            )
            .map_err(|e| Status::invalid_argument(format!("prepare DNS/hosts mounts: {e}")))?;
            spec.mounts.extend(dns_mounts);
        }

        // Bind or Named volume mounts passed via `coral.mounts` annotation (newline or comma separated).
        if let Some(raw_mounts) = cfg.annotations.get("coral.mounts") {
            for line in raw_mounts.split(['\n', ',']) {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let mount = parse_volume_spec(trimmed)
                    .map_err(|e| Status::invalid_argument(format!("invalid volume mount: {e}")))?;
                spec.mounts.push(mount);
            }
        }

        if let Some(linux_cfg) = &cfg.linux
            && let Some(res) = &linux_cfg.resources
            && let Some(spec_linux) = spec.linux.as_mut()
        {
            let mut resources = LinuxResources::default();
            if res.memory_limit_in_bytes > 0 {
                resources.memory = Some(LinuxMemory {
                    limit: Some(res.memory_limit_in_bytes),
                    reservation: None,
                    swap: None,
                });
            }
            if res.cpu_quota > 0 || res.cpu_period > 0 || res.cpu_shares > 0 {
                resources.cpu = Some(LinuxCpu {
                    shares: (res.cpu_shares > 0).then_some(res.cpu_shares as u64),
                    quota: (res.cpu_quota > 0).then_some(res.cpu_quota),
                    period: (res.cpu_period > 0).then_some(res.cpu_period as u64),
                });
            }
            if res.pids_limit > 0 {
                resources.pids = Some(LinuxPids {
                    limit: res.pids_limit,
                });
            }
            spec_linux.resources = Some(resources);
        }

        // Enable Copy-on-Write OverlayFS isolation per container by default so shared base images stay clean.
        spec.annotations.insert("coral.overlay".into(), "true".into());
        for (k, v) in &cfg.annotations {
            spec.annotations.insert(k.clone(), v.clone());
        }

        Ok(spec)
    }
}
