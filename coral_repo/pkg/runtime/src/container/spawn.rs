//! Double-fork + `unshare` + UserNS pipe synchronization + Cgroups v2 attach
//! for foreground (`run`) and detached (`create`) container execution.

use super::init::{container_init_exec, resolve_target_user_in};
use super::{ContainerExit, ContainerManager};
use crate::RuntimeError;
use crate::namespace::{NamespacePlan, build_namespace_plan, write_userns_mappings};
use crate::overlay::OverlayDirs;
use crate::pty::{PtyPair, relay_pty};
use coral_cgroups::{CgroupApplyOutcome, CgroupConfig};
use coral_oci::{ContainerState, ContainerStatus, OciSpec};
use nix::fcntl::{OFlag, open};
use nix::sched::unshare;
use nix::sys::stat::Mode;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{ForkResult, fork, pipe2, read, write};
use std::fs;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicI32, Ordering};
use tracing::info;

static FORWARD_SIGNAL_PID: AtomicI32 = AtomicI32::new(0);

extern "C" fn forward_signal_handler(sig: libc::c_int) {
    let pid = FORWARD_SIGNAL_PID.load(Ordering::Relaxed);
    if pid > 0 {
        unsafe {
            let _ = libc::kill(pid, sig);
        }
    }
}

struct SignalForwardGuard {
    saved: [(libc::c_int, libc::sigaction); 4],
}

impl SignalForwardGuard {
    fn install(target_pid: i32) -> Self {
        FORWARD_SIGNAL_PID.store(target_pid, Ordering::SeqCst);
        let sigs = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT];
        let mut saved: [(libc::c_int, libc::sigaction); 4] =
            unsafe { std::mem::zeroed() };
        for (i, &sig) in sigs.iter().enumerate() {
            unsafe {
                let mut sa: libc::sigaction = std::mem::zeroed();
                sa.sa_sigaction = forward_signal_handler as *const () as usize;
                sa.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut sa.sa_mask);
                let mut old: libc::sigaction = std::mem::zeroed();
                let _ = libc::sigaction(sig, &sa, &mut old);
                saved[i] = (sig, old);
            }
        }
        Self { saved }
    }
}

impl Drop for SignalForwardGuard {
    fn drop(&mut self) {
        FORWARD_SIGNAL_PID.store(0, Ordering::SeqCst);
        for (sig, old) in &self.saved {
            unsafe {
                let _ = libc::sigaction(*sig, old, std::ptr::null_mut());
            }
        }
    }
}

impl ContainerManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_and_run_foreground(
        &self,
        id: &str,
        spec: &OciSpec,
        rootfs: &Path,
        overlay: Option<&OverlayDirs>,
        use_tty: bool,
        interactive: bool,
        state: &mut ContainerState,
    ) -> Result<ContainerExit, RuntimeError> {
        let mut ns_plan = build_namespace_plan(spec);
        adjust_single_uid_mapping(&mut ns_plan, spec, rootfs);
        let cgroup_cfg = spec
            .linux
            .as_ref()
            .and_then(|l| l.resources.as_ref())
            .map(CgroupConfig::from_oci)
            .unwrap_or_default();

        let pty = if use_tty {
            Some(PtyPair::open()?)
        } else {
            None
        };

        let (p_userns_r, p_userns_w) = new_cloexec_pipe()?;
        let (p_mapped_r, p_mapped_w) = new_cloexec_pipe()?;
        let (p_pid_r, p_pid_w) = new_cloexec_pipe()?;
        let (p_go_r, p_go_w) = new_cloexec_pipe()?;
        let (p_err_r, p_err_w) = new_cloexec_pipe()?;

        match unsafe { fork() }.map_err(|e| RuntimeError::nix("fork intermediate shim", e))? {
            ForkResult::Parent { child: inter_pid } => {
                drop(p_userns_w);
                drop(p_mapped_r);
                drop(p_pid_w);
                drop(p_go_r);
                drop(p_err_w);
                let pty_master = pty.map(|p| {
                    drop(p.slave);
                    p.master
                });

                // 1. Wait for intermediate child to unshare namespaces.
                let mut ready = [0u8; 1];
                if read_exact_fd(&p_userns_r, &mut ready).is_err() || ready[0] != b'R' {
                    let msg = read_to_string_fd(&p_err_r);
                    drop(pty_master);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(if msg.is_empty() {
                        "intermediate process exited before namespace setup".into()
                    } else {
                        msg
                    }));
                }

                // 2. Write UID/GID maps if running with UserNS.
                if let Err(err) = write_userns_mappings(inter_pid.as_raw(), &ns_plan) {
                    let _ = write_all_fd(&p_mapped_w, b"E");
                    drop(pty_master);
                    let _ = waitpid(inter_pid, None);
                    return Err(err);
                }
                write_all_fd(&p_mapped_w, b"G")?;
                drop(p_mapped_w);

                // 3. Receive host PID of container PID 1 from intermediate shim.
                let mut pid_buf = [0u8; 4];
                if read_exact_fd(&p_pid_r, &mut pid_buf).is_err() {
                    let msg = read_to_string_fd(&p_err_r);
                    drop(pty_master);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(if msg.is_empty() {
                        "failed to receive container PID 1".into()
                    } else {
                        msg
                    }));
                }
                let init_pid = i32::from_ne_bytes(pid_buf);

                // 4. Attach container PID 1 to Cgroups v2 BEFORE it executes target workload.
                let cg_outcome =
                    self.cgroups
                        .apply_best_effort(id, init_pid as u32, &cgroup_cfg)?;
                if let CgroupApplyOutcome::Applied { ref path } = cg_outcome {
                    state.cgroup_path = Some(path.clone());
                }
                state.pid = Some(init_pid);
                state.status = ContainerStatus::Running;
                self.store.save(state)?;

                // 5. Release container PID 1 to set up rootfs and execve.
                write_all_fd(&p_go_w, b"G")?;
                drop(p_go_w);

                // 6. Wait for exec error pipe: EOF means execve succeeded cleanly!
                let err_msg = read_to_string_fd(&p_err_r);
                if !err_msg.is_empty() {
                    drop(pty_master);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(err_msg));
                }

                // 7. Forward SIGTERM/SIGINT/SIGHUP/SIGQUIT to container PID 1 while attached,
                // and relay terminal I/O if a PTY is allocated.
                let _sig_guard = SignalForwardGuard::install(init_pid);
                if let Some(master) = pty_master {
                    relay_pty(master.as_raw_fd(), interactive);
                }

                // 8. Wait for intermediate shim (which waits for container PID 1).
                let wait_status = loop {
                    match waitpid(inter_pid, None) {
                        Ok(st) => break st,
                        Err(nix::errno::Errno::EINTR) => continue,
                        Err(e) => {
                            drop(_sig_guard);
                            return Err(RuntimeError::nix("waitpid intermediate shim", e));
                        }
                    }
                };
                drop(_sig_guard);
                let exit_code = wait_status_to_exit_code(wait_status);

                state.status = ContainerStatus::Stopped;
                state.exit_code = Some(exit_code);
                self.store.save(state)?;

                info!(id, pid = init_pid, exit_code, "container exited");
                Ok(ContainerExit {
                    container_id: id.into(),
                    pid: init_pid,
                    exit_code,
                    cgroup_outcome: cg_outcome,
                })
            }
            ForkResult::Child => {
                drop(p_userns_r);
                drop(p_mapped_w);
                drop(p_pid_r);
                drop(p_go_w);
                drop(p_err_r);
                let slave_fd: Option<RawFd> = pty.as_ref().map(|p| p.slave.as_raw_fd());
                if let Some(p) = &pty {
                    let _ = unsafe { libc::close(p.master.as_raw_fd()) };
                }

                // Unshare namespaces in intermediate process (never polluting parent!).
                if let Err(e) = unshare(ns_plan.flags) {
                    let _ = write_all_fd(&p_err_w, format!("unshare failed: {e}").as_bytes());
                    unsafe { libc::_exit(127) };
                }

                let _ = write_all_fd(&p_userns_w, b"R");
                drop(p_userns_w);

                let mut mapped = [0u8; 1];
                if read_exact_fd(&p_mapped_r, &mut mapped).is_err() || mapped[0] != b'G' {
                    unsafe { libc::_exit(127) };
                }
                drop(p_mapped_r);

                // Second fork: grandchild becomes PID 1 in the new PID namespace!
                match unsafe { fork() } {
                    Ok(ForkResult::Parent { child: init_pid }) => {
                        drop(p_go_r);
                        drop(p_err_w);
                        if let Some(sfd) = slave_fd {
                            let _ = unsafe { libc::close(sfd) };
                        }
                        let _ = write_all_fd(&p_pid_w, &init_pid.as_raw().to_ne_bytes());
                        drop(p_pid_w);

                        let code = match waitpid(init_pid, None) {
                            Ok(st) => wait_status_to_exit_code(st),
                            Err(_) => 1,
                        };
                        unsafe { libc::_exit(code) };
                    }
                    Ok(ForkResult::Child) => {
                        drop(p_pid_w);
                        let mut go = [0u8; 1];
                        if read_exact_fd(&p_go_r, &mut go).is_err() || go[0] != b'G' {
                            unsafe { libc::_exit(127) };
                        }
                        drop(p_go_r);

                        if let Err(err) = container_init_exec(
                            spec,
                            rootfs,
                            overlay,
                            slave_fd,
                            ns_plan.use_user_ns,
                            ns_plan.use_net_ns,
                        ) {
                            let _ = write_all_fd(&p_err_w, err.to_string().as_bytes());
                            unsafe { libc::_exit(127) };
                        }
                        unsafe { libc::_exit(127) };
                    }
                    Err(e) => {
                        let _ =
                            write_all_fd(&p_err_w, format!("second fork failed: {e}").as_bytes());
                        unsafe { libc::_exit(127) };
                    }
                }
            }
        }
    }

    pub(crate) fn spawn_created_container(
        &self,
        id: &str,
        spec: &OciSpec,
        rootfs: &Path,
        overlay: Option<&OverlayDirs>,
        fifo_path: &Path,
        state: &mut ContainerState,
    ) -> Result<(), RuntimeError> {
        let mut ns_plan = build_namespace_plan(spec);
        adjust_single_uid_mapping(&mut ns_plan, spec, rootfs);
        let cgroup_cfg = spec
            .linux
            .as_ref()
            .and_then(|l| l.resources.as_ref())
            .map(CgroupConfig::from_oci)
            .unwrap_or_default();

        let (p_userns_r, p_userns_w) = new_cloexec_pipe()?;
        let (p_mapped_r, p_mapped_w) = new_cloexec_pipe()?;
        let (p_pid_r, p_pid_w) = new_cloexec_pipe()?;
        let (p_err_r, p_err_w) = new_cloexec_pipe()?;

        match unsafe { fork() }.map_err(|e| RuntimeError::nix("fork detached shim", e))? {
            ForkResult::Parent { child: inter_pid } => {
                drop(p_userns_w);
                drop(p_mapped_r);
                drop(p_pid_w);
                drop(p_err_w);

                let mut ready = [0u8; 1];
                if read_exact_fd(&p_userns_r, &mut ready).is_err() || ready[0] != b'R' {
                    let msg = read_to_string_fd(&p_err_r);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(if msg.is_empty() {
                        "detached shim failed before unshare".into()
                    } else {
                        msg
                    }));
                }

                if let Err(err) = write_userns_mappings(inter_pid.as_raw(), &ns_plan) {
                    let _ = write_all_fd(&p_mapped_w, b"E");
                    let _ = waitpid(inter_pid, None);
                    return Err(err);
                }
                write_all_fd(&p_mapped_w, b"G")?;
                drop(p_mapped_w);

                let mut pid_buf = [0u8; 4];
                if read_exact_fd(&p_pid_r, &mut pid_buf).is_err() {
                    let msg = read_to_string_fd(&p_err_r);
                    let _ = waitpid(inter_pid, None);
                    return Err(RuntimeError::InitFailed(if msg.is_empty() {
                        "failed to receive created container PID".into()
                    } else {
                        msg
                    }));
                }
                let init_pid = i32::from_ne_bytes(pid_buf);

                let cg_outcome =
                    self.cgroups
                        .apply_best_effort(id, init_pid as u32, &cgroup_cfg)?;
                if let CgroupApplyOutcome::Applied { path } = cg_outcome {
                    state.cgroup_path = Some(path);
                }
                state.pid = Some(init_pid);
                state.status = ContainerStatus::Created;
                self.store.save(state)?;
                Ok(())
            }
            ForkResult::Child => {
                drop(p_userns_r);
                drop(p_mapped_w);
                drop(p_pid_r);
                drop(p_err_r);

                let _ = nix::unistd::setsid();
                redirect_stdio_to_devnull();

                if let Err(e) = unshare(ns_plan.flags) {
                    let _ = write_all_fd(&p_err_w, format!("unshare failed: {e}").as_bytes());
                    unsafe { libc::_exit(127) };
                }

                let _ = write_all_fd(&p_userns_w, b"R");
                drop(p_userns_w);

                let mut mapped = [0u8; 1];
                if read_exact_fd(&p_mapped_r, &mut mapped).is_err() || mapped[0] != b'G' {
                    unsafe { libc::_exit(127) };
                }
                drop(p_mapped_r);

                match unsafe { fork() } {
                    Ok(ForkResult::Parent { child: init_pid }) => {
                        drop(p_err_w);
                        let _ = write_all_fd(&p_pid_w, &init_pid.as_raw().to_ne_bytes());
                        drop(p_pid_w);

                        // Wait for container PID 1 in the background and update state on exit.
                        let code = match waitpid(init_pid, None) {
                            Ok(st) => wait_status_to_exit_code(st),
                            Err(_) => 1,
                        };
                        if let Ok(mut st) = self.store.load(id) {
                            st.status = ContainerStatus::Stopped;
                            st.exit_code = Some(code);
                            let _ = self.store.save(&st);
                        }
                        let _ = fs::remove_file(fifo_path);
                        let _ = self.cgroups.cleanup(id);
                        unsafe { libc::_exit(code) };
                    }
                    Ok(ForkResult::Child) => {
                        drop(p_pid_w);
                        drop(p_err_w);

                        // Block on exec.fifo until `coral start <id>` opens it for writing.
                        let Ok(fd) = open(fifo_path, OFlag::O_RDONLY, Mode::empty()) else {
                            unsafe { libc::_exit(127) };
                        };
                        let owned = unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) };
                        let mut sig = [0u8; 1];
                        let _ = read_exact_fd(&owned, &mut sig);
                        drop(owned);

                        if container_init_exec(
                            spec,
                            rootfs,
                            overlay,
                            None,
                            ns_plan.use_user_ns,
                            ns_plan.use_net_ns,
                        )
                        .is_err()
                        {
                            unsafe { libc::_exit(127) };
                        }
                        unsafe { libc::_exit(127) };
                    }
                    Err(e) => {
                        let _ =
                            write_all_fd(&p_err_w, format!("second fork failed: {e}").as_bytes());
                        unsafe { libc::_exit(127) };
                    }
                }
            }
        }
    }
}

pub(crate) fn new_cloexec_pipe() -> Result<(OwnedFd, OwnedFd), RuntimeError> {
    pipe2(OFlag::O_CLOEXEC).map_err(|e| RuntimeError::nix("pipe2(O_CLOEXEC)", e))
}

fn redirect_stdio_to_devnull() {
    if let Ok(fd) = open("/dev/null", OFlag::O_RDWR, Mode::empty()) {
        unsafe {
            let _ = libc::dup2(fd, libc::STDIN_FILENO);
            let _ = libc::dup2(fd, libc::STDOUT_FILENO);
            let _ = libc::dup2(fd, libc::STDERR_FILENO);
            if fd > libc::STDERR_FILENO {
                let _ = libc::close(fd);
            }
        }
    }
}

pub(crate) fn write_all_fd(fd: &OwnedFd, mut buf: &[u8]) -> Result<(), RuntimeError> {
    while !buf.is_empty() {
        let n = write(fd, buf).map_err(|e| RuntimeError::nix("write sync pipe", e))?;
        buf = &buf[n..];
    }
    Ok(())
}

pub(crate) fn read_exact_fd(fd: &OwnedFd, mut buf: &mut [u8]) -> Result<(), RuntimeError> {
    while !buf.is_empty() {
        let n = read(fd.as_raw_fd(), buf).map_err(|e| RuntimeError::nix("read sync pipe", e))?;
        if n == 0 {
            return Err(RuntimeError::InitFailed("unexpected EOF on sync pipe".into()));
        }
        buf = &mut buf[n..];
    }
    Ok(())
}

pub(crate) fn read_to_string_fd(fd: &OwnedFd) -> String {
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

pub(crate) fn wait_status_to_exit_code(status: WaitStatus) -> i32 {
    match status {
        WaitStatus::Exited(_, code) => code,
        WaitStatus::Signaled(_, sig, _) => 128 + (sig as i32),
        _ => 1,
    }
}

/// Maps requested non-root `uid`/`gid` directly to host caller UID/GID when operating in
/// single-UID rootless UserNS (`0 host 1`), allowing `postgres`/`redis`/`nginx` non-root
/// services to run natively without `newuidmap`.
fn adjust_single_uid_mapping(ns_plan: &mut NamespacePlan, spec: &OciSpec, rootfs: &Path) {
    if !ns_plan.use_user_ns {
        return;
    }
    let single_uid_default = ns_plan.uid_mappings.len() == 1
        && ns_plan.uid_mappings[0].container_id == 0
        && ns_plan.uid_mappings[0].size == 1;
    let single_gid_default = ns_plan.gid_mappings.len() == 1
        && ns_plan.gid_mappings[0].container_id == 0
        && ns_plan.gid_mappings[0].size == 1;
    if !single_uid_default && !single_gid_default {
        return;
    }
    let spec_user = spec.process.as_ref().map(|p| (p.user.uid, p.user.gid));
    let (target_uid, target_gid, _) = resolve_target_user_in(
        rootfs,
        spec.annotations.get("coral.user").map(String::as_str),
        spec_user,
    );
    if single_uid_default && target_uid != 0 {
        ns_plan.uid_mappings[0].container_id = target_uid;
    }
    if single_gid_default && target_gid != 0 {
        ns_plan.gid_mappings[0].container_id = target_gid;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coral_oci::spec::LinuxIdMapping;

    #[test]
    fn adjust_single_uid_mapping_remaps_non_root_service_user() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        std::fs::write(
            dir.path().join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/sh\npostgres:x:70:70:Postgres:/var/lib/postgresql:/bin/sh\n",
        )
        .unwrap();

        let mut spec = OciSpec::default_spec(true, 1000, 1000);
        spec.annotations.insert("coral.user".into(), "postgres".into());
        let mut plan = NamespacePlan {
            flags: nix::sched::CloneFlags::CLONE_NEWUSER,
            use_user_ns: true,
            use_net_ns: false,
            uid_mappings: vec![LinuxIdMapping {
                container_id: 0,
                host_id: 1000,
                size: 1,
            }],
            gid_mappings: vec![LinuxIdMapping {
                container_id: 0,
                host_id: 1000,
                size: 1,
            }],
        };

        adjust_single_uid_mapping(&mut plan, &spec, dir.path());
        assert_eq!(plan.uid_mappings[0].container_id, 70);
        assert_eq!(plan.gid_mappings[0].container_id, 70);

        // Multi-UID subuid mappings remain untouched so container keeps full 0..65536 range.
        plan.uid_mappings = vec![LinuxIdMapping {
            container_id: 0,
            host_id: 100000,
            size: 65536,
        }];
        adjust_single_uid_mapping(&mut plan, &spec, dir.path());
        assert_eq!(plan.uid_mappings[0].container_id, 0);
    }
}
