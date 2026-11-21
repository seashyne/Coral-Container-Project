//! Unit tests for the VirtIO-Net userspace switch.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

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
    let csum = checksum::internet_checksum(&frame[hdr_start..hdr_start + 20]);
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
    let icsum = checksum::internet_checksum(&icmp);
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
