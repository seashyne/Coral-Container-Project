use coral_oci::{ContainerState, ContainerStatus, validate_container_id};
use nix::sys::signal::kill;
use nix::unistd::Pid;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::RuntimeError;

/// Counter that makes every temporary state file name unique.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Builds a temporary file path that no other writer will use.
///
/// The name carries the process id and a monotonic counter. Two processes can
/// write the same container's state at the same time — the daemon records the
/// spawn result while the forked container supervisor records the exit status
/// — and a shared name would let one of them rename the file away before the
/// other renames it, which fails with `ENOENT`.
fn unique_temp_path(dir: &Path) -> PathBuf {
    let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!("state.json.tmp.{}.{seq}", std::process::id()))
}

/// Moves the freshly written temporary file over the real state file.
///
/// A container that is removed while its state is being written takes its
/// state directory with it, and the rename then fails with `ENOENT`. There is
/// nothing left to persist in that case, so the save succeeds instead of
/// failing the whole container operation.
fn commit_state_file(dir: &Path, tmp: &Path, target: &Path) -> Result<(), RuntimeError> {
    match fs::rename(tmp, target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound && !dir.exists() => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(tmp);
            Err(RuntimeError::sys("rename state.json", e))
        }
    }
}

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
    ///
    /// The temporary file name is unique per write, so concurrent writers of
    /// the same container never race for the same file. The content is flushed
    /// to disk before the rename, so the file that becomes visible is never a
    /// partially written one.
    ///
    /// If the container's state directory disappears while the write is in
    /// flight the container was removed concurrently. There is then nothing
    /// left to persist, so the save succeeds instead of failing the whole
    /// container operation.
    pub fn save(&self, state: &ContainerState) -> Result<(), RuntimeError> {
        validate_container_id(&state.id)?;
        let dir = self.container_dir(&state.id);
        fs::create_dir_all(&dir)
            .map_err(|e| RuntimeError::sys("ensure container state dir", e))?;

        let target = self.state_file(&state.id);
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| RuntimeError::InitFailed(format!("serialize state.json: {e}")))?;

        let tmp = unique_temp_path(&dir);
        let write_result = (|| -> io::Result<()> {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(format!("{json}\n").as_bytes())?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(e) = write_result {
            let _ = fs::remove_file(&tmp);
            return Err(RuntimeError::sys("write temporary state.json", e));
        }

        commit_state_file(&dir, &tmp, &target)?;

        // Best effort: make the rename itself durable. A failure here does not
        // invalidate the state that was just written.
        if let Ok(dir_handle) = fs::File::open(&dir) {
            let _ = dir_handle.sync_all();
        }
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

    /// Regression test: two writers of the same container's state must not
    /// race for the same temporary file.
    ///
    /// The daemon records the spawn result while the forked container
    /// supervisor records the exit status. Both wrote through a shared
    /// `state.json.tmp`, so whichever renamed first left the other with
    /// `rename state.json: No such file or directory`.
    #[test]
    fn save_survives_concurrent_writers_of_the_same_container() {
        let dir = tempdir().unwrap();
        let store = StateStore::new(dir.path());

        let base = ContainerState::new("c1", PathBuf::from("/tmp/bundle"), BTreeMap::new());
        store.init_container(&base).unwrap();

        let mut handles = Vec::new();
        for worker in 0..8 {
            let worker_store = store.clone();
            let worker_base = base.clone();
            handles.push(std::thread::spawn(move || {
                for round in 0..25 {
                    let mut state = worker_base.clone();
                    state.exit_code = Some(worker * 100 + round);
                    state.status = ContainerStatus::Stopped;
                    worker_store.save(&state).unwrap_or_else(|e| {
                        panic!("concurrent save failed on worker {worker} round {round}: {e}")
                    });
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        // The file that survived must still be complete and parseable.
        let loaded = store.load("c1").unwrap();
        assert_eq!(loaded.status, ContainerStatus::Stopped);
        assert!(loaded.exit_code.is_some());
    }

    /// A save must not leave temporary files behind.
    #[test]
    fn save_leaves_no_temporary_files_behind() {
        let dir = tempdir().unwrap();
        let store = StateStore::new(dir.path());

        let state = ContainerState::new("c1", PathBuf::from("/tmp/bundle"), BTreeMap::new());
        store.init_container(&state).unwrap();
        store.save(&state).unwrap();

        let leftovers: Vec<_> = fs::read_dir(store.container_dir("c1"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("state.json.tmp"))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
    }

    /// Every temporary file name must be distinct, even within one process.
    #[test]
    fn temporary_state_paths_are_unique() {
        let dir = tempdir().unwrap();
        let first = unique_temp_path(dir.path());
        let second = unique_temp_path(dir.path());
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(dir.path()));
    }

    /// A container removed while its state is being written must not fail the
    /// whole operation: there is simply nothing left to persist.
    #[test]
    fn commit_tolerates_a_container_removed_mid_write() {
        let dir = tempdir().unwrap();
        let container_dir = dir.path().join("c1");
        let tmp = container_dir.join("state.json.tmp.1.0");
        let target = container_dir.join("state.json");

        // The container directory is gone before the rename happens, which is
        // exactly what the daemon does when it deletes a container while the
        // container supervisor is still recording the exit status.
        commit_state_file(&container_dir, &tmp, &target)
            .expect("a concurrently removed container must not fail the save");
    }

    /// A rename that fails for any other reason must still surface the error.
    #[test]
    fn commit_reports_other_rename_failures() {
        let dir = tempdir().unwrap();
        let container_dir = dir.path().join("c1");
        fs::create_dir_all(&container_dir).unwrap();
        let tmp = container_dir.join("state.json.tmp.1.0");
        let target = container_dir.join("state.json");

        // The temporary file does not exist, and the container directory does,
        // so this is a genuine failure rather than a concurrent removal.
        let err = commit_state_file(&container_dir, &tmp, &target)
            .expect_err("a missing temporary file must be reported");
        assert!(matches!(err, RuntimeError::Sys { .. }));
    }
}
