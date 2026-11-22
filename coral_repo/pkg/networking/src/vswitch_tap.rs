//! Rootless cross-NetNS TAP interface creation (`fork()` + `setns()` + `TUNSETIFF` +
//! `SIOCSIFADDR`/`SIOCSIFNETMASK`/`SIOCSIFFLAGS` + `SCM_RIGHTS`).

use std::fs::{self, File};
use std::io;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

const TUNSETIFF: libc::Ioctl = 0x4004_54ca_u32 as libc::Ioctl;
const IFF_TAP: libc::c_short = 0x0002;
const IFF_NO_PI: libc::c_short = 0x1000;

/// Creates and configures a TAP device (`if_name`, e.g. `"coral0"`) inside `target_pid`'s
/// `CLONE_NEWUSER | CLONE_NEWNET` namespace with the given IPv4 address and netmask, brings it
/// UP, and passes the TAP file descriptor back to the caller via `SCM_RIGHTS`.
pub fn open_tap_in_netns(
    target_pid: i32,
    if_name: &str,
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
) -> io::Result<OwnedFd> {
    if target_pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "target_pid must be positive",
        ));
    }
    let target_net = format!("/proc/{target_pid}/ns/net");
    let target_user = format!("/proc/{target_pid}/ns/user");
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

    let mut name_buf = [0 as libc::c_char; libc::IFNAMSIZ];
    for (dst, src) in name_buf.iter_mut().zip(if_name.bytes().take(libc::IFNAMSIZ - 1)) {
        *dst = src as libc::c_char;
    }

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
        unsafe {
            libc::close(sv[0]);
            let mut err_no: i32 = 0;
            let mut tap_fd: i32 = -1;

            let ns_failed = (user_fd >= 0 && libc::setns(user_fd, libc::CLONE_NEWUSER) != 0)
                || libc::setns(net_fd, libc::CLONE_NEWNET) != 0;
            if ns_failed {
                err_no = *libc::__errno_location();
            } else {
                let tun_path = c"/dev/net/tun";
                let fd = libc::open(tun_path.as_ptr(), libc::O_RDWR | libc::O_NONBLOCK);
                if fd < 0 {
                    err_no = *libc::__errno_location();
                } else {
                    let mut ifr: libc::ifreq = std::mem::zeroed();
                    ifr.ifr_name = name_buf;
                    ifr.ifr_ifru.ifru_flags = IFF_TAP | IFF_NO_PI;
                    if libc::ioctl(fd, TUNSETIFF, &raw mut ifr) < 0 {
                        err_no = *libc::__errno_location();
                        libc::close(fd);
                    } else if let Err(cfg_err) = configure_if_ipv4(&name_buf, ip, netmask) {
                        err_no = cfg_err;
                        libc::close(fd);
                    } else {
                        tap_fd = fd;
                    }
                }
            }

            let err_bytes = err_no.to_ne_bytes();
            let mut iov = libc::iovec {
                iov_base: err_bytes.as_ptr().cast_mut().cast::<libc::c_void>(),
                iov_len: err_bytes.len(),
            };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &raw mut iov;
            msg.msg_iovlen = 1;

            let mut cmsg_buf = [0u64; 4];
            if tap_fd >= 0 {
                let cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
                let cmsg_space = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize;
                msg.msg_control = cmsg_buf.as_mut_ptr().cast::<libc::c_void>();
                msg.msg_controllen = cmsg_space;
                let cmsg = libc::CMSG_FIRSTHDR(&raw const msg);
                if !cmsg.is_null() {
                    (*cmsg).cmsg_level = libc::SOL_SOCKET;
                    (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                    (*cmsg).cmsg_len = cmsg_len;
                    std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<i32>(), tap_fd);
                }
            }

            let _ = libc::sendmsg(sv[1], &raw const msg, libc::MSG_NOSIGNAL);
            if tap_fd >= 0 {
                libc::close(tap_fd);
            }
            libc::close(sv[1]);
            libc::_exit(0);
        }
    }

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
            "failed to receive TAP fd from netns helper child",
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
        return Err(io::Error::other("netns helper returned invalid TAP fd"));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(received_fd) })
}

unsafe fn configure_if_ipv4(
    name_buf: &[libc::c_char; libc::IFNAMSIZ],
    ip: Ipv4Addr,
    netmask: Ipv4Addr,
) -> Result<(), i32> {
    unsafe {
        let ctl = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if ctl < 0 {
            return Err(*libc::__errno_location());
        }

        let mut ifr_addr: libc::ifreq = std::mem::zeroed();
        ifr_addr.ifr_name = *name_buf;
        let sin = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(ip.octets()),
            },
            sin_zero: [0; 8],
        };
        std::ptr::copy_nonoverlapping(
            (&raw const sin).cast::<u8>(),
            (&raw mut ifr_addr.ifr_ifru.ifru_addr).cast::<u8>(),
            std::mem::size_of::<libc::sockaddr_in>(),
        );
        if libc::ioctl(ctl, libc::SIOCSIFADDR, &raw mut ifr_addr) < 0 {
            let e = *libc::__errno_location();
            libc::close(ctl);
            return Err(e);
        }

        let mut ifr_mask: libc::ifreq = std::mem::zeroed();
        ifr_mask.ifr_name = *name_buf;
        let sin_mask = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: 0,
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(netmask.octets()),
            },
            sin_zero: [0; 8],
        };
        std::ptr::copy_nonoverlapping(
            (&raw const sin_mask).cast::<u8>(),
            (&raw mut ifr_mask.ifr_ifru.ifru_netmask).cast::<u8>(),
            std::mem::size_of::<libc::sockaddr_in>(),
        );
        if libc::ioctl(ctl, libc::SIOCSIFNETMASK, &raw mut ifr_mask) < 0 {
            let e = *libc::__errno_location();
            libc::close(ctl);
            return Err(e);
        }

        let mut ifr_flags: libc::ifreq = std::mem::zeroed();
        ifr_flags.ifr_name = *name_buf;
        if libc::ioctl(ctl, libc::SIOCGIFFLAGS, &raw mut ifr_flags) == 0 {
            ifr_flags.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
            if libc::ioctl(ctl, libc::SIOCSIFFLAGS, &raw mut ifr_flags) < 0 {
                let e = *libc::__errno_location();
                libc::close(ctl);
                return Err(e);
            }
            if ifr_flags.ifr_ifru.ifru_flags != 0 {
                let gw_u32 = (u32::from(ip) & u32::from(netmask)) | 1;
                let gw_ip = Ipv4Addr::from(gw_u32);
                let mut rt: libc::rtentry = std::mem::zeroed();
                let sin_zero = libc::sockaddr_in {
                    sin_family: libc::AF_INET as libc::sa_family_t,
                    sin_port: 0,
                    sin_addr: libc::in_addr { s_addr: 0 },
                    sin_zero: [0; 8],
                };
                let sin_gw = libc::sockaddr_in {
                    sin_family: libc::AF_INET as libc::sa_family_t,
                    sin_port: 0,
                    sin_addr: libc::in_addr {
                        s_addr: u32::from_ne_bytes(gw_ip.octets()),
                    },
                    sin_zero: [0; 8],
                };
                std::ptr::copy_nonoverlapping(
                    (&raw const sin_zero).cast::<u8>(),
                    (&raw mut rt.rt_dst).cast::<u8>(),
                    std::mem::size_of::<libc::sockaddr_in>(),
                );
                std::ptr::copy_nonoverlapping(
                    (&raw const sin_zero).cast::<u8>(),
                    (&raw mut rt.rt_genmask).cast::<u8>(),
                    std::mem::size_of::<libc::sockaddr_in>(),
                );
                std::ptr::copy_nonoverlapping(
                    (&raw const sin_gw).cast::<u8>(),
                    (&raw mut rt.rt_gateway).cast::<u8>(),
                    std::mem::size_of::<libc::sockaddr_in>(),
                );
                rt.rt_flags = (libc::RTF_UP | libc::RTF_GATEWAY) as libc::c_ushort;
                let _ = libc::ioctl(ctl, libc::SIOCADDRT, &raw mut rt);
            }
        }
        libc::close(ctl);
        Ok(())
    }
}
