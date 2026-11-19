//! OCI container lifecycle manager (`run`, `create`, `start`, `exec`, `state`, `list`, `kill`, `delete`).

mod init;
mod spawn;

use self::init::{prepare_overlay_if_requested, resolve_rootfs};
use self::spawn::write_all_fd;
use crate::exec::{ExecOptions, exec_in_container};
use crate::store::{StateStore, is_process_alive};
use crate::{RuntimeError, default_state_root};
use coral_cgroups::{CgroupApplyOutcome, CgroupManager};
use coral_oci::{ContainerState, ContainerStatus, OciSpec, validate_container_id};
use nix::fcntl::{OFlag, open};
use nix::sys::signal::{Signal, kill};
use nix::sys::stat::Mode;
use nix::unistd::{Pid, mkfifo};
use std::fs;
use std::path::{Path, PathBuf};

/// Options controlling foreground `run` execution.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Keep the container's state directory after exit instead of auto-deleting.
    pub keep_state: bool,
    /// Allocate a Linux PTY and attach stdin/stdout/stderr for interactive shells (`-t`).
    pub tty: bool,
    /// Forward host stdin into the container PTY (`-i`).
    pub interactive: bool,
}

/// Exit outcome returned by foreground container execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerExit {
    pub container_id: String,
    pub pid: i32,
    pub exit_code: i32,
    pub cgroup_outcome: CgroupApplyOutcome,
}

/// High-level manager implementing the OCI container runtime lifecycle.
#[derive(Debug, Clone)]
pub struct ContainerManager {
    pub(crate) store: StateStore,
    pub(crate) cgroups: CgroupManager,
}

impl ContainerManager {
    pub fn new(state_root: impl Into<PathBuf>, cgroups: CgroupManager) -> Self {
        Self {
            store: StateStore::new(state_root),
            cgroups,
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(default_state_root(), CgroupManager::detect())
    }

    pub fn store(&self) -> &StateStore {
        &self.store
    }

    pub fn cgroups(&self) -> &CgroupManager {
        &self.cgroups
    }

    /// Runs a container in the foreground from an OCI bundle (`config.json` + `rootfs`),
    /// streams stdio directly, enforces namespaces + cgroups v2, and returns its exit status.
    pub fn run(
        &self,
        id: &str,
        bundle: impl AsRef<Path>,
        opts: &RunOptions,
    ) -> Result<ContainerExit, RuntimeError> {
        validate_container_id(id)?;
        let bundle_abs = fs::canonicalize(bundle.as_ref())
            .map_err(|e| RuntimeError::sys("canonicalize bundle directory", e))?;
        let spec = OciSpec::from_file(bundle_abs.join("config.json"))?;
        let rootfs_abs = resolve_rootfs(&bundle_abs, &spec)?;

        // Prepare OverlayFS CoW directories (mounted inside the container's private mount namespace).
        let overlay = prepare_overlay_if_requested(&bundle_abs, &rootfs_abs, &spec)?;

        let mut state = ContainerState::new(id, bundle_abs.clone(), spec.annotations.clone());
        self.store.init_container(&state)?;

        let use_tty = opts.tty || spec.process.as_ref().is_some_and(|p| p.terminal);
        let outcome = self.spawn_and_run_foreground(
            id,
            &spec,
            &rootfs_abs,
            overlay.as_ref(),
            use_tty,
            opts.interactive,
            &mut state,
        );

        match outcome {
            Ok(exit) => {
                let _ = self.cgroups.cleanup(id);
                if !opts.keep_state {
                    let _ = self.store.remove(id);
                }
                Ok(exit)
            }
            Err(err) => {
                let _ = self.cgroups.cleanup(id);
                if !opts.keep_state {
                    let _ = self.store.remove(id);
                }
                Err(err)
            }
        }
    }

    /// Implements OCI `create`: initializes namespaces, cgroups v2, and state (`created`),
    /// pausing PID 1 on `exec.fifo` until `start(id)` is invoked.
    pub fn create(
        &self,
        id: &str,
        bundle: impl AsRef<Path>,
    ) -> Result<ContainerState, RuntimeError> {
        validate_container_id(id)?;
        let bundle_abs = fs::canonicalize(bundle.as_ref())
            .map_err(|e| RuntimeError::sys("canonicalize bundle directory", e))?;
        let spec = OciSpec::from_file(bundle_abs.join("config.json"))?;
        let rootfs_abs = resolve_rootfs(&bundle_abs, &spec)?;
        let overlay = prepare_overlay_if_requested(&bundle_abs, &rootfs_abs, &spec)?;

        let mut state = ContainerState::new(id, bundle_abs.clone(), spec.annotations.clone());
        self.store.init_container(&state)?;

        let fifo_path = self.store.fifo_file(id);
        if let Err(e) = mkfifo(&fifo_path, Mode::S_IRUSR | Mode::S_IWUSR) {
            let _ = self.store.remove(id);
            return Err(RuntimeError::nix("create exec.fifo", e));
        }

        match self.spawn_created_container(
            id,
            &spec,
            &rootfs_abs,
            overlay.as_ref(),
            &fifo_path,
            &mut state,
        ) {
            Ok(()) => Ok(state),
            Err(err) => {
                let _ = self.cgroups.cleanup(id);
                let _ = self.store.remove(id);
                Err(err)
            }
        }
    }

    /// Executes a command directly inside a live running container's namespaces via `setns()`.
    pub fn exec(&self, id: &str, opts: &ExecOptions) -> Result<i32, RuntimeError> {
        exec_in_container(&self.store, id, opts)
    }

    /// Implements OCI `start`: signals a `created` container via `exec.fifo` to execute its entrypoint.
    pub fn start(&self, id: &str) -> Result<ContainerState, RuntimeError> {
        let mut state = self.store.load(id)?;
        if state.status != ContainerStatus::Created {
            return Err(RuntimeError::InvalidState {
                id: id.into(),
                message: format!("expected 'created', found '{}'", state.status),
            });
        }

        let fifo_path = self.store.fifo_file(id);
        let fd = open(&fifo_path, OFlag::O_WRONLY, Mode::empty())
            .map_err(|e| RuntimeError::nix("open exec.fifo for start", e))?;
        let owned = unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) };
        write_all_fd(&owned, b"S")?;
        drop(owned);

        state.status = ContainerStatus::Running;
        self.store.save(&state)?;
        Ok(state)
    }

    /// Implements OCI `state`.
    pub fn state(&self, id: &str) -> Result<ContainerState, RuntimeError> {
        self.store.load(id)
    }

    /// Lists all known containers.
    pub fn list(&self) -> Result<Vec<ContainerState>, RuntimeError> {
        self.store.list()
    }

    /// Implements OCI `kill`: sends a signal to the container's PID 1.
    pub fn kill(&self, id: &str, signal: Signal) -> Result<(), RuntimeError> {
        let state = self.store.load(id)?;
        let pid = state.pid.ok_or_else(|| RuntimeError::InvalidState {
            id: id.into(),
            message: "container has no recorded PID".into(),
        })?;
        if !is_process_alive(pid) {
            return Err(RuntimeError::InvalidState {
                id: id.into(),
                message: "container process is not running".into(),
            });
        }
        kill(Pid::from_raw(pid), signal)
            .map_err(|e| RuntimeError::nix(format!("send {signal} to PID {pid}"), e))?;
        Ok(())
    }

    /// Implements OCI `delete`: removes a stopped container (or force-kills if `force == true`).
    pub fn delete(&self, id: &str, force: bool) -> Result<(), RuntimeError> {
        let state = self.store.load(id)?;
        if matches!(
            state.status,
            ContainerStatus::Running | ContainerStatus::Created
        ) {
            if !force {
                return Err(RuntimeError::InvalidState {
                    id: id.into(),
                    message: format!(
                        "cannot delete container in '{}' state without --force",
                        state.status
                    ),
                });
            }
            if let Some(pid) = state.pid
                && is_process_alive(pid)
            {
                let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
                for _ in 0..20 {
                    if !is_process_alive(pid) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
        }

        let _ = self.cgroups.cleanup(id);
        self.store.remove(id)?;
        Ok(())
    }
}

/// Parses common signal names (`SIGTERM`, `TERM`, `KILL`, `SIGKILL`, `9`, etc.).
pub fn parse_signal(raw: &str) -> Result<Signal, RuntimeError> {
    let trimmed = raw.trim().to_ascii_uppercase();
    if let Ok(num) = trimmed.parse::<i32>() {
        return Signal::try_from(num)
            .map_err(|_| RuntimeError::InitFailed(format!("unsupported signal number '{raw}'")));
    }
    let normalized = if trimmed.starts_with("SIG") {
        trimmed
    } else {
        format!("SIG{trimmed}")
    };
    normalized
        .parse::<Signal>()
        .map_err(|_| RuntimeError::InitFailed(format!("unknown signal '{raw}'")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signal_names_and_numbers() {
        assert_eq!(parse_signal("SIGTERM").unwrap(), Signal::SIGTERM);
        assert_eq!(parse_signal("kill").unwrap(), Signal::SIGKILL);
        assert_eq!(parse_signal("9").unwrap(), Signal::SIGKILL);
        assert!(parse_signal("INVALID_SIG").is_err());
    }
}
