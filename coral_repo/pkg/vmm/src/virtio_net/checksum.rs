//! Internet checksum helpers and host DNS resolver discovery.

use std::net::Ipv4Addr;

pub(super) fn detect_host_ipv4_dns() -> Option<Ipv4Addr> {
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

pub(super) fn internet_checksum(data: &[u8]) -> u16 {
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

pub(super) fn transport_checksum(src: [u8; 4], dst: [u8; 4], proto: u8, segment: &[u8]) -> u16 {
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
