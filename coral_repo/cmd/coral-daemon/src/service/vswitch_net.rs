//! Daemon-side Rootless L2 Virtual Switch (`RootlessVSwitch`) networking,
//! private IPv4 subnet allocation (`10.89.x.0/24`), bidirectional `/etc/hosts`,
//! and embedded DNS registration for custom Coral networks.

#![allow(clippy::result_large_err)]

use super::{CoralDaemonService, DaemonStore, SandboxRecord};
use coral_api::ContainerConfig;
use coral_net::RootlessVSwitch;
use coral_runtime::store::{StateStore, is_process_alive};
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;
use tonic::Status;
use tracing::{debug, warn};

impl CoralDaemonService {
    /// Configures custom-network membership, private `10.89.x.Y` IP allocation,
    /// bidirectional sibling `/etc/hosts` entries, and embedded DNS records for a container.
    pub(crate) fn configure_container_network(
        &self,
        store: &mut DaemonStore,
        sandbox: &SandboxRecord,
        config: &mut ContainerConfig,
    ) -> Result<(), Status> {
        let net_name = config
            .annotations
            .get("coral.network")
            .cloned()
            .unwrap_or_else(|| "host".into());
        let mut allocated_ip: Option<Ipv4Addr> = None;

        if !net_name.eq_ignore_ascii_case("host")
            && !net_name.eq_ignore_ascii_case("none")
            && !net_name.eq_ignore_ascii_case("bridge")
            && !net_name.eq_ignore_ascii_case("isolated")
        {
            let net_rec = coral_net::find_network(&net_name).ok_or_else(|| {
                Status::not_found(format!(
                    "network '{net_name}' not found — create it first with `coral network create {net_name}`"
                ))
            })?;
            config
                .labels
                .insert("coral.network.name".into(), net_name.clone());

            let ip = ensure_vswitch_and_allocate_ip(
                store,
                &net_name,
                &net_rec.subnet,
                self.dns_server.clone(),
            );
            allocated_ip = Some(ip);
            let ip_str = ip.to_string();
            config
                .labels
                .insert("coral.network.ip".into(), ip_str.clone());
            if let Some(sb) = store.sandboxes.get_mut(&sandbox.id)
                && sb.pod_ip.is_none()
            {
                sb.pod_ip = Some(ip_str.clone());
            }

            let own_names = collect_service_names(config);
            let mut sibling_hosts: Vec<String> = config
                .annotations
                .get("coral.extra_hosts")
                .map(|raw| {
                    raw.split(['\n', ','])
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(ToOwned::to_owned)
                        .collect()
                })
                .unwrap_or_default();

            for name in &own_names {
                let self_entry = format!("{name}:{ip_str}");
                if !sibling_hosts.contains(&self_entry) {
                    sibling_hosts.push(self_entry);
                }
            }

            for existing in store.containers.values() {
                if existing
                    .config
                    .labels
                    .get("coral.network.name")
                    .map(String::as_str)
                    == Some(net_name.as_str())
                {
                    let peer_ip = existing
                        .config
                        .labels
                        .get("coral.network.ip")
                        .map(String::as_str)
                        .unwrap_or("127.0.0.1");
                    for peer_name in collect_service_names(&existing.config) {
                        let entry = format!("{peer_name}:{peer_ip}");
                        if !sibling_hosts.contains(&entry) {
                            sibling_hosts.push(entry);
                        }
                    }
                    for name in &own_names {
                        let _ = coral_runtime::rootfs::append_host_entry_in_place(
                            &existing.bundle_dir,
                            name,
                            &ip_str,
                        );
                    }
                }
            }

            if !sibling_hosts.is_empty() {
                config
                    .annotations
                    .insert("coral.extra_hosts".into(), sibling_hosts.join("\n"));
            }
        }

        // Register container name, compose service name, and network aliases in the embedded DNS server.
        if let Some(dns) = &self.dns_server {
            let target_ip = allocated_ip
                .or_else(|| {
                    sandbox
                        .pod_ip
                        .as_deref()
                        .and_then(|s| s.parse::<Ipv4Addr>().ok())
                })
                .unwrap_or(Ipv4Addr::LOCALHOST);
            for name in collect_service_names(config) {
                dns.add_host_ip(&name, target_ip);
            }
        }

        Ok(())
    }

    /// Spawns a background task that waits briefly for the started container's runtime PID
    /// and plugs its isolated netns into the custom network's `RootlessVSwitch`.
    pub(crate) fn spawn_vswitch_attach(&self, cid: String, net_name: String, ip_str: String) {
        let Ok(ip) = ip_str.parse::<Ipv4Addr>() else {
            return;
        };
        let svc = self.clone();
        tokio::spawn(async move {
            let vswitch = {
                let mut store = svc.store.write().await;
                if let Some(vs) = store.vswitches.get(&net_name) {
                    Some(vs.clone())
                } else if let Some(rec) = coral_net::find_network(&net_name) {
                    RootlessVSwitch::start_with_dns(&rec.subnet, svc.dns_server.clone())
                        .ok()
                        .map(|vs| {
                            let arc = Arc::new(vs);
                            store.vswitches.insert(net_name.clone(), arc.clone());
                            arc
                        })
                } else {
                    None
                }
            };
            let Some(vswitch) = vswitch else {
                return;
            };
            let runtime_root = svc.runtime_root();
            for _ in 0..50 {
                if let Ok(st) = StateStore::new(&runtime_root).load(&cid)
                    && let Some(pid) = st.pid
                    && is_process_alive(pid)
                {
                    let cid_c = cid.clone();
                    let net_c = net_name.clone();
                    let res = tokio::task::spawn_blocking(move || {
                        vswitch.attach_container(&cid_c, pid, ip)
                    })
                    .await;
                    match res {
                        Ok(Ok(())) => {
                            debug!(
                                container_id = %cid,
                                network = %net_c,
                                ip = %ip,
                                pid,
                                "attached container to rootless L2 vswitch"
                            );
                        }
                        Ok(Err(e)) => {
                            warn!(
                                container_id = %cid,
                                network = %net_c,
                                err = %e,
                                "rootless L2 vswitch attach skipped (falling back to loopback/hosts)"
                            );
                        }
                        Err(_) => {}
                    }
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        });
    }

    /// Detaches a container's TAP port from its custom network's `RootlessVSwitch`, if attached.
    pub(crate) fn detach_vswitch_port(store: &DaemonStore, cid: &str, net_name: Option<&str>) {
        if let Some(name) = net_name {
            if let Some(vs) = store.vswitches.get(name) {
                vs.detach_container(cid);
            }
        } else {
            for vs in store.vswitches.values() {
                vs.detach_container(cid);
            }
        }
    }
}

fn ensure_vswitch_and_allocate_ip(
    store: &mut DaemonStore,
    net_name: &str,
    subnet: &str,
    dns: Option<Arc<coral_net::EmbeddedDnsServer>>,
) -> Ipv4Addr {
    if let Some(vs) = store.vswitches.get(net_name) {
        return vs.allocate_ip();
    }
    let effective_subnet = if subnet.trim().is_empty() {
        "10.89.1.0/24"
    } else {
        subnet
    };
    match RootlessVSwitch::start_with_dns(effective_subnet, dns) {
        Ok(vs) => {
            let ip = vs.allocate_ip();
            store.vswitches.insert(net_name.to_string(), Arc::new(vs));
            ip
        }
        Err(e) => {
            warn!(network = %net_name, subnet = %effective_subnet, err = %e, "failed to start rootless vswitch");
            Ipv4Addr::LOCALHOST
        }
    }
}

fn collect_service_names(config: &ContainerConfig) -> Vec<String> {
    let mut names = Vec::new();
    let mut push_unique = |s: &str| {
        let trimmed = s.trim();
        if !trimmed.is_empty() && !names.iter().any(|n: &String| n == trimmed) {
            names.push(trimmed.to_string());
        }
    };
    if let Some(meta) = &config.metadata {
        push_unique(&meta.name);
    }
    if let Some(svc_name) = config.labels.get("coral.compose.service") {
        push_unique(svc_name);
    }
    if let Some(raw_aliases) = config.annotations.get("coral.network.aliases") {
        for alias in raw_aliases.split([',', '\n', ' ']) {
            push_unique(alias);
        }
    }
    names
}
