//! Pseudo-terminal (PTY) allocation, raw-mode host terminal guard, and I/O relay for `-it`.

use std::ffi::CStr;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::RuntimeError;

/// An allocated Linux PTY master/slave file descriptor pair.
pub struct PtyPair {
    pub master: OwnedFd,
    pub slave: OwnedFd,
}

impl PtyPair {
    /// Allocates a new PTY pair (`posix_openpt` + `grantpt` + `unlockpt` + `open(ptsname)`).
    pub fn open() -> Result<Self, RuntimeError> {
        let master_raw = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        if master_raw < 0 {
            return Err(RuntimeError::sys("posix_openpt", io::Error::last_os_error()));
        }
        let master = unsafe { OwnedFd::from_raw_fd(master_raw) };

        if unsafe { libc::grantpt(master.as_raw_fd()) } != 0 {
            return Err(RuntimeError::sys("grantpt", io::Error::last_os_error()));
        }
        if unsafe { libc::unlockpt(master.as_raw_fd()) } != 0 {
            return Err(RuntimeError::sys("unlockpt", io::Error::last_os_error()));
        }

        let mut name_buf = [0 as libc::c_char; 128];
        let rc = unsafe { libc::ptsname_r(master.as_raw_fd(), name_buf.as_mut_ptr(), name_buf.len()) };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "ptsname_r",
                io::Error::from_raw_os_error(rc),
            ));
        }
        let slave_name = unsafe { CStr::from_ptr(name_buf.as_ptr()) };
        let slave_raw = unsafe {
            libc::open(
                slave_name.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if slave_raw < 0 {
            return Err(RuntimeError::sys("open pty slave", io::Error::last_os_error()));
        }
        let slave = unsafe { OwnedFd::from_raw_fd(slave_raw) };

        // Mirror host terminal window size onto the PTY if stdin is a TTY.
        sync_winsize(libc::STDIN_FILENO, master.as_raw_fd());

        Ok(Self { master, slave })
    }
}

/// Copies terminal rows/columns from `src_fd` to `dst_fd` (best-effort).
pub fn sync_winsize(src_fd: RawFd, dst_fd: RawFd) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(src_fd, libc::TIOCGWINSZ, &mut ws) } == 0 {
        let _ = unsafe { libc::ioctl(dst_fd, libc::TIOCSWINSZ, &ws) };
    }
}

/// Inside the container child process: creates a new session, sets `slave_fd` as the
/// controlling terminal (`TIOCSCTTY`), and binds stdin/stdout/stderr to `slave_fd`.
pub fn attach_slave_stdio(slave_fd: RawFd) -> Result<(), RuntimeError> {
    if unsafe { libc::setsid() } < 0 {
        return Err(RuntimeError::sys("setsid for pty", io::Error::last_os_error()));
    }
    if unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY, 0) } < 0 {
        return Err(RuntimeError::sys("ioctl(TIOCSCTTY)", io::Error::last_os_error()));
    }
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if unsafe { libc::dup2(slave_fd, target) } < 0 {
            return Err(RuntimeError::sys("dup2 pty slave", io::Error::last_os_error()));
        }
    }
    if slave_fd > libc::STDERR_FILENO {
        let _ = unsafe { libc::close(slave_fd) };
    }
    Ok(())
}

/// RAII guard that puts the host stdin into raw mode (if it is a TTY) and restores it on drop.
pub struct RawTerminalGuard {
    fd: RawFd,
    original: Option<libc::termios>,
}

impl RawTerminalGuard {
    pub fn enter(fd: RawFd) -> Self {
        if unsafe { libc::isatty(fd) } == 0 {
            return Self { fd, original: None };
        }
        let mut orig: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut orig) } != 0 {
            return Self { fd, original: None };
        }
        let mut raw = orig;
        unsafe { libc::cfmakeraw(&mut raw) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Self { fd, original: None };
        }
        Self {
            fd,
            original: Some(orig),
        }
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if let Some(ref orig) = self.original {
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, orig) };
        }
    }
}

/// Relays bytes bidirectionally between host stdin/stdout and `master_fd` until the PTY closes.
pub fn relay_pty(master_fd: RawFd, interactive: bool) {
    let _guard = if interactive {
        Some(RawTerminalGuard::enter(libc::STDIN_FILENO))
    } else {
        None
    };

    let mut stdin_open = interactive;
    let mut buf = [0u8; 4096];

    loop {
        let mut fds = [
            libc::pollfd {
                fd: master_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if stdin_open { libc::STDIN_FILENO } else { -1 },
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        let nfds = if stdin_open { 2 } else { 1 };
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), nfds, 100) };
        if rc < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                sync_winsize(libc::STDIN_FILENO, master_fd);
                continue;
            }
            break;
        }
        if rc == 0 {
            sync_winsize(libc::STDIN_FILENO, master_fd);
            continue;
        }

        // 1. Container PTY output -> host stdout
        if (fds[0].revents & (libc::POLLIN | libc::POLLHUP)) != 0 {
            let n = unsafe { libc::read(master_fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            write_all_raw(libc::STDOUT_FILENO, &buf[..n as usize]);
        } else if (fds[0].revents & (libc::POLLERR | libc::POLLNVAL)) != 0 {
            break;
        }

        // 2. Host stdin -> container PTY input
        if stdin_open {
            if (fds[1].revents & libc::POLLIN) != 0 {
                let n = unsafe { libc::read(libc::STDIN_FILENO, buf.as_mut_ptr().cast(), buf.len()) };
                if n <= 0 {
                    stdin_open = false;
                } else {
                    write_all_raw(master_fd, &buf[..n as usize]);
                }
            } else if (fds[1].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL)) != 0 {
                stdin_open = false;
            }
        }
    }
}

fn write_all_raw(fd: RawFd, mut data: &[u8]) {
    while !data.is_empty() {
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n <= 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        data = &data[n as usize..];
    }
}
