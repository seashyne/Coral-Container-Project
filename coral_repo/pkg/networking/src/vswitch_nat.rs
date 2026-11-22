//! Pure-Rust Userspace L3 Outbound NAT Engine (UDP + TCP proxy) embedded in `RootlessVSwitch`.
//! Routes external IPv4 traffic (and `host.coral.internal` / `.254` -> host `127.0.0.1`)
//! from isolated rootless `CLONE_NEWUSER | CLONE_NEWNET` containers without root or `pasta`.

use crate::vswitch_gw::{GATEWAY_MAC, internet_checksum, udp_ipv4_checksum, wrap_ipv4_frame};
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

const ETH_TYPE_IPV4: u16 = 0x0800;
const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;
const FLOW_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
struct TcpFlowKey {
    src_ip: Ipv4Addr,
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
}

struct NatUdpFlow {
    ctr_id: String,
    ctr_mac: [u8; 6],
    ctr_ip: Ipv4Addr,
    ctr_port: u16,
    logical_dst_ip: Ipv4Addr,
    socket: UdpSocket,
    last_active: Instant,
}

struct NatTcpFlow {
    ctr_id: String,
    ctr_mac: [u8; 6],
    stream: TcpStream,
    our_seq: u32,
    peer_next_seq: u32,
    closed: bool,
    last_active: Instant,
}

/// Pure-Rust userspace UDP/TCP NAT router attached to a `RootlessVSwitch` subnet.
pub(crate) struct UserspaceNatEngine {
    subnet_base: u32,
    subnet_mask: u32,
    host_alias_ip: Ipv4Addr,
    udp_flows: HashMap<(String, u16, Ipv4Addr, u16), NatUdpFlow>,
    tcp_flows: HashMap<TcpFlowKey, NatTcpFlow>,
    seq_seed: u32,
}

impl UserspaceNatEngine {
    pub fn new(subnet_base: u32, netmask: Ipv4Addr) -> Self {
        let mask_u32 = u32::from(netmask);
        // Reserve `.254` (second-to-last address in subnet) as `host.coral.internal` -> host 127.0.0.1.
        let host_alias_ip = Ipv4Addr::from(subnet_base | 254);
        Self {
            subnet_base,
            subnet_mask: mask_u32,
            host_alias_ip,
            udp_flows: HashMap::new(),
            tcp_flows: HashMap::new(),
            seq_seed: 0x1000_0001,
        }
    }

    /// Returns `true` when `frame` is an outbound IPv4 packet destined for NAT (external IP or `.254` host alias).
    pub fn handle_outbound_frame(
        &mut self,
        ctr_id: &str,
        frame: &[u8],
        ctr_fd: i32,
    ) -> bool {
        if frame.len() < 14 + 20 {
            return false;
        }
        if frame[0..6] != GATEWAY_MAC {
            return false;
        }
        let eth_type = u16::from_be_bytes([frame[12], frame[13]]);
        if eth_type != ETH_TYPE_IPV4 {
            return false;
        }
        let ip = &frame[14..];
        if (ip[0] >> 4) != 4 {
            return false;
        }
        let ihl = ((ip[0] & 0x0F) as usize) * 4;
        let total_len = u16::from_be_bytes([ip[2], ip[3]]) as usize;
        if ihl < 20 || total_len < ihl || ip.len() < total_len {
            return false;
        }
        let src_ip = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
        let dst_ip = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
        let is_local_subnet = (u32::from(dst_ip) & self.subnet_mask) == self.subnet_base;
        let is_host_alias = dst_ip == self.host_alias_ip;
        if is_local_subnet && !is_host_alias {
            return false;
        }

        let mut ctr_mac = [0u8; 6];
        ctr_mac.copy_from_slice(&frame[6..12]);
        let proto = ip[9];
        let payload = &ip[ihl..total_len];
        let real_dst_ip = if is_host_alias {
            Ipv4Addr::LOCALHOST
        } else {
            dst_ip
        };

        match proto {
            17 => {
                self.handle_udp_out(ctr_id, ctr_mac, src_ip, dst_ip, real_dst_ip, payload);
                true
            }
            6 => {
                self.handle_tcp_out(ctr_id, ctr_mac, src_ip, dst_ip, real_dst_ip, payload, ctr_fd);
                true
            }
            _ => false,
        }
    }

    /// Drains ready replies from host UDP/TCP NAT sockets and injects frames into container TAP fds.
    pub fn poll_replies(&mut self, ports: &HashMap<String, OwnedFd>) {
        let now = Instant::now();
        let mut buf = [0u8; 1400];

        // 1. Poll UDP replies
        self.udp_flows.retain(|_, flow| {
            if now.duration_since(flow.last_active) > FLOW_IDLE_TIMEOUT {
                return false;
            }
            let Some(tap_fd) = ports.get(&flow.ctr_id) else {
                return false;
            };
            while let Ok((n, peer)) = flow.socket.recv_from(&mut buf) {
                flow.last_active = now;
                let src_port = peer.port();
                let frame = build_udp_reply_frame(
                    &flow.ctr_mac,
                    flow.logical_dst_ip,
                    src_port,
                    flow.ctr_ip,
                    flow.ctr_port,
                    &buf[..n],
                );
                write_tap(tap_fd.as_raw_fd(), &frame);
            }
            true
        });

        // 2. Poll TCP replies
        self.tcp_flows.retain(|key, flow| {
            if flow.closed || now.duration_since(flow.last_active) > FLOW_IDLE_TIMEOUT {
                return false;
            }
            let Some(tap_fd) = ports.get(&flow.ctr_id) else {
                return false;
            };
            loop {
                match flow.stream.read(&mut buf) {
                    Ok(0) => {
                        send_tcp_reply(key, &flow.ctr_mac, flow.our_seq, flow.peer_next_seq, TCP_FIN | TCP_ACK, &[], tap_fd.as_raw_fd());
                        flow.our_seq = flow.our_seq.wrapping_add(1);
                        flow.closed = true;
                        return false;
                    }
                    Ok(n) => {
                        flow.last_active = now;
                        send_tcp_reply(key, &flow.ctr_mac, flow.our_seq, flow.peer_next_seq, TCP_PSH | TCP_ACK, &buf[..n], tap_fd.as_raw_fd());
                        flow.our_seq = flow.our_seq.wrapping_add(n as u32);
                    }
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        send_tcp_reply(key, &flow.ctr_mac, flow.our_seq, flow.peer_next_seq, TCP_RST | TCP_ACK, &[], tap_fd.as_raw_fd());
                        return false;
                    }
                }
            }
            true
        });
    }

    fn handle_udp_out(
        &mut self,
        ctr_id: &str,
        ctr_mac: [u8; 6],
        src_ip: Ipv4Addr,
        logical_dst_ip: Ipv4Addr,
        real_dst_ip: Ipv4Addr,
        udp: &[u8],
    ) {
        if udp.len() < 8 {
            return;
        }
        let src_port = u16::from_be_bytes([udp[0], udp[1]]);
        let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
        let data = &udp[8..];
        let key = (ctr_id.to_string(), src_port, logical_dst_ip, dst_port);

        let flow = self.udp_flows.entry(key).or_insert_with(|| {
            let sock = UdpSocket::bind("0.0.0.0:0").unwrap_or_else(|_| {
                UdpSocket::bind("127.0.0.1:0").expect("bind nat udp socket")
            });
            let _ = sock.set_nonblocking(true);
            NatUdpFlow {
                ctr_id: ctr_id.to_string(),
                ctr_mac,
                ctr_ip: src_ip,
                ctr_port: src_port,
                logical_dst_ip,
                socket: sock,
                last_active: Instant::now(),
            }
        });
        flow.last_active = Instant::now();
        let _ = flow
            .socket
            .send_to(data, SocketAddr::V4(SocketAddrV4::new(real_dst_ip, dst_port)));
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_tcp_out(
        &mut self,
        ctr_id: &str,
        ctr_mac: [u8; 6],
        src_ip: Ipv4Addr,
        logical_dst_ip: Ipv4Addr,
        real_dst_ip: Ipv4Addr,
        tcp: &[u8],
        ctr_fd: i32,
    ) {
        if tcp.len() < 20 {
            return;
        }
        let src_port = u16::from_be_bytes([tcp[0], tcp[1]]);
        let dst_port = u16::from_be_bytes([tcp[2], tcp[3]]);
        let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
        let data_off = ((tcp[12] >> 4) as usize) * 4;
        if data_off < 20 || data_off > tcp.len() {
            return;
        }
        let flags = tcp[13];
        let payload = &tcp[data_off..];
        let key = TcpFlowKey {
            src_ip,
            src_port,
            dst_ip: logical_dst_ip,
            dst_port,
        };

        if (flags & TCP_RST) != 0 {
            self.tcp_flows.remove(&key);
            return;
        }

        if (flags & TCP_SYN) != 0 && (flags & TCP_ACK) == 0 {
            let target = SocketAddr::V4(SocketAddrV4::new(real_dst_ip, dst_port));
            match TcpStream::connect_timeout(&target, Duration::from_millis(500)) {
                Ok(stream) => {
                    let _ = stream.set_nonblocking(true);
                    let our_seq = self.seq_seed;
                    self.seq_seed = self.seq_seed.wrapping_add(65_537);
                    let peer_next = seq.wrapping_add(1);
                    send_tcp_reply(&key, &ctr_mac, our_seq, peer_next, TCP_SYN | TCP_ACK, &[], ctr_fd);
                    self.tcp_flows.insert(
                        key,
                        NatTcpFlow {
                            ctr_id: ctr_id.to_string(),
                            ctr_mac,
                            stream,
                            our_seq: our_seq.wrapping_add(1),
                            peer_next_seq: peer_next,
                            closed: false,
                            last_active: Instant::now(),
                        },
                    );
                }
                Err(_) => {
                    send_tcp_reply(&key, &ctr_mac, 0, seq.wrapping_add(1), TCP_RST | TCP_ACK, &[], ctr_fd);
                }
            }
            return;
        }

        if let Some(flow) = self.tcp_flows.get_mut(&key) {
            flow.last_active = Instant::now();
            if !payload.is_empty() && seq == flow.peer_next_seq {
                let _ = flow.stream.write_all(payload);
                flow.peer_next_seq = flow.peer_next_seq.wrapping_add(payload.len() as u32);
                send_tcp_reply(&key, &flow.ctr_mac, flow.our_seq, flow.peer_next_seq, TCP_ACK, &[], ctr_fd);
            }
            if (flags & TCP_FIN) != 0 {
                flow.peer_next_seq = flow.peer_next_seq.wrapping_add(1);
                send_tcp_reply(&key, &flow.ctr_mac, flow.our_seq, flow.peer_next_seq, TCP_FIN | TCP_ACK, &[], ctr_fd);
                let _ = flow.stream.shutdown(std::net::Shutdown::Both);
                flow.closed = true;
            }
        }
    }
}

fn send_tcp_reply(
    key: &TcpFlowKey,
    dst_mac: &[u8],
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
    tap_fd: i32,
) {
    let mut tcp = Vec::with_capacity(20 + payload.len());
    tcp.extend_from_slice(&key.dst_port.to_be_bytes());
    tcp.extend_from_slice(&key.src_port.to_be_bytes());
    tcp.extend_from_slice(&seq.to_be_bytes());
    tcp.extend_from_slice(&ack.to_be_bytes());
    tcp.push(0x50);
    tcp.push(flags);
    tcp.extend_from_slice(&65535u16.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes());
    tcp.extend_from_slice(&0u16.to_be_bytes());
    tcp.extend_from_slice(payload);

    let mut pseudo = Vec::with_capacity(12 + tcp.len());
    pseudo.extend_from_slice(&key.dst_ip.octets());
    pseudo.extend_from_slice(&key.src_ip.octets());
    pseudo.push(0);
    pseudo.push(6);
    pseudo.extend_from_slice(&(tcp.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(&tcp);
    let csum = internet_checksum(&pseudo);
    tcp[16..18].copy_from_slice(&csum.to_be_bytes());
    let frame = wrap_ipv4_frame(dst_mac, key.dst_ip, key.src_ip, 6, &tcp);
    write_tap(tap_fd, &frame);
}

fn build_udp_reply_frame(
    dst_mac: &[u8],
    src_ip: Ipv4Addr,
    src_port: u16,
    dst_ip: Ipv4Addr,
    dst_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = (8 + payload.len()) as u16;
    let mut udp = Vec::with_capacity(udp_len as usize);
    udp.extend_from_slice(&src_port.to_be_bytes());
    udp.extend_from_slice(&dst_port.to_be_bytes());
    udp.extend_from_slice(&udp_len.to_be_bytes());
    udp.extend_from_slice(&0u16.to_be_bytes());
    udp.extend_from_slice(payload);
    let csum = udp_ipv4_checksum(src_ip, dst_ip, &udp);
    udp[6..8].copy_from_slice(&csum.to_be_bytes());
    wrap_ipv4_frame(dst_mac, src_ip, dst_ip, 17, &udp)
}

fn write_tap(fd: i32, frame: &[u8]) {
    let _ = unsafe { libc::write(fd, frame.as_ptr().cast::<libc::c_void>(), frame.len()) };
}
