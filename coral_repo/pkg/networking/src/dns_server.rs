//! Embedded RFC 1035 UDP DNS responder with multi-IP round-robin service discovery
//! and upstream host DNS forwarding for external hostnames.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Lightweight embedded DNS server mapping container names, service names, and network
/// aliases (plus `<name>.coral.local`) to one or more IPv4 addresses (round-robin),
/// forwarding unknown external queries to an upstream resolver.
#[derive(Debug)]
pub struct EmbeddedDnsServer {
    local_addr: SocketAddr,
    records: Arc<RwLock<HashMap<String, Vec<Ipv4Addr>>>>,
    rr_counter: Arc<AtomicUsize>,
    upstream: Arc<RwLock<Option<SocketAddr>>>,
    stop_flag: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl EmbeddedDnsServer {
    /// Binds an embedded UDP DNS responder on `bind_addr` (e.g. `127.0.0.1:0`),
    /// auto-detecting upstream nameservers from `/etc/resolv.conf`.
    pub fn start(bind_addr: SocketAddr) -> io::Result<Self> {
        let sock = UdpSocket::bind(bind_addr)?;
        sock.set_read_timeout(Some(Duration::from_millis(50)))?;
        let local_addr = sock.local_addr()?;
        let records = Arc::new(RwLock::new(HashMap::from([(
            "localhost".to_string(),
            vec![Ipv4Addr::LOCALHOST],
        )])));
        let rr_counter = Arc::new(AtomicUsize::new(0));
        let upstream = Arc::new(RwLock::new(detect_host_upstream_dns()));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let rec_clone = records.clone();
        let rr_clone = rr_counter.clone();
        let up_clone = upstream.clone();
        let stop_clone = stop_flag.clone();

        let worker = thread::Builder::new()
            .name("coral-dns".into())
            .spawn(move || serve_loop(sock, rec_clone, rr_clone, up_clone, stop_clone))?;

        Ok(Self {
            local_addr,
            records,
            rr_counter,
            upstream,
            stop_flag,
            worker: Some(worker),
        })
    }

    /// Overrides the upstream DNS resolver address (pass `None` to disable forwarding).
    pub fn set_upstream(&self, addr: Option<SocketAddr>) {
        if let Ok(mut guard) = self.upstream.write() {
            *guard = addr;
        }
    }

    /// Bound UDP socket address of the DNS server.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Registers a hostname or network alias (and `<name>.coral.local`) replacing prior IPs.
    pub fn register_host(&self, name: &str, ip: Ipv4Addr) {
        let key = normalize_dns_name(name);
        if key.is_empty() {
            return;
        }
        if let Ok(mut map) = self.records.write() {
            map.insert(key.clone(), vec![ip]);
            if !key.ends_with(".coral.local") {
                map.insert(format!("{key}.coral.local"), vec![ip]);
            }
        }
    }

    /// Appends an IPv4 address to `name` (enabling round-robin DNS across scaled replicas).
    pub fn add_host_ip(&self, name: &str, ip: Ipv4Addr) {
        let key = normalize_dns_name(name);
        if key.is_empty() {
            return;
        }
        if let Ok(mut map) = self.records.write() {
            let entry = map.entry(key.clone()).or_default();
            if !entry.contains(&ip) {
                entry.push(ip);
            }
            if !key.ends_with(".coral.local") {
                let fqdn_entry = map.entry(format!("{key}.coral.local")).or_default();
                if !fqdn_entry.contains(&ip) {
                    fqdn_entry.push(ip);
                }
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

    /// Resolves the primary IPv4 for a registered hostname in-memory.
    pub fn lookup(&self, name: &str) -> Option<Ipv4Addr> {
        let key = normalize_dns_name(name);
        self.records
            .read()
            .ok()?
            .get(&key)
            .and_then(|ips| ips.first().copied())
    }

    /// Resolves all IPv4 addresses registered for a hostname (for multi-replica services).
    pub fn lookup_all(&self, name: &str) -> Vec<Ipv4Addr> {
        let key = normalize_dns_name(name);
        self.records
            .read()
            .ok()
            .and_then(|map| map.get(&key).cloned())
            .unwrap_or_default()
    }

    /// Answers a raw RFC 1035 DNS query packet directly using local records (with round-robin)
    /// and upstream host DNS forwarding for external names.
    pub fn answer_query(&self, pkt: &[u8]) -> Option<Vec<u8>> {
        let up = self.upstream.read().ok().and_then(|g| *g);
        handle_dns_packet_with_upstream(pkt, &self.records, &self.rr_counter, up)
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

fn detect_host_upstream_dns() -> Option<SocketAddr> {
    let content = std::fs::read_to_string("/etc/resolv.conf").ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("nameserver")
            && let Ok(ip) = rest.trim().parse::<Ipv4Addr>()
        {
            return Some(SocketAddr::from((ip, 53)));
        }
    }
    None
}

fn serve_loop(
    sock: UdpSocket,
    records: Arc<RwLock<HashMap<String, Vec<Ipv4Addr>>>>,
    rr_counter: Arc<AtomicUsize>,
    upstream: Arc<RwLock<Option<SocketAddr>>>,
    stop: Arc<AtomicBool>,
) {
    let mut buf = [0u8; 512];
    while !stop.load(Ordering::Relaxed) {
        let Ok((n, peer)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let up = upstream.read().ok().and_then(|g| *g);
        if let Some(resp) = handle_dns_packet_with_upstream(&buf[..n], &records, &rr_counter, up) {
            let _ = sock.send_to(&resp, peer);
        }
    }
}

/// Parses an RFC 1035 DNS query packet against single-IP records (backwards-compatible helper).
pub fn handle_dns_packet(
    pkt: &[u8],
    records: &RwLock<HashMap<String, Ipv4Addr>>,
) -> Option<Vec<u8>> {
    let multi: HashMap<String, Vec<Ipv4Addr>> = records
        .read()
        .ok()?
        .iter()
        .map(|(k, &v)| (k.clone(), vec![v]))
        .collect();
    let multi_lock = RwLock::new(multi);
    let rr = AtomicUsize::new(0);
    handle_dns_packet_with_upstream(pkt, &multi_lock, &rr, None)
}

fn handle_dns_packet_with_upstream(
    pkt: &[u8],
    records: &RwLock<HashMap<String, Vec<Ipv4Addr>>>,
    rr_counter: &AtomicUsize,
    upstream: Option<SocketAddr>,
) -> Option<Vec<u8>> {
    if pkt.len() < 12 {
        return None;
    }
    let tx_id = [pkt[0], pkt[1]];
    let rd = pkt[2] & 0x01;
    if u16::from_be_bytes([pkt[4], pkt[5]]) == 0 {
        return None;
    }

    let (qname, pos_after_name) = parse_qname(pkt, 12)?;
    if pos_after_name + 4 > pkt.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([pkt[pos_after_name], pkt[pos_after_name + 1]]);
    let question_slice = &pkt[12..pos_after_name + 4];
    let norm = normalize_dns_name(&qname);

    let found_ips = records
        .read()
        .ok()
        .and_then(|map| map.get(&norm).cloned())
        .filter(|ips| !ips.is_empty());

    if let Some(mut ips) = found_ips {
        let mut out = Vec::with_capacity(64 + ips.len() * 16);
        out.extend_from_slice(&tx_id);
        if qtype == 1 || qtype == 255 {
            let rot = rr_counter.fetch_add(1, Ordering::Relaxed) % ips.len();
            ips.rotate_left(rot);
            out.extend_from_slice(&[0x84 | rd, 0x80]);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&(ips.len() as u16).to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(question_slice);
            for ip in ips {
                out.extend_from_slice(&[0xC0, 0x0C]);
                out.extend_from_slice(&1u16.to_be_bytes());
                out.extend_from_slice(&1u16.to_be_bytes());
                out.extend_from_slice(&5u32.to_be_bytes());
                out.extend_from_slice(&4u16.to_be_bytes());
                out.extend_from_slice(&ip.octets());
            }
        } else {
            out.extend_from_slice(&[0x84 | rd, 0x80]);
            out.extend_from_slice(&1u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(&0u16.to_be_bytes());
            out.extend_from_slice(question_slice);
        }
        return Some(out);
    }

    // Forward unknown external names (non-.coral.local) to upstream DNS if configured.
    if !norm.ends_with(".coral.local")
        && norm.contains('.')
        && let Some(up_addr) = upstream
        && let Some(forwarded) = forward_dns_udp(pkt, up_addr)
    {
        return Some(forwarded);
    }

    // NXDOMAIN (RCODE = 3)
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&tx_id);
    out.extend_from_slice(&[0x84 | rd, 0x83]);
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(question_slice);
    Some(out)
}

fn forward_dns_udp(pkt: &[u8], upstream: SocketAddr) -> Option<Vec<u8>> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_millis(1200))).ok()?;
    sock.send_to(pkt, upstream).ok()?;
    let mut buf = [0u8; 512];
    let (n, _) = sock.recv_from(&mut buf).ok()?;
    (n >= 12).then(|| buf[..n].to_vec())
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
    pkt.extend_from_slice(&[0x01, 0x00]);
    pkt.extend_from_slice(&1u16.to_be_bytes());
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
    pkt.extend_from_slice(&1u16.to_be_bytes());
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_dns_supports_round_robin_replicas_and_upstream_forwarding() {
        // 1. Mock upstream DNS server answering `external.test` -> 93.184.216.34
        let up_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let up_addr = up_sock.local_addr().unwrap();
        up_sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let up_thread = thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok((n, peer)) = up_sock.recv_from(&mut buf) {
                let mut map = HashMap::new();
                map.insert("external.test".to_string(), Ipv4Addr::new(93, 184, 216, 34));
                let lock = RwLock::new(map);
                if let Some(reply) = handle_dns_packet(&buf[..n], &lock) {
                    let _ = up_sock.send_to(&reply, peer);
                }
            }
        });

        let srv = EmbeddedDnsServer::start("127.0.0.1:0".parse().unwrap()).unwrap();
        srv.set_upstream(Some(up_addr));
        srv.add_host_ip("web", Ipv4Addr::new(10, 89, 1, 2));
        srv.add_host_ip("web", Ipv4Addr::new(10, 89, 1, 3));

        // 2. Round-robin DNS across scaled replicas (`ANCOUNT == 2`, rotating first IP)
        let q1 = build_dns_query_packet(0x1111, "web.coral.local", 1);
        let r1 = srv.answer_query(&q1).unwrap();
        let r2 = srv.answer_query(&q1).unwrap();
        assert_eq!(u16::from_be_bytes([r1[6], r1[7]]), 2);
        assert_eq!(u16::from_be_bytes([r2[6], r2[7]]), 2);
        // Last 4 bytes of r1 vs r2 differ because order rotated
        assert_ne!(&r1[r1.len() - 4..], &r2[r2.len() - 4..]);

        // 3. Upstream forwarding for unknown external domain
        let q_ext = build_dns_query_packet(0x2222, "external.test", 1);
        let r_ext = srv.answer_query(&q_ext).unwrap();
        assert_eq!(r_ext[3] & 0x0F, 0);
        assert_eq!(&r_ext[r_ext.len() - 4..], &[93, 184, 216, 34]);
        let _ = up_thread.join();
    }
}
