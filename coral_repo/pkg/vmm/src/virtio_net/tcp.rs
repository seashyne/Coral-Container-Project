//! Guest TCP connection handling: outbound proxy to the host network stack.

use super::{FlowKey, TcpFlow, TcpState, VirtioNetDevice, GUEST_IP, TCP_ACK, TCP_FIN, TCP_RST, TCP_SYN};
use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, SocketAddrV4, TcpStream};
use std::time::{Duration, Instant};

impl VirtioNetDevice {
    pub(super) fn handle_tcp(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4], tcp: &[u8]) {
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
}
