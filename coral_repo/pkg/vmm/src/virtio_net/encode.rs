//! Host->guest frame encoding: IPv4/UDP/TCP packet construction and checksums.

use super::{VirtioNetDevice, ETH_IPV4, GATEWAY_MAC};
use super::checksum::transport_checksum;

impl VirtioNetDevice {
    pub(super) fn enqueue_udp_packet(
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

    pub(super) fn enqueue_tcp_synack(
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
        tcp.push(super::TCP_SYN | super::TCP_ACK);
        tcp.extend_from_slice(&65535u16.to_be_bytes());
        tcp.extend_from_slice(&[0, 0, 0, 0]);
        tcp.extend_from_slice(&[2, 4, 0x05, 0xB4]); // MSS 1460
        let csum = transport_checksum(src_ip, dst_ip, 6, &tcp);
        tcp[16..18].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(src_ip, dst_ip, 6, &tcp);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn enqueue_tcp_segment(
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

    pub(super) fn enqueue_ipv4(&mut self, src_ip: [u8; 4], dst_ip: [u8; 4], proto: u8, body: &[u8]) {
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
        let csum = super::checksum::internet_checksum(&frame[hdr_start..hdr_start + 20]);
        frame[hdr_start + 10..hdr_start + 12].copy_from_slice(&csum.to_be_bytes());
        frame.extend_from_slice(body);
        self.rx_pending.push_back(frame);
    }
}
