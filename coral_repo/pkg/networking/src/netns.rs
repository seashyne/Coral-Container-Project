//! Linux network namespace (`CLONE_NEWNET`) loopback bring-up, port mapping parser,
//! and rootless cross-NetNS TCP socket connector via `fork()` + `setns()` + `SCM_RIGHTS`.

use crate::NetError;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io;
use std::net::TcpStream;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;

/// Brings the `lo` loopback interface UP inside the caller's current Linux network namespace.
///
/// Must be called inside a newly unshared `CLONE_NEWNET` namespace where the process has `CAP_NET_ADMIN`.
pub fn bring_loopback_up() -> Result<(), NetError> {
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(NetError::Io(io::Error::last_os_error()));
    }

    let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
    let name = b"lo\0";
    for (dst, src) in ifr.ifr_name.iter_mut().zip(name.iter()) {
        *dst = *src as libc::c_char;
    }

    let res = unsafe {
        if libc::ioctl(fd, libc::SIOCGIFFLAGS, &mut ifr) < 0 {
            Err(io::Error::last_os_error())
        } else {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            if libc::ioctl(fd, libc::SIOCSIFFLAGS, &ifr) < 0 {
                Err(io::Error::last_os_error())
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

/// TCP port publish specification (`HOST_PORT:CONTAINER_PORT` or `BIND_IP:HOST_PORT:CONTAINER_PORT`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortMapping {
    pub bind_ip: String,
    pub host_port: u16,
    pub container_port: u16,
}

impl PortMapping {
    pub fn parse(raw: &str) -> Result<Self, NetError> {
        let trimmed = raw.trim();
        let parts: Vec<&str> = trimmed.split(':').collect();
        let (bind_ip, host_str, ctr_str) = match parts.as_slice() {
            [h, c] => ("0.0.0.0", *h, *c),
            [ip, h, c] => (*ip, *h, *c),
            _ => {
                return Err(NetError::Validation(format!(
                    "invalid port mapping '{raw}' (expected HOST_PORT:CONTAINER_PORT or IP:HOST_PORT:CONTAINER_PORT)"
                )));
            }
        };
        let host_port: u16 = host_str.parse().map_err(|_| {
            NetError::Validation(format!("invalid host port '{host_str}' in '{raw}'"))
        })?;
        let container_port: u16 = ctr_str.parse().map_err(|_| {
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
        })
    }
}

/// Connects a TCP stream to `127.0.0.1:container_port`, entering `target_pid`'s Linux
/// network namespace (`CLONE_NEWNET`) inside a short-lived single-threaded helper child
/// process and passing the connected socket back via `SCM_RIGHTS`.
///
/// This works in both rootful and rootless (`CLONE_NEWUSER`) mode and is safe to call
/// from multi-threaded runtimes (such as Tokio in `coral-daemon`) because `setns(CLONE_NEWUSER)`
/// runs after `fork()` where `mm_users == 1` using only async-signal-safe syscalls.
pub fn connect_container_port(target_pid: Option<i32>, container_port: u16) -> io::Result<TcpStream> {
    let Some(pid) = target_pid.filter(|&p| p > 0) else {
        return TcpStream::connect(("127.0.0.1", container_port));
    };

    let target_net = format!("/proc/{pid}/ns/net");
    let self_net = "/proc/self/ns/net";
    let (Ok(t_meta), Ok(s_meta)) = (fs::metadata(&target_net), fs::metadata(self_net)) else {
        return TcpStream::connect(("127.0.0.1", container_port));
    };
    if t_meta.dev() == s_meta.dev() && t_meta.ino() == s_meta.ino() {
        return TcpStream::connect(("127.0.0.1", container_port));
    }

    let target_user = format!("/proc/{pid}/ns/user");
    let need_user_ns = match (fs::metadata(&target_user), fs::metadata("/proc/self/ns/user")) {
        (Ok(tu), Ok(su)) => tu.dev() != su.dev() || tu.ino() != su.ino(),
        _ => false,
    };

    let user_file = if need_user_ns {
        Some(File::open(&target_user)?)
    } else {
        None
    };
    let net_file = File::open(&target_net)?;
    let user_fd = user_file.as_ref().map(|f| f.as_raw_fd()).unwrap_or(-1);
    let net_fd = net_file.as_raw_fd();

    let mut sv = [-1i32; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
            0,
            sv.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }

    let child_pid = unsafe { libc::fork() };
    if child_pid < 0 {
        let err = io::Error::last_os_error();
        unsafe {
            libc::close(sv[0]);
            libc::close(sv[1]);
        }
        return Err(err);
    }

    if child_pid == 0 {
        // Child process: strictly async-signal-safe syscalls only.
        unsafe {
            libc::close(sv[0]);

            let mut err_no: i32 = 0;
            let mut tcp_fd: i32 = -1;

            if user_fd >= 0 && libc::setns(user_fd, libc::CLONE_NEWUSER) != 0 {
                err_no = *libc::__errno_location();
            }
            if err_no == 0 && libc::setns(net_fd, libc::CLONE_NEWNET) != 0 {
                err_no = *libc::__errno_location();
            }
            if err_no == 0 {
                tcp_fd = libc::socket(
                    libc::AF_INET,
                    libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                    0,
                );
                if tcp_fd < 0 {
                    err_no = *libc::__errno_location();
                } else {
                    let sin = libc::sockaddr_in {
                        sin_family: libc::AF_INET as libc::sa_family_t,
                        sin_port: container_port.to_be(),
                        sin_addr: libc::in_addr {
                            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
                        },
                        sin_zero: [0; 8],
                    };
                    if libc::connect(
                        tcp_fd,
                        (&raw const sin).cast::<libc::sockaddr>(),
                        std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
                    ) != 0
                    {
                        err_no = *libc::__errno_location();
                        libc::close(tcp_fd);
                        tcp_fd = -1;
                    }
                }
            }

            let mut err_bytes = err_no.to_ne_bytes();
            let mut iov = libc::iovec {
                iov_base: err_bytes.as_mut_ptr().cast::<libc::c_void>(),
                iov_len: err_bytes.len(),
            };

            #[repr(C)]
            struct CmsgFd {
                hdr: libc::cmsghdr,
                fd: i32,
            }

            let mut cmsg_buf = std::mem::MaybeUninit::<CmsgFd>::zeroed();
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &raw mut iov;
            msg.msg_iovlen = 1;

            if tcp_fd >= 0 {
                let cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
                let cmsg_space = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize;
                msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
                msg.msg_controllen = cmsg_space;
                let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
                if !cmsg.is_null() {
                    (*cmsg).cmsg_level = libc::SOL_SOCKET;
                    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                    (*cmsg).cmsg_len = cmsg_len;
                    std::ptr::write_unaligned(
                        libc::CMSG_DATA(cmsg).cast::<i32>(),
                        tcp_fd,
                    );
                }
            }

            let _ = libc::sendmsg(sv[1], &raw const msg, libc::MSG_NOSIGNAL);
            if tcp_fd >= 0 {
                libc::close(tcp_fd);
            }
            libc::close(sv[1]);
            libc::_exit(0);
        }
    }

    // Parent process: receive either the connected file descriptor or the errno.
    unsafe {
        libc::close(sv[1]);
    }
    drop(user_file);
    drop(net_file);

    let mut err_bytes = [0u8; 4];
    let mut iov = libc::iovec {
        iov_base: err_bytes.as_mut_ptr().cast::<libc::c_void>(),
        iov_len: err_bytes.len(),
    };
    let mut cmsg_storage = [0u64; 4];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cmsg_storage.as_mut_ptr().cast::<libc::c_void>();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize };

    let n = unsafe { libc::recvmsg(sv[0], &raw mut msg, libc::MSG_CMSG_CLOEXEC) };
    let mut status = 0;
    unsafe {
        libc::close(sv[0]);
        let _ = libc::waitpid(child_pid, &raw mut status, 0);
    }

    if n < 4 {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "failed to receive socket from netns connector child",
        ));
    }
    let err_no = i32::from_ne_bytes(err_bytes);
    if err_no != 0 {
        return Err(io::Error::from_raw_os_error(err_no));
    }

    let mut received_fd: i32 = -1;
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                received_fd = std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>());
                break;
            }
            cmsg = libc::CMSG_NXTHDR(&raw const msg, cmsg);
        }
    }

    if received_fd < 0 {
        return Err(io::Error::other(
            "netns connector did not return a valid file descriptor",
        ));
    }

    Ok(unsafe { TcpStream::from_raw_fd(received_fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_port_mappings() {
        assert_eq!(
            PortMapping::parse("8080:80").unwrap(),
            PortMapping {
                bind_ip: "0.0.0.0".into(),
                host_port: 8080,
                container_port: 80,
            }
        );
        assert_eq!(
            PortMapping::parse("127.0.0.1:9000:3000").unwrap(),
            PortMapping {
                bind_ip: "127.0.0.1".into(),
                host_port: 9000,
                container_port: 3000,
            }
        );
        assert!(PortMapping::parse("bad").is_err());
        assert!(PortMapping::parse("0:80").is_err());
    }

    #[test]
    fn connect_container_port_bridges_into_isolated_user_and_net_namespace() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let mut ready_pipe = [-1i32; 2];
        assert_eq!(unsafe { libc::pipe(ready_pipe.as_mut_ptr()) }, 0);

        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::close(ready_pipe[0]);
                if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                    libc::_exit(1);
                }
            }
            if bring_loopback_up().is_err() {
                unsafe { libc::_exit(2) };
            }
            let Ok(listener) = TcpListener::bind("127.0.0.1:8080") else {
                unsafe { libc::_exit(3) };
            };
            unsafe {
                let _ = libc::write(ready_pipe[1], b"R".as_ptr().cast(), 1);
                libc::close(ready_pipe[1]);
            }
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4];
                let _ = stream.read_exact(&mut buf);
                if &buf == b"PING" {
                    let _ = stream.write_all(b"PONG-FROM-NETNS");
                }
            }
            unsafe { libc::_exit(0) };
        }

        unsafe {
            libc::close(ready_pipe[1]);
        }
        let mut sig = [0u8; 1];
        let n = unsafe { libc::read(ready_pipe[0], sig.as_mut_ptr().cast(), 1) };
        unsafe {
            libc::close(ready_pipe[0]);
        }
        assert_eq!(n, 1);
        assert_eq!(sig[0], b'R');

        let mut stream = connect_container_port(Some(pid), 8080).expect("connect into netns");
        stream.write_all(b"PING").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert_eq!(reply, "PONG-FROM-NETNS");

        let mut status = 0;
        unsafe {
            let _ = libc::waitpid(pid, &raw mut status, 0);
        }
    }
}
