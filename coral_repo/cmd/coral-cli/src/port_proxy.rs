//! Forks dedicated background TCP & UDP forwarder processes (keeping the parent
//! single-threaded for POSIX-safe `fork`/`unshare`) and stops them when dropped.

use anyhow::{Context, Result};
use coral_net::{PortMapping, PortProtocol};
use coral_runtime::StateStore;
use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct ForkedPortProxies {
    pids: Vec<i32>,
}

fn wait_for_bridge_pid(is_bridge_net: bool, state_root: &Path, cid: &str) -> Option<i32> {
    if !is_bridge_net {
        return None;
    }
    let store = StateStore::new(state_root);
    for _ in 0..50 {
        if let Some(pid) = store.load(cid).ok().and_then(|st| st.pid) {
            return Some(pid);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    None
}

fn fork_proxy_child(run: impl FnOnce()) -> Result<i32> {
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(anyhow::anyhow!(
            "fork port forwarder failed: {}",
            io::Error::last_os_error()
        ));
    }
    if pid == 0 {
        run();
        unsafe { libc::_exit(0) };
    }
    Ok(pid)
}

impl ForkedPortProxies {
    pub fn start(
        specs: &[String],
        is_bridge_net: bool,
        state_root: &Path,
        container_id: &str,
    ) -> Result<Self> {
        let mut pids = Vec::new();
        for raw in specs {
            let pm = PortMapping::parse(raw)?;
            if !is_bridge_net && pm.host_port == pm.container_port && pm.bind_ip == "0.0.0.0" {
                continue;
            }
            let listen_addr = format!("{}:{}", pm.bind_ip, pm.host_port);
            let container_port = pm.container_port;
            let state_root_buf = state_root.to_path_buf();
            let cid_str = container_id.to_string();

            let pid = match pm.protocol {
                PortProtocol::Tcp => {
                    let listener = TcpListener::bind(&listen_addr)
                        .with_context(|| format!("bind TCP port forwarder on {listen_addr}"))?;
                    fork_proxy_child(move || {
                        run_tcp_proxy_loop(
                            listener,
                            is_bridge_net,
                            state_root_buf,
                            cid_str,
                            container_port,
                        );
                    })?
                }
                PortProtocol::Udp => {
                    let host_sock = UdpSocket::bind(&listen_addr)
                        .with_context(|| format!("bind UDP port forwarder on {listen_addr}"))?;
                    fork_proxy_child(move || {
                        run_udp_proxy_loop(
                            host_sock,
                            is_bridge_net,
                            state_root_buf,
                            cid_str,
                            container_port,
                        );
                    })?
                }
            };
            pids.push(pid);
        }
        Ok(Self { pids })
    }
}

fn run_tcp_proxy_loop(
    listener: TcpListener,
    is_bridge_net: bool,
    state_root_buf: PathBuf,
    cid_str: String,
    container_port: u16,
) {
    for incoming in listener.incoming().flatten() {
        let sr = state_root_buf.clone();
        let cid = cid_str.clone();
        std::thread::spawn(move || {
            let mut outbound_opt = None;
            for _ in 0..50 {
                let target_pid = wait_for_bridge_pid(is_bridge_net, &sr, &cid);
                if is_bridge_net && target_pid.is_none() {
                    break;
                }
                match coral_net::connect_container_port(target_pid, container_port) {
                    Ok(s) => {
                        outbound_opt = Some(s);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
            if let Some(outbound) = outbound_opt {
                let mut in_r = incoming;
                let mut out_w = outbound;
                if let (Ok(mut in_w), Ok(mut out_r)) = (in_r.try_clone(), out_w.try_clone()) {
                    let h = std::thread::spawn(move || {
                        let _ = io::copy(&mut out_r, &mut in_w);
                        let _ = in_w.shutdown(Shutdown::Write);
                    });
                    let _ = io::copy(&mut in_r, &mut out_w);
                    let _ = out_w.shutdown(Shutdown::Write);
                    let _ = h.join();
                }
            }
        });
    }
}

fn run_udp_proxy_loop(
    host_sock: UdpSocket,
    is_bridge_net: bool,
    state_root_buf: PathBuf,
    cid_str: String,
    container_port: u16,
) {
    let host_sock = Arc::new(host_sock);
    let peers: Arc<Mutex<HashMap<SocketAddr, Arc<UdpSocket>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut buf = vec![0u8; 65_536];

    while let Ok((n, peer_addr)) = host_sock.recv_from(&mut buf) {
        let existing = peers.lock().ok().and_then(|g| g.get(&peer_addr).cloned());

        let ctr_sock = if let Some(s) = existing {
            s
        } else {
            let target_pid = wait_for_bridge_pid(is_bridge_net, &state_root_buf, &cid_str);
            if is_bridge_net && target_pid.is_none() {
                continue;
            }
            let Ok(udp) = coral_net::connect_container_udp(target_pid, container_port) else {
                continue;
            };
            let _ = udp.set_read_timeout(Some(std::time::Duration::from_secs(60)));
            let ctr_arc = Arc::new(udp);
            if let Ok(mut guard) = peers.lock() {
                guard.insert(peer_addr, Arc::clone(&ctr_arc));
            }

            let ctr_rx = Arc::clone(&ctr_arc);
            let host_rx = Arc::clone(&host_sock);
            let peers_rx = Arc::clone(&peers);
            std::thread::spawn(move || {
                let mut rbuf = vec![0u8; 65_536];
                while let Ok(rn) = ctr_rx.recv(&mut rbuf) {
                    if host_rx.send_to(&rbuf[..rn], peer_addr).is_err() {
                        break;
                    }
                }
                if let Ok(mut guard) = peers_rx.lock()
                    && let Some(cur) = guard.get(&peer_addr)
                    && Arc::ptr_eq(cur, &ctr_rx)
                {
                    guard.remove(&peer_addr);
                }
            });

            ctr_arc
        };

        if ctr_sock.send(&buf[..n]).is_err()
            && let Ok(mut guard) = peers.lock()
        {
            guard.remove(&peer_addr);
        }
    }
}

impl Drop for ForkedPortProxies {
    fn drop(&mut self) {
        for &pid in &self.pids {
            unsafe {
                let _ = libc::kill(pid, libc::SIGTERM);
                let mut status = 0;
                let _ = libc::waitpid(pid, &mut status, 0);
            }
        }
    }
}
