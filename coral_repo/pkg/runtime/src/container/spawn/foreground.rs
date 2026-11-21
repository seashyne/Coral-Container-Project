//! Foreground (`run`) container execution: double-fork + `unshare` + UserNS synchronization.

use super::{
    SignalForwardGuard, new_cloexec_pipe, read_exact_fd, read_to_string_fd, resolve_namespace_plan,
    wait_status_to_exit_code, write_all_fd,
};
use super::super::{ContainerExit, ContainerManager};
use crate::RuntimeError;
use crate::namespace::write_userns_mappings;
use crate::overlay::OverlayDirs;
use crate::pty::{PtyPair, relay_pty};
use coral_cgroups::{CgroupApplyOutcome, CgroupConfig};
use coral_oci::{ContainerState, ContainerStatus, OciSpec};
use nix::sched::unshare;
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, fork};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use tracing::info;

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
        let ns_plan = resolve_namespace_plan(spec, rootfs);
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

                        if let Err(err) = super::super::init::container_init_exec(
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
}
