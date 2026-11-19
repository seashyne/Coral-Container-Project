//! CNI v1.0.0 network configuration (`*.conf`), IPAM, and execution result models.

use crate::NetError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

pub const CNI_SPEC_VERSION: &str = "1.0.0";

/// Standard CNI v1.0.0 network configuration (`*.conf`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CniNetworkConfig {
    pub cni_version: String,
    pub name: String,
    #[serde(rename = "type")]
    pub plugin_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge: Option<String>,
    #[serde(default)]
    pub is_gateway: bool,
    #[serde(default)]
    pub ip_masq: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipam: Option<IpamConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IpamConfig {
    #[serde(rename = "type")]
    pub plugin_type: String,
    pub subnet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<CniRoute>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CniRoute {
    pub dst: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gw: Option<String>,
}

/// Standard CNI v1.0.0 execution result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CniResult {
    pub cni_version: String,
    #[serde(default)]
    pub interfaces: Vec<CniInterface>,
    #[serde(default)]
    pub ips: Vec<CniIpConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CniInterface {
    pub name: String,
    pub mac: Option<String>,
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CniIpConfig {
    pub address: String,
    pub gateway: Option<String>,
    pub interface: Option<usize>,
}

impl CniNetworkConfig {
    /// Generates Coral's default bridge CNI configuration (`coral0`, `10.88.0.0/16`).
    pub fn default_bridge() -> Self {
        Self {
            cni_version: CNI_SPEC_VERSION.into(),
            name: "coral-bridge".into(),
            plugin_type: "bridge".into(),
            bridge: Some("coral0".into()),
            is_gateway: true,
            ip_masq: true,
            ipam: Some(IpamConfig {
                plugin_type: "host-local".into(),
                subnet: "10.88.0.0/16".into(),
                gateway: Some("10.88.0.1".into()),
                routes: vec![CniRoute {
                    dst: "0.0.0.0/0".into(),
                    gw: None,
                }],
            }),
        }
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, NetError> {
        let content = fs::read_to_string(path)?;
        let cfg: Self = serde_json::from_str(&content)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn to_file(&self, path: impl AsRef<Path>) -> Result<(), NetError> {
        self.validate()?;
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, format!("{json}\n"))?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), NetError> {
        if self.cni_version.trim().is_empty() {
            return Err(NetError::Validation("cniVersion must not be empty".into()));
        }
        if self.name.trim().is_empty() {
            return Err(NetError::Validation("network name must not be empty".into()));
        }
        if self.plugin_type.trim().is_empty() {
            return Err(NetError::Validation("CNI plugin type must not be empty".into()));
        }
        if let Some(ipam) = &self.ipam
            && !ipam.subnet.contains('/')
        {
            return Err(NetError::Validation(format!(
                "IPAM subnet '{}' must be CIDR notation (e.g. 10.88.0.0/16)",
                ipam.subnet
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn default_bridge_cni_config_roundtrips() {
        let cfg = CniNetworkConfig::default_bridge();
        cfg.validate().unwrap();
        let tmp = NamedTempFile::new().unwrap();
        cfg.to_file(tmp.path()).unwrap();
        let loaded = CniNetworkConfig::from_file(tmp.path()).unwrap();
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn rejects_non_cidr_subnet() {
        let mut cfg = CniNetworkConfig::default_bridge();
        cfg.ipam.as_mut().unwrap().subnet = "10.88.0.0".into();
        assert!(matches!(cfg.validate(), Err(NetError::Validation(_))));
    }
}
