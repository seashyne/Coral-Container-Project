//! Official CNI v1.0.0 Plugin Chain Runner (`ADD`, `CHECK`, `DEL`) compatible with
//! Kubernetes CNI plugins such as Cilium, Calico, Flannel, bridge, and portmap.
//!
//! Scans `conf_dir` (`/etc/cni/net.d`) lexicographically for `*.conflist` or `*.conf`,
//! executes plugin binaries from `bin_dir` (`/opt/cni/bin`) chaining `prevResult` JSON
//! across plugins on `ADD` and tearing them down in reverse order on `DEL`.

mod chain;
mod helpers;
#[cfg(test)]
mod tests;

use crate::cni::CniResult;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub use helpers::{extract_primary_ip, has_cni_configs};

/// Parsed CNI `.conflist` (or single `.conf` wrapped as a 1-plugin chain).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CniNetworkList {
    pub cni_version: String,
    pub name: String,
    #[serde(default)]
    pub disable_check: bool,
    pub plugins: Vec<Value>,
}

/// Parameters passed to a CNI plugin chain invocation for a Kubernetes Pod Sandbox.
#[derive(Debug, Clone)]
pub struct CniExecRequest {
    pub container_id: String,
    pub netns_path: String,
    pub ifname: String,
    pub pod_name: String,
    pub pod_namespace: String,
    pub pod_uid: String,
    pub runtime_config: Option<Value>,
}

/// Outcome returned by `CniPluginChain::add_pod_network`, including raw JSON `prevResult`
/// needed for subsequent `CHECK` and `DEL` operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CniAddOutcome {
    pub network_name: String,
    pub result: CniResult,
    pub raw_result: Value,
    pub pod_ip: Option<String>,
}

/// CNI v1.0.0 plugin chain executor.
#[derive(Debug, Clone)]
pub struct CniPluginChain {
    pub conf_dir: PathBuf,
    pub bin_dir: PathBuf,
    /// When true (custom CNI dir or explicit env config), missing plugins return an error.
    /// When false (passive `/etc/cni/net.d` auto-discovery), incomplete host CNI setups
    /// are skipped cleanly so rootless pods/compose keep working.
    pub strict: bool,
}

impl Default for CniPluginChain {
    fn default() -> Self {
        let env_conf = std::env::var_os("CORAL_CNI_CONF_DIR").map(PathBuf::from);
        let env_bin = std::env::var_os("CORAL_CNI_BIN_DIR").map(PathBuf::from);
        let strict = env_conf.is_some() || env_bin.is_some();
        Self {
            conf_dir: env_conf.unwrap_or_else(|| PathBuf::from("/etc/cni/net.d")),
            bin_dir: env_bin.unwrap_or_else(|| PathBuf::from("/opt/cni/bin")),
            strict,
        }
    }
}

impl CniPluginChain {
    pub fn new(conf_dir: impl Into<PathBuf>, bin_dir: impl Into<PathBuf>) -> Self {
        let conf_dir = conf_dir.into();
        let bin_dir = bin_dir.into();
        let strict = conf_dir != Path::new("/etc/cni/net.d") || bin_dir != Path::new("/opt/cni/bin");
        Self {
            conf_dir,
            bin_dir,
            strict,
        }
    }

    fn all_plugin_bins_exist(&self, net_list: &CniNetworkList) -> bool {
        net_list.plugins.iter().all(|plugin| {
            plugin
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|p| self.bin_dir.join(p).is_file())
        })
    }

    /// Scans `conf_dir` in lexicographical order and loads the first valid `.conflist` or `.conf`.
    pub fn discover_network_list(&self) -> anyhow::Result<Option<CniNetworkList>> {
        if !self.conf_dir.is_dir() {
            return Ok(None);
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.conf_dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|ext| matches!(ext, "conflist" | "conf" | "json"))
            })
            .collect();
        files.sort();

        for path in files {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read CNI config '{}'", path.display()))?;
            let val: Value = serde_json::from_str(&raw)
                .with_context(|| format!("parse CNI JSON '{}'", path.display()))?;
            if val.get("plugins").and_then(|p| p.as_array()).is_some() {
                let list: CniNetworkList = serde_json::from_value(val)
                    .with_context(|| format!("decode CNI conflist '{}'", path.display()))?;
                if !list.plugins.is_empty() {
                    return Ok(Some(list));
                }
            } else if val.get("type").and_then(|t| t.as_str()).is_some() {
                let cni_version = val
                    .get("cniVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("1.0.0")
                    .to_string();
                let name = val
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("coral-cni")
                    .to_string();
                return Ok(Some(CniNetworkList {
                    cni_version,
                    name,
                    disable_check: false,
                    plugins: vec![val],
                }));
            }
        }
        Ok(None)
    }
}
