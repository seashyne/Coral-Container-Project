use coral_oci::{ContainerState, ContainerStatus, validate_container_id};
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::RuntimeError;

/// Resolves the default Coral container state root directory.
/// - Root (`euid == 0`): `/run/coral`
/// - Rootless (`euid != 0`): `$XDG_RUNTIME_DIR/coral` or `/tmp/coral-<uid>`
pub fn default_state_root() -> PathBuf {
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return PathBuf::from("/run/coral");
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR")
        && !xdg.trim().is_empty()
    {
        return PathBuf::from(xdg).join("coral");
    }
    PathBuf::from(format!("/tmp/coral-{euid}"))
}

/// Manages on-disk OCI container state directories (`<root>/<id>/state.json`).
#[derive(Debug, Clone)]
pub struct StateStore {
    root: PathBuf,
}

impl StateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn container_dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    pub fn state_file(&self, id: &str) -> PathBuf {
        self.container_dir(id).join("state.json")
    }

    pub fn fifo_file(&self, id: &str) -> PathBuf {
        self.container_dir(id).join("exec.fifo")
    }

    /// Creates a fresh directory for `id` (errors if it already exists).
    pub fn init_container(&self, state: &ContainerState) -> Result<PathBuf, RuntimeError> {
        validate_container_id(&state.id)?;
        fs::create_dir_all(&self.root)
            .map_err(|e| RuntimeError::sys("create state root", e))?;

        let dir = self.container_dir(&state.id);
        if dir.exists() {
            return Err(RuntimeError::AlreadyExists(state.id.clone()));
        }
        fs::create_dir(&dir).map_err(|e| RuntimeError::sys("create container state dir", e))?;
        self.save(state)?;
        Ok(dir)
    }

    /// Atomically persists `state.json` via write-to-temp + rename.
    pub fn save(&self, state: &ContainerState) -> Result<(), RuntimeError> {
        validate_container_id(&state.id)?;
        let dir = self.container_dir(&state.id);
        fs::create_dir_all(&dir)
            .map_err(|e| RuntimeError::sys("ensure container state dir", e))?;

        let tmp = dir.join("state.json.tmp");
        let target = self.state_file(&state.id);
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| RuntimeError::InitFailed(format!("serialize state.json: {e}")))?;

        fs::write(&tmp, format!("{json}\n"))
            .map_err(|e| RuntimeError::sys("write temporary state.json", e))?;
        fs::rename(&tmp, &target).map_err(|e| RuntimeError::sys("rename state.json", e))?;
        Ok(())
    }

    /// Loads `state.json` and reconciles `Running`/`Created` status against actual PID liveness.
    pub fn load(&self, id: &str) -> Result<ContainerState, RuntimeError> {
        validate_container_id(id)?;
        let path = self.state_file(id);
        if !path.exists() {
            return Err(RuntimeError::NotFound(id.to_string()));
        }
        let content = fs::read_to_string(&path)
            .map_err(|e| RuntimeError::sys("read state.json", e))?;
        let mut state: ContainerState = serde_json::from_str(&content)
            .map_err(|e| RuntimeError::InitFailed(format!("parse state.json: {e}")))?;

        if matches!(
            state.status,
            ContainerStatus::Running | ContainerStatus::Created
        ) && let Some(pid) = state.pid
            && !is_process_alive(pid)
        {
            state.status = ContainerStatus::Stopped;
            let _ = self.save(&state);
        }

        Ok(state)
    }

    /// Lists all containers stored under this root directory.
    pub fn list(&self) -> Result<Vec<ContainerState>, RuntimeError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut items = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|e| RuntimeError::sys("read state root dir", e))?
        {
            let entry = entry.map_err(|e| RuntimeError::sys("read state dir entry", e))?;
            if entry.path().is_dir()
                && let Some(id) = entry.file_name().to_str()
                && let Ok(state) = self.load(id)
            {
                items.push(state);
            }
        }
        items.sort_by_key(|a| a.created);
        Ok(items)
    }

    /// Removes the container's state directory.
    pub fn remove(&self, id: &str) -> Result<(), RuntimeError> {
        validate_container_id(id)?;
        let dir = self.container_dir(id);
        match crate::multi_uid::remove_dir_all_rootless_safe(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(RuntimeError::NotFound(id.into())),
            Err(e) => Err(RuntimeError::sys("remove container state dir", e)),
        }
    }
}

/// Checks if a Linux PID still exists and is not a dead zombie.
pub fn is_process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    if kill(Pid::from_raw(pid), None).is_err() {
        return false;
    }
    // Check /proc/<pid>/stat to see if the process is already a zombie ('Z').
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some(rparen) = stat.rfind(')')
        && let Some(state_ch) = stat[rparen + 1..].trim_start().chars().next()
        && state_ch == 'Z'
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    #[test]
    fn state_store_roundtrip_and_list() {
        let dir = tempdir().unwrap();
        let store = StateStore::new(dir.path());

        let mut st = ContainerState::new("c1", PathBuf::from("/tmp/bundle"), BTreeMap::new());
        store.init_container(&st).unwrap();
        assert!(matches!(
            store.init_container(&st),
            Err(RuntimeError::AlreadyExists(_))
        ));

        st.status = ContainerStatus::Stopped;
        st.exit_code = Some(0);
        store.save(&st).unwrap();

        let loaded = store.load("c1").unwrap();
        assert_eq!(loaded.status, ContainerStatus::Stopped);
        assert_eq!(loaded.exit_code, Some(0));
        assert_eq!(store.list().unwrap().len(), 1);

        store.remove("c1").unwrap();
        assert!(matches!(store.load("c1"), Err(RuntimeError::NotFound(_))));
    }
}
