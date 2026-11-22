//! Pure-Rust L3 Virtual Gateway (`10.89.x.1`) inside `RootlessVSwitch`:
//! handles ARP resolution, ICMP Echo (`ping 10.89.x.1`), and in-subnet RFC 1035 UDP DNS
//! (`10.89.x.1:53`) directly over the container's TAP Ethernet frame stream.

use crate::dns_server::EmbeddedDnsServer;
use std::net::Ipv4Addr;

/// Locally-administered virtual MAC address for the `10.89.x.1` subnet gateway.
pub const GATEWAY_MAC: [u8; 6] = [0x02, 0xc0, 0xa1, 0x00, 0x00, 0x01];

const ETH_TYPE_IPV4: u16 = 0x0800;
const ETH_TYPE_ARP: u16 = 0x0806;

/// Inspects an incoming Ethernet frame from a container TAP port and returns a synthesized
/// reply frame when addressed to the virtual gateway (`gw_ip` / `GATEWAY_MAC`).
pub fn handle_gateway_frame(
    frame: &[u8],
    gw_ip: Ipv4Addr,
    dns: Option<&EmbeddedDnsServer>,
) -> Option<Vec<u8>> {
    if frame.len() < 14 {
        return None;
    }
    let eth_type = u16::from_be_bytes([frame[12], frame[13]]);
    match eth_type {
        ETH_TYPE_ARP => handle_arp(frame, gw_ip),
        ETH_TYPE_IPV4 => handle_ipv4(frame, gw_ip, dns),
        _ => None,
    }
}

fn handle_arp(frame: &[u8], gw_ip: Ipv4Addr) -> Option<Vec<u8>> {
    // Ethernet (14) + ARP IPv4 (28) = 42 bytes minimum
    if frame.len() < 42 {
        return None;
    }
    let arp = &frame[14..42];
    let htype = u16::from_be_bytes([arp[0], arp[1]]);
    let ptype = u16::from_be_bytes([arp[2], arp[3]]);
    let oper = u16::from_be_bytes([arp[6], arp[7]]);
    if htype != 1 || ptype != ETH_TYPE_IPV4 || oper != 1 {
        return None;
    }
    let target_ip = Ipv4Addr::new(arp[24], arp[25], arp[26], arp[27]);
    let oct = gw_ip.octets();
    let host_alias_ip = Ipv4Addr::new(oct[0], oct[1], oct[2], 254);
    if target_ip != gw_ip && target_ip != host_alias_ip {
        return None;
    }

    let sender_mac = &arp[8..14];
    let sender_ip = &arp[14..18];

    let mut reply = Vec::with_capacity(42);
    // Ethernet header
    reply.extend_from_slice(sender_mac);
    reply.extend_from_slice(&GATEWAY_MAC);
    reply.extend_from_slice(&ETH_TYPE_ARP.to_be_bytes());
    // ARP reply payload
    reply.extend_from_slice(&1u16.to_be_bytes()); // Ethernet
    reply.extend_from_slice(&ETH_TYPE_IPV4.to_be_bytes()); // IPv4
    reply.push(6); // HLEN
    reply.push(4); // PLEN
    reply.extend_from_slice(&2u16.to_be_bytes()); // OPER = Reply
    reply.extend_from_slice(&GATEWAY_MAC);
    reply.extend_from_slice(&target_ip.octets());
    reply.extend_from_slice(sender_mac);
    reply.extend_from_slice(sender_ip);
    Some(reply)
}

fn handle_ipv4(frame: &[u8], gw_ip: Ipv4Addr, dns: Option<&EmbeddedDnsServer>) -> Option<Vec<u8>> {
    if frame.len() < 14 + 20 {
        return None;
    }
    let src_mac = &frame[6..12];
    let ip = &frame[14..];
    if (ip[0] >> 4) != 4 {
        return None;
    }
    let ihl = ((ip[0] & 0x0F) as usize) * 4;
    if ihl < 20 || ip.len() < ihl {
        return None;
    }
    let total_len = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    if total_len < ihl || ip.len() < total_len {
        return None;
    }
    let dst_ip = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
    if dst_ip != gw_ip {
        return None;
    }
    let src_ip = Ipv4Addr::new(ip[12], ip[13], ip[14], ip[15]);
    let proto = ip[9];
    let payload = &ip[ihl..total_len];

    match proto {
        1 => handle_icmp_echo(src_mac, src_ip, gw_ip, payload),
        17 => handle_udp_dns(src_mac, src_ip, gw_ip, payload, dns),
        _ => None,
    }
}

fn handle_icmp_echo(
    peer_mac: &[u8],
    peer_ip: Ipv4Addr,
    gw_ip: Ipv4Addr,
    icmp: &[u8],
) -> Option<Vec<u8>> {
    if icmp.len() < 8 || icmp[0] != 8 {
        return None;
    }
    let mut reply_icmp = icmp.to_vec();
    reply_icmp[0] = 0; // Echo Reply
    reply_icmp[2] = 0;
    reply_icmp[3] = 0;
    let csum = internet_checksum(&reply_icmp);
    reply_icmp[2..4].copy_from_slice(&csum.to_be_bytes());
    Some(wrap_ipv4_frame(peer_mac, gw_ip, peer_ip, 1, &reply_icmp))
}

fn handle_udp_dns(
    peer_mac: &[u8],
    peer_ip: Ipv4Addr,
    gw_ip: Ipv4Addr,
    udp: &[u8],
    dns: Option<&EmbeddedDnsServer>,
) -> Option<Vec<u8>> {
    if udp.len() < 8 {
        return None;
    }
    let src_port = u16::from_be_bytes([udp[0], udp[1]]);
    let dst_port = u16::from_be_bytes([udp[2], udp[3]]);
    if dst_port != 53 {
        return None;
    }
    let dns_query = &udp[8..];
    let dns_reply = dns?.answer_query(dns_query)?;

    let udp_len = (8 + dns_reply.len()) as u16;
    let mut out_udp = Vec::with_capacity(udp_len as usize);
    out_udp.extend_from_slice(&53u16.to_be_bytes());
    out_udp.extend_from_slice(&src_port.to_be_bytes());
    out_udp.extend_from_slice(&udp_len.to_be_bytes());
    out_udp.extend_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    out_udp.extend_from_slice(&dns_reply);

    let udp_csum = udp_ipv4_checksum(gw_ip, peer_ip, &out_udp);
    out_udp[6..8].copy_from_slice(&udp_csum.to_be_bytes());

    Some(wrap_ipv4_frame(peer_mac, gw_ip, peer_ip, 17, &out_udp))
}

pub(crate) fn wrap_ipv4_frame(
    dst_mac: &[u8],
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    proto: u8,
    payload: &[u8],
) -> Vec<u8> {
    let total_len = (20 + payload.len()) as u16;
    let mut ip_hdr = [0u8; 20];
    ip_hdr[0] = 0x45; // IPv4, IHL=5
    ip_hdr[2..4].copy_from_slice(&total_len.to_be_bytes());
    ip_hdr[6] = 0x40; // DF
    ip_hdr[8] = 64; // TTL
    ip_hdr[9] = proto;
    ip_hdr[12..16].copy_from_slice(&src_ip.octets());
    ip_hdr[16..20].copy_from_slice(&dst_ip.octets());
    let hdr_csum = internet_checksum(&ip_hdr);
    ip_hdr[10..12].copy_from_slice(&hdr_csum.to_be_bytes());

    let mut frame = Vec::with_capacity(14 + 20 + payload.len());
    frame.extend_from_slice(dst_mac);
    frame.extend_from_slice(&GATEWAY_MAC);
    frame.extend_from_slice(&ETH_TYPE_IPV4.to_be_bytes());
    frame.extend_from_slice(&ip_hdr);
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let (chunks, rem) = data.as_chunks::<2>();
    for pair in chunks {
        sum += u16::from_be_bytes(*pair) as u32;
    }
    if let Some(&last) = rem.first() {
        sum += (last as u32) << 8;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub(crate) fn udp_ipv4_checksum(src: Ipv4Addr, dst: Ipv4Addr, udp: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + udp.len());
    pseudo.extend_from_slice(&src.octets());
    pseudo.extend_from_slice(&dst.octets());
    pseudo.push(0);
    pseudo.push(17);
    pseudo.extend_from_slice(&(udp.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(udp);
    let c = internet_checksum(&pseudo);
    if c == 0 { 0xFFFF } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns_server::build_dns_query_packet;

    #[test]
    fn gateway_answers_arp_icmp_and_udp_dns_frames() {
        let gw = Ipv4Addr::new(10, 89, 1, 1);
        let peer_ip = Ipv4Addr::new(10, 89, 1, 2);
        let peer_mac = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x02];

        // 1. ARP Request for 10.89.1.1
        let mut arp = Vec::with_capacity(42);
        arp.extend_from_slice(&[0xff; 6]);
        arp.extend_from_slice(&peer_mac);
        arp.extend_from_slice(&ETH_TYPE_ARP.to_be_bytes());
        arp.extend_from_slice(&1u16.to_be_bytes());
        arp.extend_from_slice(&ETH_TYPE_IPV4.to_be_bytes());
        arp.extend_from_slice(&[6, 4, 0, 1]);
        arp.extend_from_slice(&peer_mac);
        arp.extend_from_slice(&peer_ip.octets());
        arp.extend_from_slice(&[0; 6]);
        arp.extend_from_slice(&gw.octets());

        let arp_reply = handle_gateway_frame(&arp, gw, None).unwrap();
        assert_eq!(&arp_reply[0..6], &peer_mac);
        assert_eq!(&arp_reply[6..12], &GATEWAY_MAC);
        assert_eq!(u16::from_be_bytes([arp_reply[20], arp_reply[21]]), 2);

        // 2. UDP DNS Query to 10.89.1.1:53
        let dns = EmbeddedDnsServer::start("127.0.0.1:0".parse().unwrap()).unwrap();
        dns.register_host("api", Ipv4Addr::new(10, 89, 1, 5));
        let query = build_dns_query_packet(0x4242, "api.coral.local", 1);
        let mut udp = Vec::new();
        udp.extend_from_slice(&55555u16.to_be_bytes());
        udp.extend_from_slice(&53u16.to_be_bytes());
        udp.extend_from_slice(&((8 + query.len()) as u16).to_be_bytes());
        udp.extend_from_slice(&0u16.to_be_bytes());
        udp.extend_from_slice(&query);
        let mut req_frame = wrap_ipv4_frame(&GATEWAY_MAC, peer_ip, gw, 17, &udp);
        req_frame[6..12].copy_from_slice(&peer_mac);

        let dns_frame = handle_gateway_frame(&req_frame, gw, Some(&dns)).unwrap();
        assert_eq!(&dns_frame[0..6], &peer_mac);
        let dns_payload = &dns_frame[14 + 20 + 8..];
        assert_eq!(&dns_payload[0..2], &[0x42, 0x42]);
        assert_eq!(&dns_payload[dns_payload.len() - 4..], &[10, 89, 1, 5]);
    }

    #[test]
    fn rootless_vswitch_nat_routes_outbound_udp_and_tcp_from_isolated_netns() {
        use crate::RootlessVSwitch;
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream, UdpSocket};
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        // 1. Start host-side UDP echo server and TCP server on 127.0.0.1
        let udp_srv = UdpSocket::bind("127.0.0.1:0").unwrap();
        let udp_port = udp_srv.local_addr().unwrap().port();
        udp_srv
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let udp_handle = thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((n, peer)) = udp_srv.recv_from(&mut buf) {
                let mut reply = b"udp:".to_vec();
                reply.extend_from_slice(&buf[..n]);
                let _ = udp_srv.send_to(&reply, peer);
            }
        });

        let tcp_srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp_port = tcp_srv.local_addr().unwrap().port();
        let tcp_handle = thread::spawn(move || {
            if let Ok((mut s, _)) = tcp_srv.accept() {
                let mut buf = [0u8; 64];
                if let Ok(n) = s.read(&mut buf) {
                    let mut reply = b"tcp:".to_vec();
                    reply.extend_from_slice(&buf[..n]);
                    let _ = s.write_all(&reply);
                    let _ = s.flush();
                }
            }
        });

        let dns = Arc::new(EmbeddedDnsServer::start("127.0.0.1:0".parse().unwrap()).unwrap());
        dns.register_host("svc-nat", Ipv4Addr::new(10, 89, 88, 9));
        let vswitch = RootlessVSwitch::start_with_dns("10.89.88.0/24", Some(dns)).unwrap();
        let ctr_ip = vswitch.allocate_ip();

        let mut ready = [-1i32; 2];
        let mut go = [-1i32; 2];
        let mut out = [-1i32; 2];
        unsafe {
            assert_eq!(libc::pipe(ready.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(go.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(out.as_mut_ptr()), 0);
        }

        let pid = unsafe { libc::fork() };
        if pid == 0 {
            unsafe {
                libc::close(ready[0]);
                libc::close(go[1]);
                libc::close(out[0]);
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                    libc::_exit(2);
                }
                let _ = crate::bring_loopback_up();
                let b = [1u8];
                libc::write(ready[1], b.as_ptr().cast(), 1);
                let mut g = [0u8];
                libc::read(go[0], g.as_mut_ptr().cast(), 1);
            }
            let mut result = String::new();
            // A. Outbound UDP through NAT (`10.89.88.254:<udp_port>` -> host `127.0.0.1:<udp_port>`)
            if let Ok(u) = UdpSocket::bind("0.0.0.0:0") {
                let _ = u.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = u.send_to(b"hi", format!("10.89.88.254:{udp_port}"));
                let mut buf = [0u8; 64];
                if let Ok((n, _)) = u.recv_from(&mut buf) {
                    result.push_str(&String::from_utf8_lossy(&buf[..n]));
                    result.push('|');
                }
            }
            // B. Outbound TCP through NAT (`10.89.88.254:<tcp_port>` -> host `127.0.0.1:<tcp_port>`)
            if let Ok(mut t) = TcpStream::connect_timeout(
                &format!("10.89.88.254:{tcp_port}").parse().unwrap(),
                Duration::from_secs(2),
            ) {
                let _ = t.set_read_timeout(Some(Duration::from_secs(2)));
                let _ = t.write_all(b"ping");
                let mut buf = [0u8; 64];
                if let Ok(n) = t.read(&mut buf) {
                    result.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
            unsafe {
                libc::write(out[1], result.as_ptr().cast(), result.len());
                libc::_exit(0);
            }
        }

        unsafe {
            libc::close(ready[1]);
            libc::close(go[0]);
            libc::close(out[1]);
            let mut b = [0u8];
            libc::read(ready[0], b.as_mut_ptr().cast(), 1);
        }
        vswitch.attach_container("nat-ctr", pid, ctr_ip).unwrap();
        unsafe {
            let g = [1u8];
            libc::write(go[1], g.as_ptr().cast(), 1);
        }

        let mut received = [0u8; 128];
        let n = unsafe { libc::read(out[0], received.as_mut_ptr().cast(), received.len()) };
        let mut st = 0;
        unsafe {
            libc::waitpid(pid, &raw mut st, 0);
            for fd in [ready[0], go[1], out[0]] {
                libc::close(fd);
            }
        }
        vswitch.detach_container("nat-ctr");
        let _ = udp_handle.join();
        let _ = tcp_handle.join();

        let text = String::from_utf8_lossy(&received[..n.max(0) as usize]);
        assert_eq!(text, "udp:hi|tcp:ping");
    }
}
