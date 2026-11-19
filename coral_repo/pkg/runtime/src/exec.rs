//! Real namespace-entering `exec` (`setns`) into a live running Coral container.

use coral_oci::{ContainerStatus, OciSpec};
use nix::fcntl::{OFlag, open};
use nix::sched::{CloneFlags, setns};
use nix::sys::stat::Mode;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, chdir, execve, fork, pipe2, read, write};
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use crate::pty::{PtyPair, attach_slave_stdio, relay_pty};
use crate::security;
use crate::store::{StateStore, is_process_alive};
use crate::RuntimeError;

/// Options controlling `coral exec` inside a running container.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    pub cmd: Vec<String>,
    pub env: Vec<String>,
    pub cwd: Option<String>,
    pub tty: bool,
    pub interactive: bool,
}

/// Executes `opts.cmd` inside the live namespaces of running container `id` via `setns()`.
pub fn exec_in_container(
    store: &StateStore,
    id: &str,
    opts: &ExecOptions,
) -> Result<i32, RuntimeError> {
    if opts.cmd.is_empty() {
        return Err(RuntimeError::InitFailed(
            "exec command must not be empty".into(),
        ));
    }

    let state = store.load(id)?;
    if state.status != ContainerStatus::Running {
        return Err(RuntimeError::InvalidState {
            id: id.into(),
            message: format!("container must be 'running' to exec (found '{}')", state.status),
        });
    }
    let target_pid = state.pid.ok_or_else(|| RuntimeError::InvalidState {
        id: id.into(),
        message: "container has no recorded PID".into(),
    })?;
    if !is_process_alive(target_pid) {
        return Err(RuntimeError::InvalidState {
            id: id.into(),
            message: format!("container PID {target_pid} is no longer alive"),
        });
    }

    // Inherit default env / cwd / privileged setting from the container's OCI spec if available.
    let spec = OciSpec::from_file(state.bundle.join("config.json")).ok();
    let privileged = spec
        .as_ref()
        .and_then(|s| s.annotations.get("coral.privileged"))
        .is_some_and(|v| v == "true");

    let cwd = opts.cwd.clone().unwrap_or_else(|| {
        spec.as_ref()
            .and_then(|s| s.process.as_ref())
            .map(|p| p.cwd.clone())
            .unwrap_or_else(|| "/".into())
    });

    let mut env_list = spec
        .as_ref()
        .and_then(|s| s.process.as_ref())
        .map(|p| p.env.clone())
        .unwrap_or_else(|| {
            vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                "TERM=xterm".into(),
            ]
        });
    for extra in &opts.env {
        if let Some((k, _)) = extra.split_once('=') {
            let prefix = format!("{k}=");
            env_list.retain(|e| !e.starts_with(&prefix));
        }
        env_list.push(extra.clone());
    }

    // Open target namespaces BEFORE forking so we compare cleanly against host /proc/self/ns/*.
    // Order is strictly required by Linux: user -> ipc/uts/net/cgroup -> pid -> mnt.
    let ns_specs: &[(&str, CloneFlags)] = &[
        ("user", CloneFlags::CLONE_NEWUSER),
        ("ipc", CloneFlags::CLONE_NEWIPC),
        ("uts", CloneFlags::CLONE_NEWUTS),
        ("net", CloneFlags::CLONE_NEWNET),
        ("cgroup", CloneFlags::CLONE_NEWCGROUP),
        ("pid", CloneFlags::CLONE_NEWPID),
        ("mnt", CloneFlags::CLONE_NEWNS),
    ];

    let mut ns_fds: Vec<(OwnedFd, CloneFlags, &'static str)> = Vec::new();
    for &(name, flag) in ns_specs {
        let target_path = format!("/proc/{target_pid}/ns/{name}");
        let self_path = format!("/proc/self/ns/{name}");
        let (Ok(t_meta), Ok(s_meta)) = (fs::metadata(&target_path), fs::metadata(&self_path)) else {
            continue;
        };
        if t_meta.dev() == s_meta.dev() && t_meta.ino() == s_meta.ino() {
            continue;
        }
        let raw_fd = open(
            target_path.as_str(),
            OFlag::O_RDONLY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| RuntimeError::nix(format!("open {target_path}"), e))?;
        let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
        ns_fds.push((owned, flag, name));
    }

    let pty = if opts.tty {
        Some(PtyPair::open()?)
    } else {
        None
    };

    let (p_err_r, p_err_w) =
        pipe2(OFlag::O_CLOEXEC).map_err(|e| RuntimeError::nix("pipe2(O_CLOEXEC)", e))?;

    match unsafe { fork() }.map_err(|e| RuntimeError::nix("fork exec intermediate", e))? {
        ForkResult::Parent { child: inter_pid } => {
            drop(ns_fds);
            drop(p_err_w);

            if let Some(pty_pair) = pty {
                drop(pty_pair.slave);
                // Check for immediate setup/exec errors before relaying? No: error pipe closes
                // cleanly on successful execve (O_CLOEXEC), so check it first!
                let err_msg = read_to_string_fd(&p_err_r);
                if !err_msg.is_empty() {
                    drop(pty_pair.master);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(err_msg));
                }
                relay_pty(pty_pair.master.as_raw_fd(), opts.interactive);
            } else {
                let err_msg = read_to_string_fd(&p_err_r);
                if !err_msg.is_empty() {
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(err_msg));
                }
            }

            let status = waitpid(inter_pid, None)
                .map_err(|e| RuntimeError::nix("waitpid exec child", e))?;
            Ok(wait_status_to_exit_code(status))
        }
        ForkResult::Child => {
            drop(p_err_r);
            let slave_fd: Option<RawFd> = pty.as_ref().map(|p| p.slave.as_raw_fd());
            if let Some(p) = &pty {
                let _ = unsafe { libc::close(p.master.as_raw_fd()) };
            }

            for (fd, flag, name) in &ns_fds {
                if let Err(e) = setns(fd, *flag) {
                    let msg = format!("setns({name}) failed: {e}");
                    let _ = write_all_fd(&p_err_w, msg.as_bytes());
                    unsafe { libc::_exit(127) };
                }
            }
            drop(ns_fds);

            // Second fork enters the target PID namespace.
            match unsafe { fork() } {
                Ok(ForkResult::Parent { child: exec_pid }) => {
                    drop(p_err_w);
                    if let Some(sfd) = slave_fd {
                        let _ = unsafe { libc::close(sfd) };
                    }
                    let code = match waitpid(exec_pid, None) {
                        Ok(st) => wait_status_to_exit_code(st),
                        Err(_) => 1,
                    };
                    unsafe { libc::_exit(code) };
                }
                Ok(ForkResult::Child) => {
                    if let Err(e) =
                        exec_child_finish(&opts.cmd, &env_list, &cwd, slave_fd, privileged)
                    {
                        let _ = write_all_fd(&p_err_w, e.to_string().as_bytes());
                        unsafe { libc::_exit(127) };
                    }
                    unsafe { libc::_exit(127) };
                }
                Err(e) => {
                    let _ = write_all_fd(&p_err_w, format!("exec second fork failed: {e}").as_bytes());
                    unsafe { libc::_exit(127) };
                }
            }
        }
    }
}

fn exec_child_finish(
    cmd: &[String],
    env_list: &[String],
    cwd: &str,
    slave_fd: Option<RawFd>,
    privileged: bool,
) -> Result<(), RuntimeError> {
    if let Some(sfd) = slave_fd {
        attach_slave_stdio(sfd)?;
    }

    chdir(Path::new(cwd))
        .map_err(|e| RuntimeError::nix(format!("chdir to '{cwd}' in container"), e))?;

    if !privileged {
        let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "prctl(PR_SET_NO_NEW_PRIVS)",
                io::Error::last_os_error(),
            ));
        }
        security::drop_capabilities()?;
        security::apply_default_seccomp()?;
    }

    let exe = resolve_executable(&cmd[0], env_list)?;
    let exe_cstr = CString::new(exe.as_str())
        .map_err(|e| RuntimeError::InitFailed(format!("invalid executable NUL byte: {e}")))?;
    let argv: Vec<CString> = cmd
        .iter()
        .map(|s| {
            CString::new(s.as_str())
                .map_err(|e| RuntimeError::InitFailed(format!("invalid arg NUL byte: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let envp: Vec<CString> = env_list
        .iter()
        .map(|s| {
            CString::new(s.as_str())
                .map_err(|e| RuntimeError::InitFailed(format!("invalid env NUL byte: {e}")))
        })
        .collect::<Result<_, _>>()?;

    execve(&exe_cstr, &argv, &envp)
        .map_err(|e| RuntimeError::nix(format!("execve '{exe}'"), e))?;
}

fn resolve_executable(prog: &str, env: &[String]) -> Result<String, RuntimeError> {
    if prog.contains('/') {
        return Ok(prog.to_string());
    }
    let path_var = env
        .iter()
        .find_map(|e| e.strip_prefix("PATH="))
        .unwrap_or("/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
    for dir in path_var.split(':') {
        let candidate = Path::new(dir).join(prog);
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err(RuntimeError::InitFailed(format!(
        "executable '{prog}' not found in container PATH '{path_var}'"
    )))
}

fn write_all_fd(fd: &OwnedFd, mut buf: &[u8]) -> Result<(), RuntimeError> {
    while !buf.is_empty() {
        let n = write(fd, buf).map_err(|e| RuntimeError::nix("write exec pipe", e))?;
        buf = &buf[n..];
    }
    Ok(())
}

fn read_to_string_fd(fd: &OwnedFd) -> String {
    let mut out = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match read(fd.as_raw_fd(), &mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn wait_status_to_exit_code(status: WaitStatus) -> i32 {
    match status {
        WaitStatus::Exited(_, code) => code,
        WaitStatus::Signaled(_, sig, _) => 128 + (sig as i32),
        _ => 1,
    }
}
