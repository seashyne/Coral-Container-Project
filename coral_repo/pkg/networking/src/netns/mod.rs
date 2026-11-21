//! Linux network namespace (`CLONE_NEWNET`) loopback bring-up, TCP/UDP port mapping parser,
//! and rootless cross-NetNS TCP/UDP socket connector via `fork()` + `setns()` + `SCM_RIGHTS`.

mod connector;
#[cfg(test)]
mod tests;

use crate::NetError;
use serde::{Deserialize, Serialize};

pub use connector::{connect_container_port, connect_container_udp};

/// Brings the `lo` loopback interface UP inside the caller's current Linux network namespace.
///
/// Must be called inside a newly unshared `CLONE_NEWNET` namespace where the process has `CAP_NET_ADMIN`.
pub fn bring_loopback_up() -> Result<(), NetError> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(NetError::Io(std::io::Error::last_os_error()));
    }

    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    let name = b"lo\0";
    for (dst, src) in ifr.ifr_name.iter_mut().zip(name.iter()) {
        *dst = *src as libc::c_char;
    }

    let res = unsafe {
        if libc::ioctl(fd, libc::SIOCGIFFLAGS, &mut ifr) < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            if libc::ioctl(fd, libc::SIOCSIFFLAGS, &ifr) < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    };

    unsafe {
        libc::close(fd);
    }

    res.map_err(NetError::Io)
}

/// Transport protocol for published container ports.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PortProtocol {
    #[default]
    Tcp,
    Udp,
}

impl std::fmt::Display for PortProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp => f.write_str("tcp"),
            Self::Udp => f.write_str("udp"),
        }
    }
}

/// Port publish specification (`HOST_PORT:CONTAINER_PORT[/tcp|/udp]`,
/// `BIND_IP:HOST_PORT:CONTAINER_PORT[/tcp|/udp]`, or `PORT[/tcp|/udp]`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortMapping {
    pub bind_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    #[serde(default)]
    pub protocol: PortProtocol,
}

impl PortMapping {
    pub fn parse(raw: &str) -> Result<Self, NetError> {
        let trimmed = raw.trim();
        let (addr_part, protocol) = match trimmed.rsplit_once('/') {
            Some((base, proto_raw)) => {
                let proto = match proto_raw.trim().to_ascii_lowercase().as_str() {
                    "tcp" => PortProtocol::Tcp,
                    "udp" => PortProtocol::Udp,
                    other => {
                        return Err(NetError::Validation(format!(
                            "unsupported port protocol '{other}' in '{raw}' (expected 'tcp' or 'udp')"
                        )));
                    }
                };
                (base.trim(), proto)
            }
            None => (trimmed, PortProtocol::Tcp),
        };

        let parts: Vec<&str> = addr_part.split(':').collect();
        let (bind_ip, host_str, ctr_str) = match parts.as_slice() {
            [p] => ("0.0.0.0", *p, *p),
            [h, c] => ("0.0.0.0", *h, *c),
            [ip, h, c] => (if ip.trim().is_empty() { "0.0.0.0" } else { ip.trim() }, *h, *c),
            _ => {
                return Err(NetError::Validation(format!(
                    "invalid port mapping '{raw}' (expected HOST_PORT:CONTAINER_PORT[/tcp|/udp] or IP:HOST_PORT:CONTAINER_PORT[/tcp|/udp])"
                )));
            }
        };
        let host_port: u16 = host_str.trim().parse().map_err(|_| {
            NetError::Validation(format!("invalid host port '{host_str}' in '{raw}'"))
        })?;
        let container_port: u16 = ctr_str.trim().parse().map_err(|_| {
            NetError::Validation(format!("invalid container port '{ctr_str}' in '{raw}'"))
        })?;
        if host_port == 0 || container_port == 0 {
            return Err(NetError::Validation(format!(
                "port numbers must be non-zero in '{raw}'"
            )));
        }
        Ok(Self {
            bind_ip: bind_ip.to_string(),
            host_port,
            container_port,
            protocol,
        })
    }
}
