//! Polling of active host-side TCP/UDP flows and host->guest port-forward listeners.

use super::{FlowKey, TcpFlow, TcpState, VirtioNetDevice, GATEWAY_IP, GUEST_IP, TCP_ACK, TCP_FIN, TCP_PSH, TCP_RST, TCP_SYN};
use std::io::{ErrorKind, Read};
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

impl VirtioNetDevice {
    pub(super) fn poll_listeners(&mut self) {
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

    pub(super) fn poll_tcp_flows(&mut self) {
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

    pub(super) fn poll_udp_flows(&mut self) {
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

    pub(super) fn translate_target_ip(&self, dst_ip: [u8; 4], dst_port: u16) -> Ipv4Addr {
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
}
