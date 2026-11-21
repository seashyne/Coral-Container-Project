//! Asynchronous Tokio TCP & UDP port forwarders for `HOST_PORT:CONTAINER_PORT[/tcp|/udp]`
//! mappings across both `--network host` and `--network bridge` (`CLONE_NEWNET`).

use coral_net::{PortMapping, PortProtocol};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tonic::Status;
use tracing::info;

type UdpPeerMap = Arc<Mutex<HashMap<SocketAddr, (Arc<UdpSocket>, JoinHandle<()>)>>>;

struct AbortPeersOnDrop(UdpPeerMap);

impl Drop for AbortPeersOnDrop {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.0.try_lock() {
            for (_, (_, handle)) in guard.drain() {
                handle.abort();
            }
        }
    }
}

async fn resolve_bridge_pid(is_bridge_net: bool, runtime_root: &Path, cid: &str) -> Option<i32> {
    if !is_bridge_net {
        return None;
    }
    for _ in 0..50 {
        if let Some(pid) = coral_runtime::StateStore::new(runtime_root)
            .load(cid)
            .ok()
            .and_then(|st| st.pid)
        {
            return Some(pid);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    None
}

#[allow(clippy::result_large_err)]
pub(crate) async fn spawn_port_forwarders(
    container_id: &str,
    raw_ports: &str,
    is_bridge_net: bool,
    runtime_root: &Path,
) -> Result<Vec<JoinHandle<()>>, Status> {
    let mut handles = Vec::new();
    for item in raw_ports.split(['\n', ',']) {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        let pm = PortMapping::parse(trimmed)
            .map_err(|e| Status::invalid_argument(format!("invalid port mapping: {e}")))?;
        if !is_bridge_net && pm.host_port == pm.container_port && pm.bind_ip == "0.0.0.0" {
            continue;
        }
        let listen_addr = format!("{}:{}", pm.bind_ip, pm.host_port);
        let rt_root = runtime_root.to_path_buf();
        let cid_for_port = container_id.to_string();

        match pm.protocol {
            PortProtocol::Tcp => {
                let listener = TcpListener::bind(&listen_addr).await.map_err(|e| {
                    Status::failed_precondition(format!("bind port forwarder {listen_addr}: {e}"))
                })?;
                info!(
                    container_id = %container_id,
                    from = %listen_addr,
                    container_port = pm.container_port,
                    proto = "tcp",
                    bridge = is_bridge_net,
                    "forwarding container port"
                );
                handles.push(spawn_tcp_forwarder(
                    listener,
                    pm.container_port,
                    is_bridge_net,
                    rt_root,
                    cid_for_port,
                ));
            }
            PortProtocol::Udp => {
                let host_sock = UdpSocket::bind(&listen_addr).await.map_err(|e| {
                    Status::failed_precondition(format!(
                        "bind UDP port forwarder {listen_addr}: {e}"
                    ))
                })?;
                info!(
                    container_id = %container_id,
                    from = %listen_addr,
                    container_port = pm.container_port,
                    proto = "udp",
                    bridge = is_bridge_net,
                    "forwarding container port"
                );
                handles.push(spawn_udp_forwarder(
                    Arc::new(host_sock),
                    pm.container_port,
                    is_bridge_net,
                    rt_root,
                    cid_for_port,
                ));
            }
        }
    }
    Ok(handles)
}

fn spawn_tcp_forwarder(
    listener: TcpListener,
    container_port: u16,
    is_bridge_net: bool,
    rt_root: PathBuf,
    cid_for_port: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok((mut inbound, _)) = listener.accept().await {
            let rt_root_inner = rt_root.clone();
            let cid_inner = cid_for_port.clone();
            tokio::spawn(async move {
                let mut outbound_opt = None;
                for _ in 0..50 {
                    let target_pid =
                        resolve_bridge_pid(is_bridge_net, &rt_root_inner, &cid_inner).await;
                    if is_bridge_net && target_pid.is_none() {
                        break;
                    }

                    let connect_res = tokio::task::spawn_blocking(move || {
                        coral_net::connect_container_port(target_pid, container_port)
                    })
                    .await;

                    match connect_res {
                        Ok(Ok(std_stream)) => {
                            if std_stream.set_nonblocking(true).is_ok()
                                && let Ok(tk_stream) = TcpStream::from_std(std_stream)
                            {
                                outbound_opt = Some(tk_stream);
                            }
                            break;
                        }
                        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        }
                        _ => break,
                    }
                }
                if let Some(mut outbound) = outbound_opt {
                    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                }
            });
        }
    })
}

fn spawn_udp_forwarder(
    host_sock: Arc<UdpSocket>,
    container_port: u16,
    is_bridge_net: bool,
    rt_root: PathBuf,
    cid_for_port: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let peers: UdpPeerMap = Arc::new(Mutex::new(HashMap::new()));
        let _drop_guard = AbortPeersOnDrop(Arc::clone(&peers));
        let mut buf = vec![0u8; 65_536];

        while let Ok((n, peer_addr)) = host_sock.recv_from(&mut buf).await {
            let existing = {
                let guard = peers.lock().await;
                guard.get(&peer_addr).map(|(s, _)| Arc::clone(s))
            };

            let ctr_sock = if let Some(s) = existing {
                s
            } else {
                let target_pid = resolve_bridge_pid(is_bridge_net, &rt_root, &cid_for_port).await;
                if is_bridge_net && target_pid.is_none() {
                    continue;
                }
                let Ok(Ok(std_udp)) = tokio::task::spawn_blocking(move || {
                    coral_net::connect_container_udp(target_pid, container_port)
                })
                .await
                else {
                    continue;
                };
                if std_udp.set_nonblocking(true).is_err() {
                    continue;
                }
                let Ok(tk_udp) = UdpSocket::from_std(std_udp) else {
                    continue;
                };
                let ctr_arc = Arc::new(tk_udp);

                let ctr_for_rx = Arc::clone(&ctr_arc);
                let host_for_rx = Arc::clone(&host_sock);
                let peers_for_rx = Arc::clone(&peers);
                let rx_task = tokio::spawn(async move {
                    let mut rbuf = vec![0u8; 65_536];
                    while let Ok(Ok(rn)) = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        ctr_for_rx.recv(&mut rbuf),
                    )
                    .await
                    {
                        if host_for_rx.send_to(&rbuf[..rn], peer_addr).await.is_err() {
                            break;
                        }
                    }
                    let mut guard = peers_for_rx.lock().await;
                    if let Some((cur, _)) = guard.get(&peer_addr)
                        && Arc::ptr_eq(cur, &ctr_for_rx)
                    {
                        guard.remove(&peer_addr);
                    }
                });

                let mut guard = peers.lock().await;
                if let Some((_prev, prev_handle)) =
                    guard.insert(peer_addr, (Arc::clone(&ctr_arc), rx_task))
                {
                    prev_handle.abort();
                }
                ctr_arc
            };

            if ctr_sock.send(&buf[..n]).await.is_err() {
                let mut guard = peers.lock().await;
                if let Some((_, handle)) = guard.remove(&peer_addr) {
                    handle.abort();
                }
            }
        }
    })
}
