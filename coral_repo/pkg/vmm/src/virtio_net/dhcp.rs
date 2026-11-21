//! Guest UDP relay and built-in DHCP server (`10.0.2.15/24`, gateway `10.0.2.2`).

use super::{FlowKey, UdpFlow, VirtioNetDevice, GUEST_IP, GATEWAY_IP, SUBNET_MASK};
use std::collections::hash_map::Entry;
use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::Instant;

impl VirtioNetDevice {
    pub(super) fn handle_udp(&mut self, _src_ip: [u8; 4], dst_ip: [u8; 4], udp: &[u8]) {
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
            Entry::Occupied(o) => o.into_mut(),
            Entry::Vacant(v) => {
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
}
