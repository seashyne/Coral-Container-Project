//! CNI v1.0.0 networking configuration and Linux network namespace utilities.

pub mod cni;
pub mod cni_runner;
pub mod dns_server;
pub mod egress;
pub mod netns;
pub mod networks;
pub mod vswitch;
pub mod vswitch_tap;

use std::io;
use thiserror::Error;

pub use cni::{
    CNI_SPEC_VERSION, CniInterface, CniIpConfig, CniNetworkConfig, CniResult, CniRoute, IpamConfig,
};
pub use cni_runner::{
    CniAddOutcome, CniExecRequest, CniNetworkList, CniPluginChain, extract_primary_ip,
    has_cni_configs,
};
pub use dns_server::{EmbeddedDnsServer, build_dns_query_packet, handle_dns_packet};
pub use vswitch::RootlessVSwitch;
pub use vswitch_tap::open_tap_in_netns;
pub use egress::{EgressBackend, RootlessEgressGuard, detect_egress_backend};
pub use netns::{
    PortMapping, PortProtocol, bring_loopback_up, connect_container_port, connect_container_udp,
};
pub use networks::{
    NetworkRecord, builtin_networks, create_network, create_network_at, default_networks_file,
    find_network, find_network_at, is_valid_network_name, list_networks, list_networks_at,
    remove_network, remove_network_at,
};

#[derive(Debug, Error)]
pub enum NetError {
    #[error("network I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("invalid CNI JSON configuration: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid CNI configuration: {0}")]
    Validation(String),
}
