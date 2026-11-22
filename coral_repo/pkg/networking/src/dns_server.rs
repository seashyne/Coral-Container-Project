//! Embedded RFC 1035 UDP DNS responder for container & network-alias service discovery.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Lightweight embedded DNS server mapping container names, service names, and network
/// aliases (plus `<name>.coral.local`) to IPv4 addresses.
#[derive(Debug)]
pub struct EmbeddedDnsServer {
    local_addr: SocketAddr,
    records: Arc<RwLock<HashMap<String, Ipv4Addr>>>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl EmbeddedDnsServer {
    /// Binds an embedded UDP DNS responder on `bind_addr` (e.g. `127.0.0.1:0`).
    pub fn start(bind_addr: SocketAddr) -> io::Result<Self> {
        let sock = UdpSocket::bind(bind_addr)?;
        sock.set_read_timeout(Some(Duration::from_millis(50)))?;
        let local_addr = sock.local_addr()?;
        let records = Arc::new(RwLock::new(HashMap::from([(
            "localhost".to_string(),
            Ipv4Addr::LOCALHOST,
        )])));
        let stop_flag = Arc::new(AtomicBool::new(false));
        let rec_clone = records.clone();
        let stop_clone = stop_flag.clone();

        let worker = thread::Builder::new()
            .name("coral-dns".into())
            .spawn(move || serve_loop(sock, rec_clone, stop_clone))?;

        Ok(Self {
            local_addr,
            records,
            stop_flag,
            worker: Some(worker),
        })
    }

    /// Bound UDP socket address of the DNS server.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Registers a hostname or network alias (and `<name>.coral.local`) pointing to `ip`.
    pub fn register_host(&self, name: &str, ip: Ipv4Addr) {
        let key = normalize_dns_name(name);
        if key.is_empty() {
            return;
        }
        if let Ok(mut map) = self.records.write() {
            map.insert(key.clone(), ip);
            if !key.ends_with(".coral.local") {
                map.insert(format!("{key}.coral.local"), ip);
            }
        }
    }

    /// Removes a hostname or alias from the embedded DNS registry.
    pub fn unregister_host(&self, name: &str) {
        let key = normalize_dns_name(name);
        if let Ok(mut map) = self.records.write() {
            map.remove(&key);
            map.remove(&format!("{key}.coral.local"));
        }
    }

    /// Resolves a registered hostname in-memory.
    pub fn lookup(&self, name: &str) -> Option<Ipv4Addr> {
        let key = normalize_dns_name(name);
        self.records.read().ok()?.get(&key).copied()
    }
}

impl Drop for EmbeddedDnsServer {
    fn drop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn normalize_dns_name(raw: &str) -> String {
    raw.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn serve_loop(
    sock: UdpSocket,
    records: Arc<RwLock<HashMap<String, Ipv4Addr>>>,
    stop: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 512];
    while !stop.load(Ordering::Relaxed) {
        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
            continue;
        };
        if let Some(resp) = handle_dns_packet(&buf[..n], &records) {
            let _ = sock.send_to(&resp, peer);
        }
    }
}

/// Parses an RFC 1035 DNS query packet and builds an `A` / `AAAA` response.
pub fn handle_dns_packet(
    pkt: &[u8],
    records: &RwLock<HashMap<String, Ipv4Addr>>,
) -> Option<Vec<u8>> {
    if pkt.len() < 12 {
        return None;
    }
    let tx_id = [pkt[0], pkt[1]];
    let flags_hi = pkt[2];
    let rd = flags_hi & 0x01;
    let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
    if qdcount == 0 {
        return None;
    }

    let (qname, pos_after_name) = parse_qname(pkt, 12)?;
    if pos_after_name + 4 > pkt.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([pkt[pos_after_name], pkt[pos_after_name + 1]]);
    let question_end = pos_after_name + 4;
    let question_slice = &pkt[12..question_end];

    let found_ip = records
        .read()
        .ok()
        .and_then(|map| map.get(&normalize_dns_name(&qname)).copied());

    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&tx_id);

    match found_ip {
        Some(ip) if qtype == 1 || qtype == 255 => {
            // Standard response, Authoritative + Recursion Desired/Available, RCODE=0 (NOERROR)
            out.extend_from_slice(&[0x84 | rd, 0x80]);
            out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
            out.extend_from_slice(&1u16.to_be_bytes()); // ANCOUNT = 1
            out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
            out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0
            out.extend_from_slice(question_slice);
            // Answer RR: pointer to QNAME at offset 12 (0xC00C), TYPE A (1), CLASS IN (1), TTL 5s
            out.extend_from_slice(&[0xC0, 0x0C]);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&5u32.to_be_bytes());
            out.extend_from_slice(&4u16.to_be_bytes());
            out.extend_from_slice(&ip.octets());
        }
        Some(_) => {
            // Registered name but non-A query (e.g. AAAA=28): return NODATA (NOERROR, ANCOUNT=0)
            out.extend_from_slice(&[0x84 | rd, 0x80]);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(question_slice);
        }
        None => {
            // NXDOMAIN (RCODE = 3)
            out.extend_from_slice(&[0x84 | rd, 0x83]);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(question_slice);
        }
    }
    Some(out)
}

fn parse_qname(pkt: &[u8], mut idx: usize) -> Option<(String, usize)> {
    let mut labels = Vec::new();
    loop {
        if idx >= pkt.len() {
            return None;
        }
        let len = pkt[idx] as usize;
        idx += 1;
        if len == 0 {
            break;
        }
        if len > 63 || idx + len > pkt.len() {
            return None;
        }
        let label = std::str::from_utf8(&pkt[idx..idx + len]).ok()?;
        labels.push(label);
        idx += len;
    }
    Some((labels.join("."), idx))
}

/// Builds a minimal RFC 1035 DNS query packet for `host` and `qtype` (`1` = A, `28` = AAAA).
pub fn build_dns_query_packet(tx_id: u16, host: &str, qtype: u16) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(64);
    pkt.extend_from_slice(&tx_id.to_be_bytes());
    pkt.extend_from_slice(&[0x01, 0x00]); // RD=1
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
    pkt.extend_from_slice(&0u16.to_be_bytes());
    pkt.extend_from_slice(&0u16.to_be_bytes());
    pkt.extend_from_slice(&0u16.to_be_bytes());
    for label in host.trim_end_matches('.').split('.') {
        if !label.is_empty() && label.len() <= 63 {
            pkt.push(label.len() as u8);
            pkt.extend_from_slice(label.as_bytes());
        }
    }
    pkt.push(0);
    pkt.extend_from_slice(&qtype.to_be_bytes());
    pkt.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_dns_server_resolves_registered_services_and_aliases_over_udp() {
        let srv = EmbeddedDnsServer::start("127.0.0.1:0".parse().unwrap()).unwrap();
        srv.register_host("redis-cache", Ipv4Addr::new(127, 0, 0, 1));
        srv.register_host("cache-alias", Ipv4Addr::new(10, 88, 0, 5));

        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let q = build_dns_query_packet(0x1234, "cache-alias.coral.local", 1);
        client.send_to(&q, srv.local_addr()).unwrap();
        let mut resp = [0u8; 512];
        let (n, _) = client.recv_from(&mut resp).unwrap();
        assert!(n >= 16);
        assert_eq!(&resp[0..2], &[0x12, 0x34]);
        assert_eq!(resp[3] & 0x0F, 0); // NOERROR
        assert_eq!(u16::from_be_bytes([resp[6], resp[7]]), 1); // 1 answer
        assert_eq!(&resp[n - 4..n], &[10, 88, 0, 5]);
    }
}
