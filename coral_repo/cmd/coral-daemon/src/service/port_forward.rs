//! Asynchronous Tokio TCP port forwarders for `HOST_PORT:CONTAINER_PORT` mappings
//! across both `--network host` and `--network bridge` (`CLONE_NEWNET`).

use coral_net::PortMapping;
use std::path::Path;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tonic::Status;
use tracing::info;

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
        let listener = TcpListener::bind(&listen_addr).await.map_err(|e| {
            Status::failed_precondition(format!("bind port forwarder {listen_addr}: {e}"))
        })?;
        let container_port = pm.container_port;
        let rt_root = runtime_root.to_path_buf();
        let cid_for_port = container_id.to_string();
        info!(
            container_id = %container_id,
            from = %listen_addr,
            container_port,
            bridge = is_bridge_net,
            "forwarding container TCP port"
        );
        handles.push(tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                let rt_root_inner = rt_root.clone();
                let cid_inner = cid_for_port.clone();
                tokio::spawn(async move {
                    let mut outbound_opt = None;
                    for _ in 0..50 {
                        let target_pid = if is_bridge_net {
                            let pid_opt = coral_runtime::StateStore::new(&rt_root_inner)
                                .load(&cid_inner)
                                .ok()
                                .and_then(|st| st.pid);
                            let Some(p) = pid_opt else {
                                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                                continue;
                            };
                            Some(p)
                        } else {
                            None
                        };

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
        }));
    }
    Ok(handles)
}
