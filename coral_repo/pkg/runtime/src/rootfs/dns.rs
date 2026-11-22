//! Per-container `/etc/resolv.conf` (`--dns`) and `/etc/hosts` (`--add-host` + hostname) synthesis.

use crate::RuntimeError;
use coral_oci::Mount;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Returns read-only OCI bind mounts for host DNS (`/etc/resolv.conf`) and `/etc/hosts`
/// so containers sharing the host network namespace can resolve external domains.
pub fn host_dns_mounts() -> Vec<Mount> {
    let mut mounts = Vec::new();
    for path in ["/etc/resolv.conf", "/etc/hosts"] {
        if let Ok(abs) = fs::canonicalize(path)
            && abs.is_file()
        {
            mounts.push(Mount {
                destination: path.to_string(),
                typ: Some("bind".to_string()),
                source: Some(abs.to_string_lossy().into_owned()),
                options: vec!["bind".to_string(), "ro".to_string()],
            });
        }
    }
    mounts
}

/// Generates per-container `/etc/hosts` (including `127.0.0.1 localhost <hostname>` and any
/// `--add-host HOST:IP` entries) and `/etc/resolv.conf` (using `--dns IP` servers if provided,
/// or falling back to the host's `/etc/resolv.conf`) inside `bundle_dir`, returning their OCI mounts.
pub fn prepare_dns_and_hosts_mounts(
    bundle_dir: &Path,
    hostname: &str,
    custom_dns: &[String],
    extra_hosts: &[String],
) -> Result<Vec<Mount>, RuntimeError> {
    fs::create_dir_all(bundle_dir)
        .map_err(|e| RuntimeError::sys("create bundle dir for DNS files", e))?;
    let mut mounts = Vec::new();

    // 1. Always synthesize a clean `/etc/hosts` containing the container's own hostname + extra_hosts.
    let hosts_file = bundle_dir.join("coral-hosts");
    let safe_host = if hostname.trim().is_empty() {
        "coral-container"
    } else {
        hostname.trim()
    };
    let mut hosts_content = format!(
        "127.0.0.1\tlocalhost {safe_host}\n::1\tlocalhost ip6-localhost ip6-loopback {safe_host}\n"
    );
    for entry in extra_hosts {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((host, ip)) = trimmed.split_once(':') else {
            return Err(RuntimeError::InitFailed(format!(
                "invalid --add-host '{trimmed}' (expected HOST:IP)"
            )));
        };
        let host = host.trim();
        let ip = ip.trim();
        if host.is_empty() || ip.is_empty() {
            return Err(RuntimeError::InitFailed(format!(
                "invalid --add-host '{trimmed}' (expected HOST:IP)"
            )));
        }
        hosts_content.push_str(&format!("{ip}\t{host}\n"));
    }
    fs::write(&hosts_file, hosts_content)
        .map_err(|e| RuntimeError::sys("write container coral-hosts", e))?;
    if let Ok(abs) = fs::canonicalize(&hosts_file) {
        mounts.push(Mount {
            destination: "/etc/hosts".to_string(),
            typ: Some("bind".to_string()),
            source: Some(abs.to_string_lossy().into_owned()),
            options: vec!["bind".to_string(), "ro".to_string()],
        });
    }

    // 2. Custom `--dns` servers or fallback to host's `/etc/resolv.conf`.
    let dns_servers: Vec<&str> = custom_dns
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if !dns_servers.is_empty() {
        let resolv_file = bundle_dir.join("coral-resolv.conf");
        let mut content = String::new();
        for ns in dns_servers {
            content.push_str(&format!("nameserver {ns}\n"));
        }
        fs::write(&resolv_file, content)
            .map_err(|e| RuntimeError::sys("write container coral-resolv.conf", e))?;
        if let Ok(abs) = fs::canonicalize(&resolv_file) {
            mounts.push(Mount {
                destination: "/etc/resolv.conf".to_string(),
                typ: Some("bind".to_string()),
                source: Some(abs.to_string_lossy().into_owned()),
                options: vec!["bind".to_string(), "ro".to_string()],
            });
        }
    } else if let Ok(abs) = fs::canonicalize("/etc/resolv.conf")
        && abs.is_file()
    {
        mounts.push(Mount {
            destination: "/etc/resolv.conf".to_string(),
            typ: Some("bind".to_string()),
            source: Some(abs.to_string_lossy().into_owned()),
            options: vec!["bind".to_string(), "ro".to_string()],
        });
    }

    Ok(mounts)
}

/// Appends a `ip\thost` line in-place to an existing container's `bundle_dir/coral-hosts` file
/// (preserving its inode so a live bind-mounted `/etc/hosts` sees the new peer immediately).
pub fn append_host_entry_in_place(bundle_dir: &Path, host: &str, ip: &str) -> std::io::Result<()> {
    let host = host.trim();
    let ip = ip.trim();
    if host.is_empty() || ip.is_empty() {
        return Ok(());
    }
    let hosts_file = bundle_dir.join("coral-hosts");
    if !hosts_file.is_file() {
        return Ok(());
    }
    let existing = fs::read_to_string(&hosts_file).unwrap_or_default();
    let line = format!("{ip}\t{host}");
    if existing.lines().any(|l| l.trim() == line) {
        return Ok(());
    }
    let mut f = OpenOptions::new().append(true).open(&hosts_file)?;
    writeln!(f, "{line}")?;
    f.flush()
}
