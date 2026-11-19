//! Persistent user-defined container networks (`coral network create / ls / inspect / rm`)
//! with automatic non-overlapping `/24` IPv4 subnet allocation.

use crate::NetError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkRecord {
    pub name: String,
    pub driver: String,
    pub subnet: String,
    pub gateway: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub created_at: i64,
}

/// Resolves the network registry JSON path (`CORAL_NETWORKS_FILE`, else `$XDG_STATE_HOME/coral/networks.json`
/// or `$HOME/.local/state/coral/networks.json`; `/var/lib/coral/networks.json` for root).
pub fn default_networks_file() -> PathBuf {
    if let Ok(explicit) = std::env::var("CORAL_NETWORKS_FILE")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return PathBuf::from("/var/lib/coral/networks.json");
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral").join("networks.json");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
    {
        return PathBuf::from(home)
            .join(".local")
            .join("state")
            .join("coral")
            .join("networks.json");
    }
    PathBuf::from("/tmp/coral-networks.json")
}

pub fn builtin_networks() -> Vec<NetworkRecord> {
    vec![
        NetworkRecord {
            name: "host".into(),
            driver: "host".into(),
            subnet: String::new(),
            gateway: String::new(),
            builtin: true,
            created_at: 0,
        },
        NetworkRecord {
            name: "bridge".into(),
            driver: "bridge".into(),
            subnet: "10.89.0.0/24".into(),
            gateway: "10.89.0.1".into(),
            builtin: true,
            created_at: 0,
        },
        NetworkRecord {
            name: "none".into(),
            driver: "null".into(),
            subnet: String::new(),
            gateway: String::new(),
            builtin: true,
            created_at: 0,
        },
    ]
}

pub fn is_valid_network_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.')
        && !name.starts_with('-')
}

pub fn list_networks() -> Vec<NetworkRecord> {
    list_networks_at(&default_networks_file())
}

pub fn list_networks_at(path: &Path) -> Vec<NetworkRecord> {
    let mut all = builtin_networks();
    let custom = load_custom_networks(path);
    all.extend(custom);
    all
}

pub fn find_network(name: &str) -> Option<NetworkRecord> {
    find_network_at(&default_networks_file(), name)
}

pub fn find_network_at(path: &Path, name: &str) -> Option<NetworkRecord> {
    let trimmed = name.trim();
    list_networks_at(path)
        .into_iter()
        .find(|n| n.name.eq_ignore_ascii_case(trimmed))
}

pub fn create_network(
    name: &str,
    driver: Option<&str>,
    subnet: Option<&str>,
) -> Result<NetworkRecord, NetError> {
    create_network_at(&default_networks_file(), name, driver, subnet)
}

pub fn create_network_at(
    path: &Path,
    name: &str,
    driver: Option<&str>,
    subnet: Option<&str>,
) -> Result<NetworkRecord, NetError> {
    let trimmed = name.trim();
    if !is_valid_network_name(trimmed) {
        return Err(NetError::Validation(format!(
            "invalid network name '{trimmed}' (use alphanumeric, '-', '_', '.')"
        )));
    }
    let drv = driver.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("bridge");
    if drv != "bridge" && drv != "isolated" {
        return Err(NetError::Validation(format!(
            "unsupported network driver '{drv}' (expected 'bridge')"
        )));
    }

    let existing = list_networks_at(path);
    if existing.iter().any(|n| n.name.eq_ignore_ascii_case(trimmed)) {
        return Err(NetError::Validation(format!(
            "network '{trimmed}' already exists"
        )));
    }

    let (resolved_subnet, gateway) = match subnet.map(str::trim).filter(|s| !s.is_empty()) {
        Some(cidr) => {
            let gw = parse_subnet_and_gateway(cidr)?;
            (cidr.to_string(), gw)
        }
        None => allocate_next_subnet(&existing)?,
    };

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let record = NetworkRecord {
        name: trimmed.to_string(),
        driver: "bridge".into(),
        subnet: resolved_subnet,
        gateway,
        builtin: false,
        created_at,
    };

    let mut custom = load_custom_networks(path);
    custom.push(record.clone());
    save_custom_networks(path, &custom)?;
    Ok(record)
}

pub fn remove_network(name: &str) -> Result<(), NetError> {
    remove_network_at(&default_networks_file(), name)
}

pub fn remove_network_at(path: &Path, name: &str) -> Result<(), NetError> {
    let trimmed = name.trim();
    if builtin_networks()
        .iter()
        .any(|b| b.name.eq_ignore_ascii_case(trimmed))
    {
        return Err(NetError::Validation(format!(
            "cannot remove built-in network '{trimmed}'"
        )));
    }
    let mut custom = load_custom_networks(path);
    let before = custom.len();
    custom.retain(|n| !n.name.eq_ignore_ascii_case(trimmed));
    if custom.len() == before {
        return Err(NetError::Validation(format!(
            "network '{trimmed}' not found"
        )));
    }
    save_custom_networks(path, &custom)?;
    Ok(())
}

fn parse_subnet_and_gateway(cidr: &str) -> Result<String, NetError> {
    let Some((ip_str, prefix_str)) = cidr.split_once('/') else {
        return Err(NetError::Validation(format!(
            "invalid CIDR subnet '{cidr}' (expected e.g. 10.89.1.0/24)"
        )));
    };
    let ip: Ipv4Addr = ip_str
        .parse()
        .map_err(|_| NetError::Validation(format!("invalid IPv4 address in subnet '{cidr}'")))?;
    let prefix: u8 = prefix_str
        .parse()
        .map_err(|_| NetError::Validation(format!("invalid CIDR prefix in '{cidr}'")))?;
    if !(8..=30).contains(&prefix) {
        return Err(NetError::Validation(format!(
            "CIDR prefix /{prefix} out of supported range (/8..=/30)"
        )));
    }
    let octets = ip.octets();
    let gw = Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3].saturating_add(1).max(1));
    Ok(gw.to_string())
}

fn allocate_next_subnet(existing: &[NetworkRecord]) -> Result<(String, String), NetError> {
    let used: HashSet<&str> = existing.iter().map(|n| n.subnet.as_str()).collect();
    for third in 1u8..=254 {
        let candidate = format!("10.89.{third}.0/24");
        if !used.contains(candidate.as_str()) {
            return Ok((candidate, format!("10.89.{third}.1")));
        }
    }
    Err(NetError::Validation(
        "no free 10.89.x.0/24 subnets remaining; pass --subnet explicitly".into(),
    ))
}

fn load_custom_networks(path: &Path) -> Vec<NetworkRecord> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_custom_networks(path: &Path, list: &[NetworkRecord]) -> Result<(), NetError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(list)?;
    fs::write(&tmp, format!("{json}\n"))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn creates_allocates_subnets_inspects_and_removes_networks() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("networks.json");

        let n1 = create_network_at(&file, "backend", None, None).unwrap();
        assert_eq!(n1.subnet, "10.89.1.0/24");
        assert_eq!(n1.gateway, "10.89.1.1");

        let n2 = create_network_at(&file, "frontend", Some("bridge"), Some("172.28.0.0/24")).unwrap();
        assert_eq!(n2.subnet, "172.28.0.0/24");
        assert_eq!(n2.gateway, "172.28.0.1");

        assert!(create_network_at(&file, "backend", None, None).is_err());
        assert!(remove_network_at(&file, "bridge").is_err());

        let all = list_networks_at(&file);
        assert!(all.iter().any(|n| n.name == "backend"));
        assert!(all.iter().any(|n| n.name == "frontend"));

        remove_network_at(&file, "backend").unwrap();
        assert!(find_network_at(&file, "backend").is_none());
    }
}
