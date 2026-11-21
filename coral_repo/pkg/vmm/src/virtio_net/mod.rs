//! Pure-Rust VirtIO-MMIO v2 Network Device (`virtio-net`) + Userspace Virtual L2/L3 Switch.
//!
//! Exposes `eth0` (`virtio_mmio.device=4K@0xd0002000:6`) to Linux guests with zero root/TAP
//! requirements: includes built-in ARP, DHCP (`10.0.2.15/24`), ICMP echo (`10.0.2.2`),
//! non-blocking UDP/DNS relay, outbound TCP proxy, and host->guest TCP port forwarding.

mod checksum;
mod device;
mod dhcp;
mod encode;
mod flows;
mod frames;
mod tcp;
#[cfg(test)]
mod tests;

use crate::virtio_mmio::VirtioMmioTransport;
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::time::Instant;

pub const VIRTIO_NET_MMIO_BASE: u64 = 0xD000_2000;
pub const VIRTIO_NET_MMIO_SIZE: u64 = 0x1000;
pub const VIRTIO_NET_IRQ: u32 = 6;

const VIRTIO_NET_F_MAC: u32 = 1 << 5;
const VIRTIO_NET_F_STATUS: u32 = 1 << 16;
const VIRTIO_NET_S_LINK_UP: u16 = 1;
const VIRTIO_NET_HDR_LEN: usize = 12;

pub const GUEST_IP: [u8; 4] = [10, 0, 2, 15];
pub const GATEWAY_IP: [u8; 4] = [10, 0, 2, 2];
pub const SUBNET_MASK: [u8; 4] = [255, 255, 255, 0];
pub const GUEST_MAC: [u8; 6] = [0x02, 0x43, 0x4F, 0x52, 0x41, 0x01];
pub const GATEWAY_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x02];

const ETH_ARP: u16 = 0x0806;
const ETH_IPV4: u16 = 0x0800;

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;

/// Host-to-guest TCP port mapping (`127.0.0.1:host_port` -> `10.0.2.15:guest_port`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortForwardRule {
    pub host_port: u16,
    pub guest_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FlowKey {
    remote_ip: [u8; 4],
    remote_port: u16,
    guest_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpState {
    SynSent,
    Established,
    FinWait,
}

struct TcpFlow {
    stream: TcpStream,
    state: TcpState,
    host_seq: u32,
    guest_seq: u32,
    last_active: Instant,
}

struct UdpFlow {
    socket: UdpSocket,
    last_active: Instant,
}

/// Pure-Rust VirtIO-MMIO Net device with virtual L2/L3/L4 host bridge.
pub struct VirtioNetDevice {
    pub transport: VirtioMmioTransport,
    guest_mac: [u8; 6],
    rx_pending: VecDeque<Vec<u8>>,
    tcp_flows: HashMap<FlowKey, TcpFlow>,
    udp_flows: HashMap<FlowKey, UdpFlow>,
    listeners: Vec<(u16, TcpListener)>,
    next_ephemeral_port: u16,
    ip_ident: u16,
    dns_upstream: Option<Ipv4Addr>,
    pub tx_packets: u64,
    pub rx_packets: u64,
}
