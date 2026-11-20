//! Pure-Rust VirtIO-MMIO v2 Network Device (`virtio-net`) + Userspace Virtual L2/L3 Switch.
//!
//! Exposes `eth0` (`virtio_mmio.device=4K@0xd0002000:6`) to Linux guests with zero root/TAP
//! requirements: includes built-in ARP, DHCP (`10.0.2.15/24`), ICMP echo (`10.0.2.2`),
//! non-blocking UDP/DNS relay, outbound TCP proxy, and host->guest TCP port forwarding.

use crate::virtio_mmio::{VIRTIO_DEV_NET, VIRTIO_MMIO_INT_VRING, VirtioMmioTransport};
use anyhow::{Context, Result};
use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

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

impl VirtioNetDevice {
    pub fn new(port_forwards: &[PortForwardRule]) -> Result<Self> {
        let mut listeners = Vec::new();
        for rule in port_forwards {
            let l = TcpListener::bind(("127.0.0.1", rule.host_port)).with_context(|| {
                format!(
                    "bind MicroVM VirtIO-Net port forward 127.0.0.1:{} -> guest:{}",
                    rule.host_port, rule.guest_port
                )
            })?;
            l.set_nonblocking(true)?;
            listeners.push((rule.guest_port, l));
        }

        let mut cfg = Vec::with_capacity(8);
        cfg.extend_from_slice(&GUEST_MAC);
        cfg.extend_from_slice(&VIRTIO_NET_S_LINK_UP.to_le_bytes());

        Ok(Self {
            transport: VirtioMmioTransport::new(
                VIRTIO_DEV_NET,
                VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS,
                0,
                2,
                cfg,
            ),
            guest_mac: GUEST_MAC,
            rx_pending: VecDeque::new(),
            tcp_flows: HashMap::new(),
            udp_flows: HashMap::new(),
            listeners,
            next_ephemeral_port: 49152,
            ip_ident: 1,
            dns_upstream: detect_host_ipv4_dns(),
            tx_packets: 0,
            rx_packets: 0,
        })
    }

    pub fn read_mmio(&self, offset: u64, data: &mut [u8]) {
        self.transport.read_mmio(offset, data);
    }

    /// Processes an MMIO write and handles TX/RX virtqueue notifications.
    pub fn write_mmio(&mut self, offset: u64, data: &[u8], mem: &mut [u8]) -> bool {
        let Some(qidx) = self.transport.write_mmio(offset, data) else {
            return false;
        };
        match qidx {
            1 => {
                self.drain_tx(mem);
                self.flush_rx(mem)
            }
            0 => self.flush_rx(mem),
            _ => false,
        }
    }

    /// Polls active TCP/UDP sockets + port-forward listeners and delivers RX frames.
    pub fn poll(&mut self, mem: &mut [u8]) -> bool {
        self.poll_listeners();
        self.poll_tcp_flows();
        self.poll_udp_flows();
        self.flush_rx(mem)
    }

    fn drain_tx(&mut self, mem: &mut [u8]) {
        while let Some(chain) = self.transport.queues[1].pop_avail_chain(mem) {
            let mut packet = Vec::with_capacity(1536);
            for seg in &chain.segments {
                if !seg.write_only {
                    let s = seg.addr as usize;
                    packet.extend_from_slice(&mem[s..s + seg.len as usize]);
                }
            }
            let _ = self.transport.queues[1].push_used(mem, chain.head, 0);
            self.transport.interrupt_status |= VIRTIO_MMIO_INT_VRING;

            if packet.len() > VIRTIO_NET_HDR_LEN {
                self.tx_packets = self.tx_packets.saturating_add(1);
                let frame = &packet[VIRTIO_NET_HDR_LEN..];
                self.handle_guest_frame(frame);
            }
        }
    }

    fn flush_rx(&mut self, mem: &mut [u8]) -> bool {
        let mut delivered = false;
        while let Some(frame) = self.rx_pending.front() {
            let need_len = VIRTIO_NET_HDR_LEN + frame.len();
            let Some(chain) = self.transport.queues[0].pop_avail_chain(mem) else {
                break;
            };
            let frame = self.rx_pending.pop_front().unwrap();
            let mut full = Vec::with_capacity(need_len);
            full.extend_from_slice(&[0u8; VIRTIO_NET_HDR_LEN]);
            full.extend_from_slice(&frame);

            let mut offset = 0usize;
            for seg in &chain.segments {
                if !seg.write_only || offset >= full.len() {
                    continue;
                }
                let take = (seg.len as usize).min(full.len() - offset);
                let dst = &mut mem[seg.addr as usize..seg.addr as usize + take];
                dst.copy_from_slice(&full[offset..offset + take]);
                offset += take;
            }
            let _ = self
                .transport
                .queues[0]
                .push_used(mem, chain.head, offset as u32);
            self.transport.interrupt_status |= VIRTIO_MMIO_INT_VRING;
            self.rx_packets = self.rx_packets.saturating_add(1);
            delivered = true;
        }
        delivered
    }

    /// Directly injects a guest TX Ethernet frame (used by unit tests as well as VirtIO TX).
    pub fn handle_guest_frame(&mut self, frame: &[u8]) {
        if frame.len() < 14 {
            return;
        }
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        let payload = &frame[14..];
        match ethertype {
            ETH_ARP => self.handle_arp(payload),
            ETH_IPV4 => self.handle_ipv4(payload),
            _ => {}
        }
    }

    fn handle_arp(&mut self, arp: &[u8]) {
        if arp.len() < 28 {
            return;
        }
        let oper = u16::from_be_bytes([arp[6], arp[7]]);
        if oper != 1 {
            return;
        }
        let sha: [u8; 6] = arp[8..14].try_into().unwrap();
        let spa: [u8; 4] = arp[14..18].try_into().unwrap();
        let tpa: [u8; 4] = arp[24..28].try_into().unwrap();
        if tpa != GATEWAY_IP {
            return;
        }
        let mut reply = Vec::with_capacity(42);
        reply.extend_from_slice(&sha);
        reply.extend_from_slice(&GATEWAY_MAC);
        reply.extend_from_slice(&ETH_ARP.to_be_bytes());
        reply.extend_from_slice(&1u16.to_be_bytes()); // Ethernet
        reply.extend_from_slice(&ETH_IPV4.to_be_bytes()); // IPv4
        reply.push(6);
        reply.push(4);
        reply.extend_from_slice(&2u16.to_be_bytes()); // ARP Reply
        reply.extend_from_slice(&GATEWAY_MAC);
        reply.extend_from_slice(&GATEWAY_IP);
        reply.extend_from_slice(&sha);
        reply.extend_from_slice(&spa);
        self.rx_pending.push_back(reply);
    }

    fn handle_ipv4(&mut self, pkt: &[u8]) {
        if pkt.len() < 20 || (pkt[0] >> 4) != 4 {
            return;
        }
        let ihl = ((pkt[0] & 0x0F) as usize) * 4;
        let total_len = u16::from_be_bytes([pkt[2], pkt[3]]) as usize;
        if ihl < 20 || total_len < ihl || pkt.len() < total_len {
            return;
        }
        let proto = pkt[9];
        let src_ip: [u8; 4] = pkt[12..16].try_into().unwrap();
        let dst_ip: [u8; 4] = pkt[16..20].try_into().unwrap();
        let body = &pkt[ihl..total_len];

        match proto {
            1 => self.handle_icmp(src_ip, dst_ip, body),
            6 => self.handle_tcp(src_ip, dst_ip, body),
            17 => self.handle_udp(src_ip, dst_ip, body),
            _ => {}
        }
    }

    fn handle_icmp(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4], icmp: &[u8]) {
        if icmp.len() < 8 || icmp[0] != 8 || dst_ip != GATEWAY_IP {
            return;
        }
        let mut reply = icmp.to_vec();
        reply[0] = 0; // Echo Reply
        reply[2] = 0;
        reply[3] = 0;
        let csum = internet_checksum(&reply);
        reply[2..4].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(GATEWAY_IP, src_ip, 1, &reply);
    }

    fn handle_udp(&mut self, _src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) {
        if udp.len() < 8 {
            return;
        }
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let udp_len = u16::from_be_bytes([udp[4], udp[5]]) as usize;
        if udp_len < 8 || udp.len() < udp_len {
            return;
        }
        let payload = &udp[8..udp_len];

        if src_port == 68 && dst_port == 67 {
            self.handle_dhcp(payload);
            return;
        }

        let target_ip = self.translate_target_ip(dst_ip, dst_port);
        let key = FlowKey {
            remote_ip: dst_ip,
            remote_port: dst_port,
            guest_port: src_port,
        };
        let flow = match self.udp_flows.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
                    return;
                };
                let _ = sock.set_nonblocking(true);
                v.insert(UdpFlow {
                    socket: sock,
                    last_active: Instant::now(),
                })
            }
        };
        flow.last_active = Instant::now();
        let _ = flow
            .socket
            .send_to(payload, SocketAddr::V4(SocketAddrV4::new(target_ip, dst_port)));
    }

    fn handle_dhcp(&mut self, bootp: &[u8]) {
        if bootp.len() < 240 || bootp[0] != 1 {
            return;
        }
        // Check DHCP magic cookie 99.130.83.99
        if bootp[236..240] != [99, 130, 83, 99] {
            return;
        }
        let xid = &bootp[4..8];
        let flags = &bootp[10..12];
        let chaddr = &bootp[28..44];
        let mut msg_type = None;
        let mut i = 240;
        while i < bootp.len() {
            let opt = bootp[i];
            if opt == 255 {
                break;
            }
            if opt == 0 {
                i += 1;
                continue;
            }
            if i + 1 >= bootp.len() {
                break;
            }
            let len = bootp[i + 1] as usize;
            if opt == 53 && len >= 1 && i + 2 < bootp.len() {
                msg_type = Some(bootp[i + 2]);
            }
            i += 2 + len;
        }

        let resp_type = match msg_type {
            Some(1) => 2u8, // DISCOVER -> DHCPOFFER
            Some(3) => 5u8, // REQUEST -> DHCPACK
            _ => return,
        };

        let mut dhcp = vec![0u8; 300];
        dhcp[0] = 2; // BOOTREPLY
        dhcp[1] = 1;
        dhcp[2] = 6;
        dhcp[4..8].copy_from_slice(xid);
        dhcp[10..12].copy_from_slice(flags);
        dhcp[16..20].copy_from_slice(&GUEST_IP); // yiaddr
        dhcp[20..24].copy_from_slice(&GATEWAY_IP); // siaddr
        dhcp[28..44].copy_from_slice(chaddr);
        dhcp[236..240].copy_from_slice(&[99, 130, 83, 99]);

        let mut p = 240;
        // Opt 53: DHCP Message Type
        dhcp[p..p + 3].copy_from_slice(&[53, 1, resp_type]);
        p += 3;
        // Opt 54: Server Identifier
        dhcp[p..p + 2].copy_from_slice(&[54, 4]);
        dhcp[p + 2..p + 6].copy_from_slice(&GATEWAY_IP);
        p += 6;
        // Opt 51: Lease Time (86400s)
        dhcp[p..p + 2].copy_from_slice(&[51, 4]);
        dhcp[p + 2..p + 6].copy_from_slice(&86400u32.to_be_bytes());
        p += 6;
        // Opt 1: Subnet Mask
        dhcp[p..p + 2].copy_from_slice(&[1, 4]);
        dhcp[p + 2..p + 6].copy_from_slice(&SUBNET_MASK);
        p += 6;
        // Opt 3: Router
        dhcp[p..p + 2].copy_from_slice(&[3, 4]);
        dhcp[p + 2..p + 6].copy_from_slice(&GATEWAY_IP);
        p += 6;
        // Opt 6: DNS Server
        dhcp[p..p + 2].copy_from_slice(&[6, 4]);
        dhcp[p + 2..p + 6].copy_from_slice(&GATEWAY_IP);
        p += 6;
        dhcp[p] = 255;

        self.enqueue_udp_packet(GATEWAY_IP, 67, GUEST_IP, 68, &dhcp);
    }

    fn handle_tcp(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4], tcp: &[u8]) {
        if tcp.len() < 20 || src_ip != GUEST_IP {
            return;
        }
        let guest_port = u16::from_be_bytes([tcp[0], tcp[1]]);
        let remote_port = u16::from_be_bytes([tcp[2], tcp[3]]);
        let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
        let ack = u32::from_be_bytes([tcp[8], tcp[9], tcp[10], tcp[11]]);
        let data_off = ((tcp[12] >> 4) as usize) * 4;
        if data_off < 20 || tcp.len() < data_off {
            return;
        }
        let flags = tcp[13];
        let payload = &tcp[data_off..];
        let key = FlowKey {
            remote_ip: dst_ip,
            remote_port,
            guest_port,
        };

        if (flags & TCP_RST) != 0 {
            self.tcp_flows.remove(&key);
            return;
        }

        if (flags & TCP_SYN) != 0 && (flags & TCP_ACK) == 0 {
            let target_ip = self.translate_target_ip(dst_ip, remote_port);
            let addr = SocketAddr::V4(SocketAddrV4::new(target_ip, remote_port));
            let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(60)) else {
                self.enqueue_tcp_segment(dst_ip, remote_port, src_ip, guest_port, 0, seq.wrapping_add(1), TCP_RST | TCP_ACK, &[]);
                return;
            };
            let _ = stream.set_nonblocking(true);
            let _ = stream.set_nodelay(true);
            let host_seq = 0x1000_0001u32.wrapping_add(u32::from(guest_port));
            let guest_next = seq.wrapping_add(1);
            self.tcp_flows.insert(
                key,
                TcpFlow {
                    stream,
                    state: TcpState::Established,
                    host_seq: host_seq.wrapping_add(1),
                    guest_seq: guest_next,
                    last_active: Instant::now(),
                },
            );
            // Send SYN|ACK with MSS=1460 option.
            self.enqueue_tcp_synack(dst_ip, remote_port, src_ip, guest_port, host_seq, guest_next);
            return;
        }

        let Some(flow) = self.tcp_flows.get_mut(&key) else {
            return;
        };
        flow.last_active = Instant::now();

        if flow.state == TcpState::SynSent && (flags & (TCP_SYN | TCP_ACK)) == (TCP_SYN | TCP_ACK) {
            flow.guest_seq = seq.wrapping_add(1);
            flow.state = TcpState::Established;
            let host_seq = flow.host_seq;
            let guest_seq = flow.guest_seq;
            self.enqueue_tcp_segment(
                key.remote_ip,
                key.remote_port,
                GUEST_IP,
                key.guest_port,
                host_seq,
                guest_seq,
                TCP_ACK,
                &[],
            );
            return;
        }

        let mut close_now = false;
        if !payload.is_empty() {
            if seq == flow.guest_seq {
                let mut written = 0usize;
                while written < payload.len() {
                    match flow.stream.write(&payload[written..]) {
                        Ok(0) => {
                            close_now = true;
                            break;
                        }
                        Ok(n) => written += n,
                        Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {
                            close_now = true;
                            break;
                        }
                    }
                }
                flow.guest_seq = flow.guest_seq.wrapping_add(written as u32);
            }
            let host_seq = flow.host_seq;
            let guest_seq = flow.guest_seq;
            self.enqueue_tcp_segment(
                dst_ip,
                remote_port,
                GUEST_IP,
                guest_port,
                host_seq,
                guest_seq,
                TCP_ACK,
                &[],
            );
        }

        if (flags & TCP_FIN) != 0 {
            let Some(flow) = self.tcp_flows.get_mut(&key) else {
                return;
            };
            flow.guest_seq = flow.guest_seq.wrapping_add(1);
            let host_seq = flow.host_seq;
            let guest_seq = flow.guest_seq;
            let state = flow.state;
            if state == TcpState::FinWait {
                let _ = flow.stream.shutdown(std::net::Shutdown::Both);
                self.tcp_flows.remove(&key);
                self.enqueue_tcp_segment(
                    dst_ip,
                    remote_port,
                    GUEST_IP,
                    guest_port,
                    host_seq,
                    guest_seq,
                    TCP_ACK,
                    &[],
                );
            } else {
                // Guest half-closed its write side (`printf | nc`): only shut down write
                // direction to host so response bytes still stream back into the VM!
                let _ = flow.stream.shutdown(std::net::Shutdown::Write);
                self.enqueue_tcp_segment(
                    dst_ip,
                    remote_port,
                    GUEST_IP,
                    guest_port,
                    host_seq,
                    guest_seq,
                    TCP_ACK,
                    &[],
                );
            }
            return;
        }

        let _ = ack;
        if close_now {
            self.tcp_flows.remove(&key);
        }
    }

    fn poll_listeners(&mut self) {
        for idx in 0..self.listeners.len() {
            let guest_port = self.listeners[idx].0;
            loop {
                match self.listeners[idx].1.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(true);
                        let _ = stream.set_nodelay(true);
                        let remote_port = self.next_ephemeral_port;
                        self.next_ephemeral_port = if self.next_ephemeral_port >= 60999 {
                            49152
                        } else {
                            self.next_ephemeral_port + 1
                        };
                        let key = FlowKey {
                            remote_ip: GATEWAY_IP,
                            remote_port,
                            guest_port,
                        };
                        let host_seq = 0x2000_0001u32.wrapping_add(u32::from(remote_port));
                        self.tcp_flows.insert(
                            key,
                            TcpFlow {
                                stream,
                                state: TcpState::SynSent,
                                host_seq: host_seq.wrapping_add(1),
                                guest_seq: 0,
                                last_active: Instant::now(),
                            },
                        );
                        self.enqueue_tcp_segment(
                            GATEWAY_IP,
                            remote_port,
                            GUEST_IP,
                            guest_port,
                            host_seq,
                            0,
                            TCP_SYN,
                            &[],
                        );
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
    }

    fn poll_tcp_flows(&mut self) {
        let keys: Vec<FlowKey> = self.tcp_flows.keys().copied().collect();
        let mut to_send: Vec<(FlowKey, u32, u32, u8, Vec<u8>)> = Vec::new();
        let mut remove_keys = Vec::new();
        let now = Instant::now();

        for key in keys {
            let Some(flow) = self.tcp_flows.get_mut(&key) else {
                continue;
            };
            if now.duration_since(flow.last_active) > Duration::from_secs(300) {
                remove_keys.push(key);
                continue;
            }
            if flow.state != TcpState::Established {
                continue;
            }
            let mut buf = [0u8; 1400];
            loop {
                match flow.stream.read(&mut buf) {
                    Ok(0) => {
                        flow.state = TcpState::FinWait;
                        let seq = flow.host_seq;
                        flow.host_seq = flow.host_seq.wrapping_add(1);
                        to_send.push((key, seq, flow.guest_seq, TCP_FIN | TCP_ACK, Vec::new()));
                        break;
                    }
                    Ok(n) => {
                        flow.last_active = now;
                        let seq = flow.host_seq;
                        flow.host_seq = flow.host_seq.wrapping_add(n as u32);
                        to_send.push((key, seq, flow.guest_seq, TCP_PSH | TCP_ACK, buf[..n].to_vec()));
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        to_send.push((key, flow.host_seq, flow.guest_seq, TCP_RST | TCP_ACK, Vec::new()));
                        remove_keys.push(key);
                        break;
                    }
                }
            }
        }

        for k in remove_keys {
            self.tcp_flows.remove(&k);
        }
        for (key, seq, ack, flags, data) in to_send {
            self.enqueue_tcp_segment(
                key.remote_ip,
                key.remote_port,
                GUEST_IP,
                key.guest_port,
                seq,
                ack,
                flags,
                &data,
            );
        }
    }

    fn poll_udp_flows(&mut self) {
        let keys: Vec<FlowKey> = self.udp_flows.keys().copied().collect();
        let mut out = Vec::new();
        let now = Instant::now();
        let mut stale = Vec::new();
        let mut buf = [0u8; 1500];

        for key in keys {
            let Some(flow) = self.udp_flows.get_mut(&key) else {
                continue;
            };
            if now.duration_since(flow.last_active) > Duration::from_secs(60) {
                stale.push(key);
                continue;
            }
            loop {
                match flow.socket.recv_from(&mut buf) {
                    Ok((n, _)) => {
                        flow.last_active = now;
                        out.push((key, buf[..n].to_vec()));
                    }
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        for k in stale {
            self.udp_flows.remove(&k);
        }
        for (key, payload) in out {
            self.enqueue_udp_packet(
                key.remote_ip,
                key.remote_port,
                GUEST_IP,
                key.guest_port,
                &payload,
            );
        }
    }

    fn translate_target_ip(&self, dst_ip: [u8; 4], dst_port: u16) -> Ipv4Addr {
        if dst_ip == GATEWAY_IP {
            if dst_port == 53
                && let Some(dns) = self.dns_upstream
            {
                return dns;
            }
            return Ipv4Addr::LOCALHOST;
        }
        Ipv4Addr::from(dst_ip)
    }

    fn enqueue_udp_packet(
        &mut self,
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        payload: &[u8],
    ) {
        let udp_len = (8 + payload.len()) as u16;
        let mut udp = Vec::with_capacity(udp_len as usize);
        udp.extend_from_slice(&src_port.to_be_bytes());
        udp.extend_from_slice(&dst_port.to_be_bytes());
        udp.extend_from_slice(&udp_len.to_be_bytes());
        udp.extend_from_slice(&[0u8, 0u8]);
        udp.extend_from_slice(payload);
        let csum = transport_checksum(src_ip, dst_ip, 17, &udp);
        udp[6..8].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(src_ip, dst_ip, 17, &udp);
    }

    fn enqueue_tcp_synack(
        &mut self,
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        seq: u32,
        ack: u32,
    ) {
        let mut tcp = Vec::with_capacity(24);
        tcp.extend_from_slice(&src_port.to_be_bytes());
        tcp.extend_from_slice(&dst_port.to_be_bytes());
        tcp.extend_from_slice(&seq.to_be_bytes());
        tcp.extend_from_slice(&ack.to_be_bytes());
        tcp.push(6 << 4); // data_off = 24 bytes
        tcp.push(TCP_SYN | TCP_ACK);
        tcp.extend_from_slice(&65535u16.to_be_bytes());
        tcp.extend_from_slice(&[0, 0, 0, 0]);
        tcp.extend_from_slice(&[2, 4, 0x05, 0xB4]); // MSS 1460
        let csum = transport_checksum(src_ip, dst_ip, 6, &tcp);
        tcp[16..18].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(src_ip, dst_ip, 6, &tcp);
    }

    #[allow(clippy::too_many_arguments)]
    fn enqueue_tcp_segment(
        &mut self,
        src_ip: [u8; 4],
        src_port: u16,
        dst_ip: [u8; 4],
        dst_port: u16,
        seq: u32,
        ack: u32,
        flags: u8,
        payload: &[u8],
    ) {
        let mut tcp = Vec::with_capacity(20 + payload.len());
        tcp.extend_from_slice(&src_port.to_be_bytes());
        tcp.extend_from_slice(&dst_port.to_be_bytes());
        tcp.extend_from_slice(&seq.to_be_bytes());
        tcp.extend_from_slice(&ack.to_be_bytes());
        tcp.push(5 << 4); // 20 bytes
        tcp.push(flags);
        tcp.extend_from_slice(&65535u16.to_be_bytes());
        tcp.extend_from_slice(&[0, 0, 0, 0]);
        tcp.extend_from_slice(payload);
        let csum = transport_checksum(src_ip, dst_ip, 6, &tcp);
        tcp[16..18].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(src_ip, dst_ip, 6, &tcp);
    }

    fn enqueue_ipv4(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4], proto: u8, body: &[u8]) {
        self.ip_ident = self.ip_ident.wrapping_add(1);
        let total_len = (20 + body.len()) as u16;
        let mut frame = Vec::with_capacity(14 + total_len as usize);
        let dst_mac = if dst_ip == [255, 255, 255, 255] {
            [0xFF; 6]
        } else {
            self.guest_mac
        };
        frame.extend_from_slice(&dst_mac);
        frame.extend_from_slice(&GATEWAY_MAC);
        frame.extend_from_slice(&ETH_IPV4.to_be_bytes());

        let hdr_start = frame.len();
        frame.push(0x45);
        frame.push(0);
        frame.extend_from_slice(&total_len.to_be_bytes());
        frame.extend_from_slice(&self.ip_ident.to_be_bytes());
        frame.extend_from_slice(&0x4000u16.to_be_bytes());
        frame.push(64);
        frame.push(proto);
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&src_ip);
        frame.extend_from_slice(&dst_ip);
        let csum = internet_checksum(&frame[hdr_start..hdr_start + 20]);
        frame[hdr_start + 10..hdr_start + 12].copy_from_slice(&csum.to_be_bytes());
        frame.extend_from_slice(body);
        self.rx_pending.push_back(frame);
    }

    #[cfg(test)]
    pub(crate) fn pop_rx_frame_for_test(&mut self) -> Option<Vec<u8>> {
        self.rx_pending.pop_front()
    }
}

fn detect_host_ipv4_dns() -> Option<Ipv4Addr> {
    let raw = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    for line in raw.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("nameserver")
            && let Ok(ip) = rest.trim().parse::<Ipv4Addr>()
            && !ip.is_loopback()
        {
            return Some(ip);
        }
    }
    None
}

fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn transport_checksum(src: [u8; 4], dst: [u8; 4], proto: u8, segment: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in [src, dst] {
        sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
        sum += u32::from(u16::from_be_bytes([pair[2], pair[3]]));
    }
    sum += u32::from(proto);
    sum += segment.len() as u32;
    for chunk in segment.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let res = !(sum as u16);
    if proto == 17 && res == 0 { 0xFFFF } else { res }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap_ipv4(src: [u8; 4], dst: [u8; 4], proto: u8, body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&GATEWAY_MAC);
        frame.extend_from_slice(&GUEST_MAC);
        frame.extend_from_slice(&ETH_IPV4.to_be_bytes());
        let total_len = (20 + body.len()) as u16;
        let hdr_start = frame.len();
        frame.extend_from_slice(&[0x45, 0]);
        frame.extend_from_slice(&total_len.to_be_bytes());
        frame.extend_from_slice(&[0, 1, 0x40, 0, 64, proto, 0, 0]);
        frame.extend_from_slice(&src);
        frame.extend_from_slice(&dst);
        let csum = internet_checksum(&frame[hdr_start..hdr_start + 20]);
        frame[hdr_start + 10..hdr_start + 12].copy_from_slice(&csum.to_be_bytes());
        frame.extend_from_slice(body);
        frame
    }

    #[test]
    fn handles_arp_dhcp_icmp_and_loopback_tcp_proxy() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 32];
            let n = s.read(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"PING");
            s.write_all(b"PONG").unwrap();
        });

        let mut net = VirtioNetDevice::new(&[]).unwrap();

        // 1. ARP request for 10.0.2.2
        let mut arp = Vec::new();
        arp.extend_from_slice(&[0xFF; 6]);
        arp.extend_from_slice(&GUEST_MAC);
        arp.extend_from_slice(&ETH_ARP.to_be_bytes());
        arp.extend_from_slice(&1u16.to_be_bytes());
        arp.extend_from_slice(&ETH_IPV4.to_be_bytes());
        arp.extend_from_slice(&[6, 4, 0, 1]);
        arp.extend_from_slice(&GUEST_MAC);
        arp.extend_from_slice(&GUEST_IP);
        arp.extend_from_slice(&[0; 6]);
        arp.extend_from_slice(&GATEWAY_IP);
        net.handle_guest_frame(&arp);
        let arp_reply = net.pop_rx_frame_for_test().expect("ARP reply");
        assert_eq!(u16::from_be_bytes([arp_reply[12], arp_reply[13]]), ETH_ARP);
        assert_eq!(&arp_reply[22..28], &GATEWAY_MAC);

        // 2. DHCP DISCOVER -> DHCPOFFER granting 10.0.2.15
        let mut bootp = vec![0u8; 244];
        bootp[0] = 1;
        bootp[4..8].copy_from_slice(&[1, 2, 3, 4]);
        bootp[236..240].copy_from_slice(&[99, 130, 83, 99]);
        bootp[240..244].copy_from_slice(&[53, 1, 1, 255]);
        let mut udp = Vec::new();
        udp.extend_from_slice(&68u16.to_be_bytes());
        udp.extend_from_slice(&67u16.to_be_bytes());
        udp.extend_from_slice(&((8 + bootp.len()) as u16).to_be_bytes());
        udp.extend_from_slice(&[0, 0]);
        udp.extend_from_slice(&bootp);
        let dhcp_frame = wrap_ipv4([0, 0, 0, 0], [255, 255, 255, 255], 17, &udp);
        net.handle_guest_frame(&dhcp_frame);
        let offer = net.pop_rx_frame_for_test().expect("DHCP offer");
        assert_eq!(&offer[14 + 28 + 16..14 + 28 + 20], &GUEST_IP);

        // 3. ICMP Echo Request -> Reply
        let mut icmp = vec![8, 0, 0, 0, 0, 1, 0, 1, b'h', b'i'];
        let icsum = internet_checksum(&icmp);
        icmp[2..4].copy_from_slice(&icsum.to_be_bytes());
        net.handle_guest_frame(&wrap_ipv4(GUEST_IP, GATEWAY_IP, 1, &icmp));
        let pong = net.pop_rx_frame_for_test().expect("ICMP echo reply");
        assert_eq!(pong[14 + 20], 0);

        // 4. Guest TCP SYN -> 10.0.2.2:<port> -> writes PING -> reads PONG
        let mut syn = vec![0u8; 20];
        syn[0..2].copy_from_slice(&50000u16.to_be_bytes());
        syn[2..4].copy_from_slice(&port.to_be_bytes());
        syn[4..8].copy_from_slice(&100u32.to_be_bytes());
        syn[12] = 5 << 4;
        syn[13] = TCP_SYN;
        net.handle_guest_frame(&wrap_ipv4(GUEST_IP, GATEWAY_IP, 6, &syn));
        let synack = net.pop_rx_frame_for_test().expect("SYN-ACK");
        assert_eq!(synack[14 + 20 + 13], TCP_SYN | TCP_ACK);

        let mut psh = vec![0u8; 24];
        psh[0..2].copy_from_slice(&50000u16.to_be_bytes());
        psh[2..4].copy_from_slice(&port.to_be_bytes());
        psh[4..8].copy_from_slice(&101u32.to_be_bytes());
        psh[12] = 5 << 4;
        psh[13] = TCP_PSH | TCP_ACK;
        psh[20..24].copy_from_slice(b"PING");
        net.handle_guest_frame(&wrap_ipv4(GUEST_IP, GATEWAY_IP, 6, &psh));
        let _ack = net.pop_rx_frame_for_test().expect("ACK");

        let mut got = None;
        for _ in 0..50 {
            net.poll_tcp_flows();
            if let Some(f) = net.pop_rx_frame_for_test()
                && f.len() > 14 + 20 + 20
            {
                got = Some(f[14 + 20 + 20..].to_vec());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got.as_deref(), Some(&b"PONG"[..]));
        srv.join().unwrap();
    }
}
