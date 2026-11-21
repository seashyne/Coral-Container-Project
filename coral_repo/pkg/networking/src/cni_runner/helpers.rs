//! Small helpers for reading CNI results and probing the local CNI configuration directory.

use super::CniPluginChain;
use crate::cni::CniResult;
use std::path::{Path, PathBuf};

/// Extracts the primary IPv4/IPv6 host address (without `/prefix` CIDR) from a CNI v1.0.0 result.
pub fn extract_primary_ip(res: &CniResult) -> Option<String> {
    res.ips
        .first()
        .map(|ip| ip.address.split('/').next().unwrap_or(&ip.address).to_string())
        .filter(|s| !s.is_empty())
}

/// Checks if any CNI plugin configuration file exists inside `conf_dir`.
pub fn has_cni_configs(conf_dir: &Path) -> bool {
    CniPluginChain::new(conf_dir, PathBuf::new())
        .discover_network_list()
        .ok()
        .flatten()
        .is_some()
}
