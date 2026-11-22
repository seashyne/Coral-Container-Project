//! CNCF Container Device Interface (CDI) JSON spec resolver and host NVIDIA driver
//! `.so` / `nvidia-smi` read-only bind-mount injection for `--gpus`.

use coral_oci::Mount;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Standard CNCF CDI specification search directories.
pub const DEFAULT_CDI_DIRS: &[&str] = &["/etc/cdi", "/var/run/cdi"];

/// Standard host library directories searched for NVIDIA driver `.so` files.
pub const DEFAULT_NVIDIA_LIB_DIRS: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu",
    "/usr/lib/aarch64-linux-gnu",
    "/usr/lib64",
    "/usr/lib",
    "/lib/x86_64-linux-gnu",
    "/lib/aarch64-linux-gnu",
];

/// Standard host binary directories searched for NVIDIA utility binaries.
pub const DEFAULT_NVIDIA_BIN_DIRS: &[&str] = &["/usr/bin", "/usr/local/bin"];

/// Core NVIDIA user-space driver shared library prefixes injected into GPU containers.
const NVIDIA_LIB_PREFIXES: &[&str] = &[
    "libcuda.so",
    "libnvidia-ml.so",
    "libnvidia-ptxjitcompiler.so",
    "libnvidia-nvvm.so",
    "libnvidia-opencl.so",
    "libnvidia-cfg.so",
    "libnvcuvid.so",
    "libnvidia-encode.so",
    "libcudadebugger.so",
];

const NVIDIA_BINS: &[&str] = &["nvidia-smi"];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiSpec {
    kind: String,
    #[serde(default)]
    devices: Vec<CdiDevice>,
    #[serde(default)]
    container_edits: Option<CdiContainerEdits>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiDevice {
    name: String,
    #[serde(default)]
    container_edits: Option<CdiContainerEdits>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiContainerEdits {
    #[serde(default)]
    env: Vec<String>,
    #[serde(default)]
    device_nodes: Vec<CdiDeviceNode>,
    #[serde(default)]
    mounts: Vec<CdiMount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiDeviceNode {
    path: String,
    #[serde(default)]
    host_path: Option<String>,
    #[serde(default)]
    permissions: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdiMount {
    host_path: String,
    container_path: String,
    #[serde(default)]
    options: Vec<String>,
}

/// Returns `true` when `spec` looks like a CNCF CDI qualified device name
/// (`[cdi=]<vendor>/<class>=<name>`, e.g. `nvidia.com/gpu=all`).
pub fn is_cdi_spec(spec: &str) -> bool {
    let trimmed = spec.trim().strip_prefix("cdi=").unwrap_or(spec.trim());
    if let Some((kind, name)) = trimmed.split_once('=') {
        kind.contains('/') && !name.trim().is_empty()
    } else {
        false
    }
}

/// Resolves a CNCF CDI qualified device spec (`nvidia.com/gpu=all`, `nvidia.com/gpu=0`, etc.)
/// across `cdi_dirs`, returning `(mounts, envs)` if a matching spec & device was found.
pub fn resolve_cdi_device_in(cdi_dirs: &[&Path], raw_spec: &str) -> Option<(Vec<Mount>, Vec<String>)> {
    let clean = raw_spec.trim().strip_prefix("cdi=").unwrap_or(raw_spec.trim());
    let (wanted_kind, wanted_name) = clean.split_once('=')?;
    let wanted_kind = wanted_kind.trim();
    let wanted_name = wanted_name.trim();
    if wanted_kind.is_empty() || wanted_name.is_empty() {
        return None;
    }

    let mut mounts = Vec::new();
    let mut envs = Vec::new();
    let mut seen_dests: HashSet<String> = HashSet::new();
    let mut matched = false;

    for dir in cdi_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();

        for file in files {
            let Ok(raw) = fs::read_to_string(&file) else {
                continue;
            };
            let Ok(spec) = serde_json::from_str::<CdiSpec>(&raw) else {
                continue;
            };
            if !spec.kind.eq_ignore_ascii_case(wanted_kind) {
                continue;
            }
            let dev = spec
                .devices
                .iter()
                .find(|d| d.name.eq_ignore_ascii_case(wanted_name));
            let Some(dev) = dev else {
                continue;
            };
            matched = true;
            if let Some(global_edits) = &spec.container_edits {
                apply_cdi_edits(global_edits, &mut mounts, &mut envs, &mut seen_dests);
            }
            if let Some(dev_edits) = &dev.container_edits {
                apply_cdi_edits(dev_edits, &mut mounts, &mut envs, &mut seen_dests);
            }
        }
    }

    if matched { Some((mounts, envs)) } else { None }
}

/// Resolves a CNCF CDI qualified device spec from standard host directories (`/etc/cdi`, `/var/run/cdi`).
pub fn resolve_cdi_device(raw_spec: &str) -> Option<(Vec<Mount>, Vec<String>)> {
    let dirs: Vec<&Path> = DEFAULT_CDI_DIRS.iter().map(Path::new).collect();
    resolve_cdi_device_in(&dirs, raw_spec)
}

fn apply_cdi_edits(
    edits: &CdiContainerEdits,
    mounts: &mut Vec<Mount>,
    envs: &mut Vec<String>,
    seen_dests: &mut HashSet<String>,
) {
    for env in &edits.env {
        if !env.trim().is_empty() && !envs.contains(env) {
            envs.push(env.clone());
        }
    }
    for node in &edits.device_nodes {
        let host = node.host_path.as_deref().unwrap_or(&node.path);
        let host_p = Path::new(host);
        if !host_p.exists() || !seen_dests.insert(node.path.clone()) {
            continue;
        }
        let abs = fs::canonicalize(host_p).unwrap_or_else(|_| host_p.to_path_buf());
        let mode = match node.permissions.as_deref() {
            Some(p) if p.contains('r') && !p.contains('w') => "ro",
            _ => "rw",
        };
        mounts.push(Mount {
            destination: node.path.clone(),
            typ: Some("bind".into()),
            source: Some(abs.to_string_lossy().into_owned()),
            options: vec!["bind".into(), mode.into()],
        });
    }
    for m in &edits.mounts {
        let host_p = Path::new(&m.host_path);
        if !host_p.exists() || !seen_dests.insert(m.container_path.clone()) {
            continue;
        }
        let abs = fs::canonicalize(host_p).unwrap_or_else(|_| host_p.to_path_buf());
        let mut opts = if m.options.is_empty() {
            vec!["bind".into(), "ro".into()]
        } else {
            m.options.clone()
        };
        if !opts.iter().any(|o| o == "bind" || o == "rbind") {
            opts.insert(0, "bind".into());
        }
        mounts.push(Mount {
            destination: m.container_path.clone(),
            typ: Some("bind".into()),
            source: Some(abs.to_string_lossy().into_owned()),
            options: opts,
        });
    }
}

/// Scans `lib_dirs` and `bin_dirs` for host NVIDIA user-space driver `.so` libraries and
/// `nvidia-smi`, returning read-only bind mounts and the container `LD_LIBRARY_PATH` directories.
pub fn discover_nvidia_driver_mounts_in(
    lib_dirs: &[&Path],
    bin_dirs: &[&Path],
) -> (Vec<Mount>, Vec<String>) {
    let mut mounts = Vec::new();
    let mut ld_dirs = Vec::new();
    let mut seen_dests: HashSet<String> = HashSet::new();

    for dir in lib_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| {
                NVIDIA_LIB_PREFIXES
                    .iter()
                    .any(|p| name == p || name.starts_with(&format!("{p}.")))
            })
            .collect();
        names.sort();

        let mut dir_had_match = false;
        for name in names {
            let entry_path = dir.join(&name);
            let Ok(abs) = fs::canonicalize(&entry_path) else {
                continue;
            };
            if !abs.is_file() {
                continue;
            }
            let dest = dir.join(&name).to_string_lossy().into_owned();
            if seen_dests.insert(dest.clone()) {
                mounts.push(Mount {
                    destination: dest,
                    typ: Some("bind".into()),
                    source: Some(abs.to_string_lossy().into_owned()),
                    options: vec!["bind".into(), "ro".into()],
                });
                dir_had_match = true;
            }
        }
        if dir_had_match {
            let dir_str = dir.to_string_lossy().into_owned();
            if !ld_dirs.contains(&dir_str) {
                ld_dirs.push(dir_str);
            }
        }
    }

    for dir in bin_dirs {
        for bin in NVIDIA_BINS {
            let p = dir.join(bin);
            if let Ok(abs) = fs::canonicalize(&p)
                && abs.is_file()
            {
                let dest = format!("/usr/bin/{bin}");
                if seen_dests.insert(dest.clone()) {
                    mounts.push(Mount {
                        destination: dest,
                        typ: Some("bind".into()),
                        source: Some(abs.to_string_lossy().into_owned()),
                        options: vec!["bind".into(), "ro".into()],
                    });
                }
            }
        }
    }

    (mounts, ld_dirs)
}

/// Scans standard host directories for NVIDIA driver `.so` libraries and `nvidia-smi`.
pub fn discover_nvidia_driver_mounts() -> (Vec<Mount>, Vec<String>) {
    let lib_dirs: Vec<&Path> = DEFAULT_NVIDIA_LIB_DIRS.iter().map(Path::new).collect();
    let bin_dirs: Vec<&Path> = DEFAULT_NVIDIA_BIN_DIRS.iter().map(Path::new).collect();
    discover_nvidia_driver_mounts_in(&lib_dirs, &bin_dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn resolves_cncf_cdi_json_spec_mounts_devices_and_envs() {
        let dir = tempdir().unwrap();
        let fake_lib = dir.path().join("libcuda.so.550.54");
        let fake_dev = dir.path().join("nvidia0");
        fs::write(&fake_lib, b"elf-cuda").unwrap();
        fs::write(&fake_dev, b"").unwrap();

        let cdi_json = serde_json::json!({
            "cdiVersion": "0.6.0",
            "kind": "nvidia.com/gpu",
            "containerEdits": {
                "env": ["NVIDIA_DRIVER_CAPABILITIES=all"],
                "mounts": [{
                    "hostPath": fake_lib.to_string_lossy(),
                    "containerPath": "/usr/lib/x86_64-linux-gnu/libcuda.so.1",
                    "options": ["ro", "nosuid", "nodev", "bind"]
                }]
            },
            "devices": [{
                "name": "all",
                "containerEdits": {
                    "env": ["NVIDIA_VISIBLE_DEVICES=all"],
                    "deviceNodes": [{
                        "path": "/dev/nvidia0",
                        "hostPath": fake_dev.to_string_lossy(),
                        "permissions": "rw"
                    }]
                }
            }]
        });
        fs::write(
            dir.path().join("nvidia.json"),
            serde_json::to_string_pretty(&cdi_json).unwrap(),
        )
        .unwrap();

        assert!(is_cdi_spec("nvidia.com/gpu=all"));
        assert!(is_cdi_spec("cdi=nvidia.com/gpu=all"));
        assert!(!is_cdi_spec("all"));

        let (mounts, envs) = resolve_cdi_device_in(&[dir.path()], "cdi=nvidia.com/gpu=all").unwrap();
        assert!(envs.contains(&"NVIDIA_VISIBLE_DEVICES=all".to_string()));
        assert!(envs.contains(&"NVIDIA_DRIVER_CAPABILITIES=all".to_string()));
        assert!(mounts.iter().any(|m| {
            m.destination == "/usr/lib/x86_64-linux-gnu/libcuda.so.1"
                && m.options.contains(&"ro".to_string())
        }));
        assert!(mounts.iter().any(|m| m.destination == "/dev/nvidia0"));
    }

    #[test]
    fn discovers_host_nvidia_driver_libs_and_nvidia_smi_read_only() {
        let dir = tempdir().unwrap();
        let lib_dir = dir.path().join("lib");
        let bin_dir = dir.path().join("bin");
        fs::create_dir_all(&lib_dir).unwrap();
        fs::create_dir_all(&bin_dir).unwrap();

        fs::write(lib_dir.join("libcuda.so.550.54"), b"cuda").unwrap();
        std::os::unix::fs::symlink(
            lib_dir.join("libcuda.so.550.54"),
            lib_dir.join("libcuda.so.1"),
        )
        .unwrap();
        fs::write(lib_dir.join("libnvidia-ml.so.1"), b"nvml").unwrap();
        fs::write(lib_dir.join("libunrelated.so.1"), b"other").unwrap();
        fs::write(bin_dir.join("nvidia-smi"), b"#!/bin/sh").unwrap();

        let (mounts, ld_dirs) =
            discover_nvidia_driver_mounts_in(&[lib_dir.as_path()], &[bin_dir.as_path()]);
        assert_eq!(ld_dirs.len(), 1);
        assert!(mounts.iter().any(|m| {
            m.destination.ends_with("libcuda.so.1") && m.options.contains(&"ro".to_string())
        }));
        assert!(mounts.iter().any(|m| m.destination.ends_with("libnvidia-ml.so.1")));
        assert!(mounts.iter().any(|m| m.destination == "/usr/bin/nvidia-smi"));
        assert!(!mounts.iter().any(|m| m.destination.contains("libunrelated")));
    }
}
