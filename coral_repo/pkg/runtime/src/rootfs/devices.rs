//! Hardware device passthrough (`--device /dev/xyz[:/dev/xyz[:rwm]]`) and
//! GPU device discovery (`--gpus all` for NVIDIA `/dev/nvidia*`, AMD `/dev/kfd`, and DRM `/dev/dri/*`).

use crate::RuntimeError;
use coral_oci::Mount;
use std::fs;
use std::path::Path;

/// Parses a Docker-compatible `--device HOST_DEV[:CONTAINER_DEV[:PERMS]]` specification
/// into an OCI bind `Mount`.
pub fn parse_device_mount(raw: &str) -> Result<Mount, RuntimeError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RuntimeError::InitFailed("empty --device specification".into()));
    }
    let parts: Vec<&str> = trimmed.split(':').collect();
    let (host_dev, ctr_dev, perms) = match parts.as_slice() {
        [h] => (*h, *h, "rwm"),
        [h, c] => (*h, *c, "rwm"),
        [h, c, p] => (*h, *c, *p),
        _ => {
            return Err(RuntimeError::InitFailed(format!(
                "invalid --device specification '{trimmed}' (expected HOST_DEV[:CTR_DEV[:PERMS]])"
            )));
        }
    };

    let host_dev = host_dev.trim();
    let ctr_dev = ctr_dev.trim();
    if !host_dev.starts_with('/') || !ctr_dev.starts_with('/') {
        return Err(RuntimeError::InitFailed(format!(
            "--device paths must be absolute in '{trimmed}'"
        )));
    }
    let host_path = Path::new(host_dev);
    if !host_path.exists() {
        return Err(RuntimeError::InitFailed(format!(
            "host device '{}' does not exist",
            host_dev
        )));
    }
    let abs_host = fs::canonicalize(host_path).unwrap_or_else(|_| host_path.to_path_buf());
    let mode = if perms.contains('r') && !perms.contains('w') {
        "ro"
    } else {
        "rw"
    };

    Ok(Mount {
        destination: ctr_dev.to_string(),
        typ: Some("bind".to_string()),
        source: Some(abs_host.to_string_lossy().into_owned()),
        options: vec!["bind".to_string(), mode.to_string()],
    })
}

/// Discovers GPU device nodes present under `dev_root` (typically `/dev`) for `--gpus <all|index>`
/// and returns `(gpu_bind_mounts, gpu_env_vars)`.
pub fn discover_gpu_mounts_and_envs_in(
    dev_root: &Path,
    gpus_spec: &str,
) -> Result<(Vec<Mount>, Vec<String>), RuntimeError> {
    let spec = gpus_spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") || spec == "0" {
        return Ok((Vec::new(), Vec::new()));
    }

    // 0. CNCF Container Device Interface (CDI) spec (`--gpus nvidia.com/gpu=all` or `cdi=...`).
    if super::gpu_cdi::is_cdi_spec(spec)
        && let Some((cdi_mounts, cdi_envs)) = super::gpu_cdi::resolve_cdi_device(spec)
    {
        return Ok((cdi_mounts, cdi_envs));
    }

    let wanted_index: Option<u32> = if spec.eq_ignore_ascii_case("all") {
        None
    } else if let Some(idx_str) = spec.strip_prefix("device=") {
        idx_str.trim().parse::<u32>().ok()
    } else {
        spec.parse::<u32>().ok()
    };

    let mut mounts = Vec::new();
    let mut add_if_exists = |rel: &str| {
        let host_p = dev_root.join(rel);
        if host_p.exists() {
            let abs = fs::canonicalize(&host_p).unwrap_or(host_p);
            mounts.push(Mount {
                destination: format!("/dev/{rel}"),
                typ: Some("bind".to_string()),
                source: Some(abs.to_string_lossy().into_owned()),
                options: vec!["bind".to_string(), "rw".to_string()],
            });
        }
    };

    // 1. NVIDIA control nodes + GPU device nodes
    for ctrl in [
        "nvidiactl",
        "nvidia-uvm",
        "nvidia-uvm-tools",
        "nvidia-modeset",
        "nvidia-caps",
    ] {
        add_if_exists(ctrl);
    }
    if let Some(idx) = wanted_index {
        add_if_exists(&format!("nvidia{idx}"));
    } else {
        for idx in 0..16 {
            add_if_exists(&format!("nvidia{idx}"));
        }
    }

    // 2. AMD ROCm (`/dev/kfd`) and Linux Direct Rendering Manager (`/dev/dri`)
    add_if_exists("kfd");
    add_if_exists("dri");

    let visible = wanted_index
        .map(|i| i.to_string())
        .unwrap_or_else(|| "all".to_string());
    let mut envs = vec![
        format!("NVIDIA_VISIBLE_DEVICES={visible}"),
        "NVIDIA_DRIVER_CAPABILITIES=compute,utility".to_string(),
        format!("ROCR_VISIBLE_DEVICES={visible}"),
    ];

    // 3. NVIDIA Container Toolkit driver injection: prefer CNCF CDI spec if present on host,
    //    otherwise auto-bind host `libcuda.so*`, `libnvidia-ml.so*`, and `nvidia-smi` read-only.
    let has_nvidia_nodes = mounts.iter().any(|m| m.destination.starts_with("/dev/nvidia"));
    if has_nvidia_nodes {
        let cdi_query = format!("nvidia.com/gpu={visible}");
        if let Some((cdi_mounts, cdi_envs)) = super::gpu_cdi::resolve_cdi_device(&cdi_query) {
            for m in cdi_mounts {
                if !mounts.iter().any(|e| e.destination == m.destination) {
                    mounts.push(m);
                }
            }
            for e in cdi_envs {
                if !envs.contains(&e) {
                    envs.push(e);
                }
            }
        } else {
            let (drv_mounts, ld_dirs) = super::gpu_cdi::discover_nvidia_driver_mounts();
            for m in drv_mounts {
                if !mounts.iter().any(|e| e.destination == m.destination) {
                    mounts.push(m);
                }
            }
            if !ld_dirs.is_empty() {
                envs.push(format!("LD_LIBRARY_PATH={}", ld_dirs.join(":")));
            }
        }
    }

    Ok((mounts, envs))
}

/// Discovers GPU device nodes under `/dev` for `--gpus <spec>`.
pub fn discover_gpu_mounts_and_envs(
    gpus_spec: &str,
) -> Result<(Vec<Mount>, Vec<String>), RuntimeError> {
    discover_gpu_mounts_and_envs_in(Path::new("/dev"), gpus_spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_device_mount_and_discovers_mock_gpu_nodes() {
        let m = parse_device_mount("/dev/null:/dev/custom-null:ro").unwrap();
        assert_eq!(m.destination, "/dev/custom-null");
        assert!(m.options.contains(&"ro".to_string()));

        let fake_dev = tempdir().unwrap();
        fs::write(fake_dev.path().join("nvidiactl"), b"").unwrap();
        fs::write(fake_dev.path().join("nvidia0"), b"").unwrap();
        fs::create_dir_all(fake_dev.path().join("dri")).unwrap();

        let (mounts, envs) = discover_gpu_mounts_and_envs_in(fake_dev.path(), "all").unwrap();
        let dests: Vec<&str> = mounts.iter().map(|m| m.destination.as_str()).collect();
        assert!(dests.contains(&"/dev/nvidiactl"));
        assert!(dests.contains(&"/dev/nvidia0"));
        assert!(dests.contains(&"/dev/dri"));
        assert!(envs.contains(&"NVIDIA_VISIBLE_DEVICES=all".to_string()));
    }
}
