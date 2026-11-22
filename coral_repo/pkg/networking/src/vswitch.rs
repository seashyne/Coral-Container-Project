//! Pure-Rust Rootless L2 Virtual Ethernet Switch (`RootlessVSwitch`) connecting isolated
//! `CLONE_NEWUSER | CLONE_NEWNET` containers on a private virtual subnet (`10.89.x.0/24`)
//! without root privileges or host port forwarding.

use crate::NetError;
use crate::dns_server::EmbeddedDnsServer;
use crate::vswitch_gw::handle_gateway_frame;
use crate::vswitch_nat::UserspaceNatEngine;
use crate::vswitch_tap::open_tap_in_netns;
use std::collections::HashMap;
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

/// Rootless L2 learning Ethernet switch for a custom Coral bridge network.
#[derive(Debug)]
pub struct RootlessVSwitch {
    subnet_base: u32,
    netmask: Ipv4Addr,
    host_capacity: u32,
    next_host: AtomicU32,
    ports: Arc<Mutex<HashMap<String, OwnedFd>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RootlessVSwitch {
    /// Creates a new rootless virtual Ethernet switch for `subnet_cidr` (e.g. `"10.89.0.0/24"`).
    pub fn start(subnet_cidr: &str) -> Result<Self, NetError> {
        Self::start_with_dns(subnet_cidr, None)
    }

    /// Creates a new rootless virtual switch with an in-subnet virtual gateway (`10.89.x.1`)
    /// answering ARP, ICMP Echo, and UDP DNS queries on `10.89.x.1:53`.
    pub fn start_with_dns(
        subnet_cidr: &str,
        dns: Option<Arc<EmbeddedDnsServer>>,
    ) -> Result<Self, NetError> {
        let (base_u32, mask, host_cap) = parse_ipv4_cidr(subnet_cidr)?;
        let gw_ip = Ipv4Addr::from(base_u32 | 1);
        let ports: Arc<Mutex<HashMap<String, OwnedFd>>> = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let ports_c = ports.clone();
        let stop_c = stop.clone();

        let worker = thread::Builder::new()
            .name("coral-vswitch".into())
            .spawn(move || switch_loop(ports_c, stop_c, base_u32, mask, gw_ip, dns))
            .map_err(NetError::Io)?;

        Ok(Self {
            subnet_base: base_u32,
            netmask: mask,
            host_capacity: host_cap,
            next_host: AtomicU32::new(2), // .1 reserved for virtual gateway
            ports,
            stop,
            worker: Some(worker),
        })
    }

    /// Returns the virtual gateway IPv4 address (`10.89.x.1`) for this subnet.
    pub fn gateway_ip(&self) -> Ipv4Addr {
        Ipv4Addr::from(self.subnet_base | 1)
    }

    /// Allocates the next unique IPv4 address on this virtual subnet (`.2`..`.254`).
    pub fn allocate_ip(&self) -> Ipv4Addr {
        let offset = self.next_host.fetch_add(1, Ordering::Relaxed);
        let usable = self.host_capacity.saturating_sub(2).max(1);
        let host_part = ((offset - 2) % usable) + 2;
        Ipv4Addr::from(self.subnet_base | host_part)
    }

    /// Attaches a container's isolated network namespace (`pid`) to this L2 switch by creating
    /// `coral0` with `ip` inside the container's netns and plugging its TAP fd into the switch.
    pub fn attach_container(
        &self,
        container_id: &str,
        pid: i32,
        ip: Ipv4Addr,
    ) -> io::Result<()> {
        let tap_fd = open_tap_in_netns(pid, "coral0", ip, self.netmask)?;
        if let Ok(mut map) = self.ports.lock() {
            map.insert(container_id.to_string(), tap_fd);
        }
        Ok(())
    }

    /// Detaches a container's TAP port from the virtual switch.
    pub fn detach_container(&self, container_id: &str) {
        if let Ok(mut map) = self.ports.lock() {
            map.remove(container_id);
        }
    }
}

impl Drop for RootlessVSwitch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn parse_ipv4_cidr(cidr: &str) -> Result<(u32, Ipv4Addr, u32), NetError> {
    let (ip_str, prefix_str) = cidr
        .trim()
        .split_once('/')
        .ok_or_else(|| NetError::Validation(format!("invalid CIDR '{cidr}'")))?;
    let ip: Ipv4Addr = ip_str
        .parse()
        .map_err(|_| NetError::Validation(format!("invalid IPv4 in CIDR '{cidr}'")))?;
    let prefix: u32 = prefix_str
        .parse()
        .map_err(|_| NetError::Validation(format!("invalid prefix in CIDR '{cidr}'")))?;
    if !(8..=30).contains(&prefix) {
        return Err(NetError::Validation(format!(
            "CIDR prefix /{prefix} must be between /8 and /30"
        )));
    }
    let mask_u32 = !0u32 << (32 - prefix);
    let base_u32 = u32::from(ip) & mask_u32;
    let host_cap = 1u32 << (32 - prefix);
    Ok((base_u32, Ipv4Addr::from(mask_u32), host_cap))
}

fn switch_loop(
    ports: Arc<Mutex<HashMap<String, OwnedFd>>>,
    stop: Arc<AtomicBool>,
    subnet_base: u32,
    netmask: Ipv4Addr,
    gw_ip: Ipv4Addr,
    dns: Option<Arc<EmbeddedDnsServer>>,
) {
    let mut mac_table: HashMap<[u8; 6], String> = HashMap::new();
    let mut nat = UserspaceNatEngine::new(subnet_base, netmask);
    let mut frame_buf = [0u8; 2048];

    while !stop.load(Ordering::Relaxed) {
        let snapshot: Vec<(String, i32)> = {
            let Ok(guard) = ports.lock() else {
                break;
            };
            guard
                .iter()
                .map(|(k, fd)| (k.clone(), fd.as_raw_fd()))
                .collect()
        };
        if snapshot.is_empty() {
            thread::sleep(std::time::Duration::from_millis(15));
            continue;
        }

        let mut pfds: Vec<libc::pollfd> = snapshot
            .iter()
            .map(|(_, fd)| libc::pollfd {
                fd: *fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();

        let ready = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, 5) };
        let Ok(guard) = ports.lock() else {
            break;
        };
        nat.poll_replies(&guard);
        if ready <= 0 {
            continue;
        }
        for (idx, pfd) in pfds.iter().enumerate() {
            if (pfd.revents & libc::POLLIN) == 0 {
                continue;
            }
            let src_id = &snapshot[idx].0;
            let Some(src_fd) = guard.get(src_id) else {
                continue;
            };
            loop {
                let n = unsafe {
                    libc::read(
                        src_fd.as_raw_fd(),
                        frame_buf.as_mut_ptr().cast::<libc::c_void>(),
                        frame_buf.len(),
                    )
                };
                if n < 14 {
                    break;
                }
                let frame = &frame_buf[..n as usize];
                if nat.handle_outbound_frame(src_id, frame, src_fd.as_raw_fd()) {
                    continue;
                }
                if let Some(gw_reply) = handle_gateway_frame(frame, gw_ip, dns.as_deref()) {
                    let _ = unsafe {
                        libc::write(
                            src_fd.as_raw_fd(),
                            gw_reply.as_ptr().cast::<libc::c_void>(),
                            gw_reply.len(),
                        )
                    };
                    if (frame[0] & 1) == 0 {
                        continue;
                    }
                }
                let mut dst_mac = [0u8; 6];
                let mut src_mac = [0u8; 6];
                dst_mac.copy_from_slice(&frame[0..6]);
                src_mac.copy_from_slice(&frame[6..12]);
                mac_table.insert(src_mac, src_id.clone());

                let is_bcast_or_mcast = (dst_mac[0] & 1) == 1;
                if !is_bcast_or_mcast
                    && let Some(dst_id) = mac_table.get(&dst_mac)
                    && dst_id != src_id
                    && let Some(dst_fd) = guard.get(dst_id)
                {
                    let _ = unsafe {
                        libc::write(
                            dst_fd.as_raw_fd(),
                            frame.as_ptr().cast::<libc::c_void>(),
                            frame.len(),
                        )
                    };
                    continue;
                }

                // Flood broadcast, multicast, or unknown unicast to all peer ports.
                for (peer_id, peer_fd) in guard.iter() {
                    if peer_id != src_id {
                        let _ = unsafe {
                            libc::write(
                                peer_fd.as_raw_fd(),
                                frame.as_ptr().cast::<libc::c_void>(),
                                frame.len(),
                            )
                        };
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    #[test]
    fn rootless_vswitch_routes_direct_container_to_container_tcp_across_isolated_netns() {
        let vswitch = RootlessVSwitch::start("10.89.77.0/24").unwrap();
        let ip_a = vswitch.allocate_ip();
        let ip_b = vswitch.allocate_ip();
        assert_eq!(ip_a, Ipv4Addr::new(10, 89, 77, 2));
        assert_eq!(ip_b, Ipv4Addr::new(10, 89, 77, 3));

        // Spawn two isolated CLONE_NEWUSER | CLONE_NEWNET children communicating over 10.89.77.0/24.
        let mut ready_b = [-1i32; 2];
        let mut go_b = [-1i32; 2];
        let mut ready_a = [-1i32; 2];
        let mut go_a = [-1i32; 2];
        let mut out_a = [-1i32; 2];
        unsafe {
            assert_eq!(libc::pipe(ready_b.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(go_b.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(ready_a.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(go_a.as_mut_ptr()), 0);
            assert_eq!(libc::pipe(out_a.as_mut_ptr()), 0);
        }

        // Child B: TCP server on 0.0.0.0:9090 inside its own isolated netns
        let pid_b = unsafe { libc::fork() };
        if pid_b == 0 {
            unsafe {
                libc::close(ready_b[0]);
                libc::close(go_b[1]);
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                    libc::_exit(2);
                }
                let _ = crate::bring_loopback_up();
                let b = [1u8];
                libc::write(ready_b[1], b.as_ptr().cast(), 1);
                let mut g = [0u8];
                libc::read(go_b[0], g.as_mut_ptr().cast(), 1);
            }
            if let Ok(listener) = TcpListener::bind(("0.0.0.0", 9090))
                && let Ok((mut s, _)) = listener.accept()
            {
                let _ = s.write_all(b"vswitch-l2-ok");
                let _ = s.flush();
                thread::sleep(Duration::from_millis(50));
            }
            unsafe { libc::_exit(0) };
        }

        // Child A: TCP client connecting directly to 10.89.77.3:9090 inside its own isolated netns
        let pid_a = unsafe { libc::fork() };
        if pid_a == 0 {
            unsafe {
                libc::close(ready_a[0]);
                libc::close(go_a[1]);
                libc::close(out_a[0]);
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                    libc::_exit(2);
                }
                let _ = crate::bring_loopback_up();
                let b = [1u8];
                libc::write(ready_a[1], b.as_ptr().cast(), 1);
                let mut g = [0u8];
                libc::read(go_a[0], g.as_mut_ptr().cast(), 1);
            }
            for _ in 0..40 {
                if let Ok(mut s) = TcpStream::connect_timeout(
                    &format!("{ip_b}:9090").parse().unwrap(),
                    Duration::from_millis(200),
                ) {
                    let mut buf = [0u8; 64];
                    if let Ok(n) = s.read(&mut buf) {
                        unsafe {
                            libc::write(out_a[1], buf.as_ptr().cast(), n);
                        }
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(25));
            }
            unsafe { libc::_exit(0) };
        }

        // Parent: wait for both children to unshare, attach both to the rootless L2 vswitch, release them.
        unsafe {
            libc::close(ready_b[1]);
            libc::close(go_b[0]);
            libc::close(ready_a[1]);
            libc::close(go_a[0]);
            libc::close(out_a[1]);
            let mut b = [0u8];
            libc::read(ready_b[0], b.as_mut_ptr().cast(), 1);
            libc::read(ready_a[0], b.as_mut_ptr().cast(), 1);
        }

        vswitch.attach_container("ctr-b", pid_b, ip_b).unwrap();
        vswitch.attach_container("ctr-a", pid_a, ip_a).unwrap();

        unsafe {
            let go = [1u8];
            libc::write(go_b[1], go.as_ptr().cast(), 1);
            thread::sleep(Duration::from_millis(50));
            libc::write(go_a[1], go.as_ptr().cast(), 1);
        }

        let mut received = [0u8; 64];
        let n = unsafe { libc::read(out_a[0], received.as_mut_ptr().cast(), received.len()) };
        let mut st = 0;
        unsafe {
            libc::waitpid(pid_a, &raw mut st, 0);
            libc::waitpid(pid_b, &raw mut st, 0);
            for fd in [ready_b[0], go_b[1], ready_a[0], go_a[1], out_a[0]] {
                libc::close(fd);
            }
        }
        vswitch.detach_container("ctr-a");
        vswitch.detach_container("ctr-b");

        assert!(n > 0, "expected TCP response over rootless L2 vswitch");
        assert_eq!(&received[..n as usize], b"vswitch-l2-ok");
    }
}
