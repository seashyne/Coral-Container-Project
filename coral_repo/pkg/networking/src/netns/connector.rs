//! Rootless cross-NetNS TCP/UDP socket connector via `fork()` + `setns()` + `SCM_RIGHTS`.

use std::fs::{self, File};
use std::io;
use std::net::{TcpStream, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

/// Connects a TCP stream to `127.0.0.1:container_port`, entering `target_pid`'s Linux
/// network namespace (`CLONE_NEWNET`) when running in an isolated bridge namespace.
pub fn connect_container_port(target_pid: Option<i32>, container_port: u16) -> io::Result<TcpStream> {
    let owned = open_container_netns_socket(target_pid, libc::SOCK_STREAM, container_port)?;
    Ok(TcpStream::from(owned))
}

/// Opens a connected UDP socket targeting `127.0.0.1:container_port`, entering `target_pid`'s
/// Linux network namespace (`CLONE_NEWNET`) inside a short-lived helper child when isolated.
pub fn connect_container_udp(target_pid: Option<i32>, container_port: u16) -> io::Result<UdpSocket> {
    let owned = open_container_netns_socket(target_pid, libc::SOCK_DGRAM, container_port)?;
    Ok(UdpSocket::from(owned))
}

/// Connects a `SOCK_STREAM` or `SOCK_DGRAM` IPv4 socket to `127.0.0.1:container_port`,
/// entering `target_pid`'s Linux user + net namespace if needed and passing the FD back via
/// `SCM_RIGHTS` (async-signal-safe after `fork()`).
fn open_container_netns_socket(
    target_pid: Option<i32>,
    sock_type: libc::c_int,
    container_port: u16,
) -> io::Result<OwnedFd> {
    let connect_direct = || -> io::Result<OwnedFd> {
        if sock_type == libc::SOCK_DGRAM {
            let udp = UdpSocket::bind(("127.0.0.1", 0))?;
            udp.connect(("127.0.0.1", container_port))?;
            Ok(OwnedFd::from(udp))
        } else {
            let tcp = TcpStream::connect(("127.0.0.1", container_port))?;
            Ok(OwnedFd::from(tcp))
        }
    };

    let Some(pid) = target_pid.filter(|&p| p > 0) else {
        return connect_direct();
    };

    let target_net = format!("/proc/{pid}/ns/net");
    let self_net = "/proc/self/ns/net";
    let (Ok(t_meta), Ok(s_meta)) = (fs::metadata(&target_net), fs::metadata(self_net)) else {
        return connect_direct();
    };
    if t_meta.dev() == s_meta.dev() && t_meta.ino() == s_meta.ino() {
        return connect_direct();
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
                    sock_type | libc::SOCK_CLOEXEC,
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

    Ok(unsafe { OwnedFd::from_raw_fd(received_fd) })
}
