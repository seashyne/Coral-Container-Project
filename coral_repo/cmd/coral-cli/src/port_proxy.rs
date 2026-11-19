//! Forks a dedicated background TCP forwarder process (keeping the parent single-threaded
//! for POSIX-safe `fork`/`unshare`) and stops it automatically when dropped.

use anyhow::{Context, Result};
use coral_net::PortMapping;
use coral_runtime::StateStore;
use std::io;
use std::net::{Shutdown, TcpListener};
use std::path::Path;

pub struct ForkedPortProxies {
    pids: Vec<i32>,
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
            let listener = TcpListener::bind(&listen_addr)
                .with_context(|| format!("bind TCP port forwarder on {listen_addr}"))?;
            let container_port = pm.container_port;
            let state_root_buf = state_root.to_path_buf();
            let cid_str = container_id.to_string();

            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return Err(anyhow::anyhow!(
                    "fork port forwarder failed: {}",
                    io::Error::last_os_error()
                ));
            }
            if pid == 0 {
                // Child proxy process: accept connections and relay bidirectionally
                // (entering the container's CLONE_NEWNET when `is_bridge_net == true`).
                for incoming in listener.incoming().flatten() {
                    let sr = state_root_buf.clone();
                    let cid = cid_str.clone();
                    std::thread::spawn(move || {
                        let store = StateStore::new(&sr);
                        let mut outbound_opt = None;
                        for _ in 0..50 {
                            let target_pid = if is_bridge_net {
                                let Some(p) = store.load(&cid).ok().and_then(|st| st.pid) else {
                                    std::thread::sleep(std::time::Duration::from_millis(20));
                                    continue;
                                };
                                Some(p)
                            } else {
                                None
                            };
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
                            if let (Ok(mut in_w), Ok(mut out_r)) =
                                (in_r.try_clone(), out_w.try_clone())
                            {
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
                unsafe { libc::_exit(0) };
            }
            drop(listener);
            pids.push(pid);
        }
        Ok(Self { pids })
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
