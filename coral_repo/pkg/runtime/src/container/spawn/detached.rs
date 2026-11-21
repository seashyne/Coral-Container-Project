//! Detached (`create`) container execution: double-fork + `unshare` + `exec.fifo` gating.

use super::{
    new_cloexec_pipe, read_exact_fd, read_to_string_fd, redirect_stdio_to_devnull,
    resolve_namespace_plan, wait_status_to_exit_code, write_all_fd,
};
use super::super::ContainerManager;
use crate::RuntimeError;
use crate::namespace::write_userns_mappings;
use crate::overlay::OverlayDirs;
use coral_cgroups::{CgroupApplyOutcome, CgroupConfig};
use coral_oci::{ContainerState, ContainerStatus, OciSpec};
use nix::fcntl::{OFlag, open};
use nix::sched::unshare;
use nix::sys::stat::Mode;
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, fork};
use std::fs;
use std::path::Path;

impl ContainerManager {
    pub(crate) fn spawn_created_container(
        &self,
        id: &str,
        spec: &OciSpec,
        rootfs: &Path,
        overlay: Option<&OverlayDirs>,
        fifo_path: &Path,
        state: &mut ContainerState,
    ) -> Result<(), RuntimeError> {
        let ns_plan = resolve_namespace_plan(spec, rootfs);
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

                        if super::super::init::container_init_exec(
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
