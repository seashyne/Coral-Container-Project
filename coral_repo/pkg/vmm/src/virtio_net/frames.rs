//! Guest-originated Ethernet frame dispatch: ARP, IPv4 demux, and ICMP echo.

use super::{VirtioNetDevice, ETH_ARP, ETH_IPV4, GATEWAY_IP, GATEWAY_MAC};

impl VirtioNetDevice {
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
        let csum = super::checksum::internet_checksum(&reply);
        reply[2..4].copy_from_slice(&csum.to_be_bytes());
        self.enqueue_ipv4(GATEWAY_IP, src_ip, 1, &reply);
    }
}
