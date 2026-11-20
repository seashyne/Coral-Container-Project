//! MicroVM Guest Linux Kernel Manager (`coral vm setup-kernel`).
//!
//! Fetches and extracts an official virtualization-optimized Alpine `linux-virt` bzImage
//! kernel (no root privileges, no QEMU/Firecracker required) into `~/.local/state/coral/vmm/`,
//! or installs a user-supplied `bzImage` kernel after verifying its `HdrS` boot header.

use crate::initramfs::{BzImageInfo, inspect_bzimage};
use anyhow::{Context, Result, bail};
use flate2::read::{GzDecoder, MultiGzDecoder};
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

/// Companion kernel modules directory next to `kernel_path` (`<dir>/modules`).
pub fn default_modules_dir(kernel_path: &Path) -> PathBuf {
    kernel_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("modules")
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

/// Extracts `boot/vmlinuz-virt` (or any `boot/vmlinuz*`) plus VirtIO + ext4 loadable
/// kernel modules (`.ko.gz` -> `.ko`) out of an Alpine Linux `.apk` archive into `dest`.
pub fn extract_bzimage_from_apk(apk_bytes: &[u8], dest: &Path) -> Result<BzImageInfo> {
    const WANTED_MODULES: &[&str] = &[
        "virtio_blk.ko.gz",
        "virtio_net.ko.gz",
        "net_failover.ko.gz",
        "failover.ko.gz",
        "crc16.ko.gz",
        "mbcache.ko.gz",
        "jbd2.ko.gz",
        "libcrc32c.ko.gz",
        "crc32c_generic.ko.gz",
        "ext4.ko.gz",
    ];

    let gz = MultiGzDecoder::new(apk_bytes);
    let mut archive = tar::Archive::new(gz);
    let mod_dir = default_modules_dir(dest);
    let mut kernel_found = false;

    for entry in archive.entries().context("iterate Alpine kernel APK entries")? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        let norm = path.trim_start_matches("./").trim_start_matches('/');
        if !kernel_found && norm.starts_with("boot/vmlinuz") {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(dest, &buf)
                .with_context(|| format!("write extracted kernel '{}'", dest.display()))?;
            kernel_found = true;
        } else if let Some(fname) = Path::new(norm).file_name().and_then(|s| s.to_str())
            && WANTED_MODULES.contains(&fname)
        {
            fs::create_dir_all(&mod_dir)?;
            let mut compressed = Vec::new();
            entry.read_to_end(&mut compressed)?;
            let mut dec = GzDecoder::new(compressed.as_slice());
            let mut ko_bytes = Vec::new();
            dec.read_to_end(&mut ko_bytes)
                .with_context(|| format!("decompress kernel module '{fname}'"))?;
            let ko_name = fname.trim_end_matches(".gz");
            fs::write(mod_dir.join(ko_name), &ko_bytes)?;
        }
    }
    if !kernel_found {
        bail!("Alpine APK did not contain a boot/vmlinuz* kernel binary");
    }
    inspect_bzimage(dest)
}

/// Downloads the official Alpine Linux 3.20 `linux-virt` kernel APK over HTTPS,
/// extracts `boot/vmlinuz-virt` and VirtIO/ext4 `.ko` modules into `dest`.
pub fn setup_alpine_microvm_kernel(dest: &Path) -> Result<KernelInstallReport> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture '{other}' for Alpine linux-virt kernel"),
    };
    let mirrors = [
        "https://dl-cdn.alpinelinux.org",
        "https://uk.alpinelinux.org",
        "https://nl.alpinelinux.org",
    ];
    let mut last_err = None;
    for base in mirrors {
        let index_url = format!("{base}/alpine/v3.20/main/{arch}/");
        let Ok(index_html) = fetch_url_bytes(&index_url) else {
            continue;
        };
        let html_str = String::from_utf8_lossy(&index_html);
        let Some(pkg_name) = html_str
            .split('"')
            .find(|s| s.starts_with("linux-virt-") && s.ends_with(".apk"))
        else {
            continue;
        };
        let apk_url = format!("{index_url}{pkg_name}");
        match fetch_url_bytes(&apk_url) {
            Ok(apk_bytes) => {
                let info = extract_bzimage_from_apk(&apk_bytes, dest)?;
                return Ok(report_from_info(dest, &info, apk_url));
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("failed to download Alpine linux-virt APK from all mirrors")))
}

fn fetch_url_bytes(url: &str) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "--http1.1",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "--connect-timeout",
            "15",
            "--max-time",
            "180",
            url,
        ])
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
