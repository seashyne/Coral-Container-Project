//! MicroVM Guest Linux Kernel Manager (`coral vm setup-kernel`).
//!
//! Fetches and extracts an official virtualization-optimized Alpine `linux-virt` bzImage
//! kernel (no root privileges, no QEMU/Firecracker required) into `~/.local/state/coral/vmm/`,
//! or installs a user-supplied `bzImage` kernel after verifying its `HdrS` boot header.

use crate::initramfs::{BzImageInfo, inspect_bzimage};
use anyhow::{Context, Result, bail};
use flate2::read::MultiGzDecoder;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Result returned when installing or inspecting Coral's MicroVM guest Linux kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelInstallReport {
    pub kernel_path: String,
    pub size_bytes: u64,
    pub protocol_version: String,
    pub source: String,
}

/// Default location of Coral's guest Linux MicroVM kernel (`$CORAL_VMM_KERNEL` or XDG state dir).
pub fn default_kernel_path() -> PathBuf {
    if let Ok(p) = std::env::var("CORAL_VMM_KERNEL")
        && !p.trim().is_empty()
    {
        return PathBuf::from(p);
    }
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("coral/vmm/vmlinuz-virt")
}

/// Copies and validates a local Linux x86-64 `bzImage` kernel into `dest`.
pub fn install_kernel_from_file(src: &Path, dest: &Path) -> Result<KernelInstallReport> {
    let info = inspect_bzimage(src)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if src != dest {
        fs::copy(src, dest)
            .with_context(|| format!("copy kernel '{}' -> '{}'", src.display(), dest.display()))?;
    }
    Ok(report_from_info(dest, &info, src.display().to_string()))
}

/// Extracts `boot/vmlinuz-virt` (or any `boot/vmlinuz*`) out of an Alpine Linux `.apk`
/// archive (concatenated gzipped tar streams) into `dest` and validates its `HdrS` signature.
pub fn extract_bzimage_from_apk(apk_bytes: &[u8], dest: &Path) -> Result<BzImageInfo> {
    let gz = MultiGzDecoder::new(apk_bytes);
    let mut archive = tar::Archive::new(gz);
    for entry in archive.entries().context("iterate Alpine kernel APK entries")? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let norm = path.trim_start_matches("./").trim_start_matches('/');
        if norm.starts_with("boot/vmlinuz") && entry.header().entry_type().is_file() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(dest, &buf)
                .with_context(|| format!("write extracted kernel '{}'", dest.display()))?;
            return inspect_bzimage(dest);
        }
    }
    bail!("Alpine APK did not contain a boot/vmlinuz* kernel binary")
}

/// Downloads the official Alpine Linux 3.20 `linux-virt` kernel APK over HTTPS,
/// extracts `boot/vmlinuz-virt` into `dest`, and verifies its Linux boot header.
pub fn setup_alpine_microvm_kernel(dest: &Path) -> Result<KernelInstallReport> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture '{other}' for Alpine linux-virt kernel"),
    };
    let index_url = format!("https://dl-cdn.alpinelinux.org/alpine/v3.20/main/{arch}/");
    let index_html = fetch_url_bytes(&index_url)
        .with_context(|| format!("fetch Alpine package index from {index_url}"))?;
    let html_str = String::from_utf8_lossy(&index_html);

    let pkg_name = html_str
        .split('"')
        .find(|s| s.starts_with("linux-virt-") && s.ends_with(".apk"))
        .ok_or_else(|| anyhow::anyhow!("could not locate linux-virt-*.apk in {index_url}"))?;

    let apk_url = format!("{index_url}{pkg_name}");
    let apk_bytes = fetch_url_bytes(&apk_url)
        .with_context(|| format!("download Alpine kernel package {apk_url}"))?;

    let info = extract_bzimage_from_apk(&apk_bytes, dest)?;
    Ok(report_from_info(dest, &info, apk_url))
}

fn fetch_url_bytes(url: &str) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .with_context(|| format!("spawn curl to fetch {url}"))?;
    if !out.status.success() {
        bail!(
            "curl failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn report_from_info(dest: &Path, info: &BzImageInfo, source: String) -> KernelInstallReport {
    KernelInstallReport {
        kernel_path: dest.display().to_string(),
        size_bytes: info.total_size as u64,
        protocol_version: format!("{}.{:02}", info.protocol_major, info.protocol_minor),
        source,
    }
}
