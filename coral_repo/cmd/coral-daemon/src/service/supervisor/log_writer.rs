//! Bounded-size append log writer with `.1`..`.N` rotation.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Log size-based rotation settings applied to supervised container stdout/stderr logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogRotationPolicy {
    /// Maximum size in bytes before rotating (`0` disables rotation).
    pub max_size_bytes: u64,
    /// Number of archived `.1`..`.N` rotated log files to retain (`0` truncates active file).
    pub max_files: usize,
}

impl Default for LogRotationPolicy {
    fn default() -> Self {
        Self {
            max_size_bytes: 10 * 1024 * 1024,
            max_files: 3,
        }
    }
}

/// Bounded-size append log writer supporting `.1`..`.N` rotation without restarting containers.
pub struct RotatingLogWriter {
    path: PathBuf,
    file: std::fs::File,
    written_bytes: u64,
    policy: LogRotationPolicy,
    rotations: u64,
}

impl RotatingLogWriter {
    pub fn open(path: PathBuf, policy: LogRotationPolicy) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let written_bytes = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written_bytes,
            policy,
            rotations: 0,
        })
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.policy.max_size_bytes > 0
            && self.written_bytes > 0
            && self.written_bytes.saturating_add(bytes.len() as u64) > self.policy.max_size_bytes
        {
            self.rotate()?;
        }
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.written_bytes = self.written_bytes.saturating_add(bytes.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom};
        let _ = self.file.flush();
        if self.policy.max_files == 0 {
            self.file.set_len(0)?;
            self.file.seek(SeekFrom::Start(0))?;
            self.written_bytes = 0;
            self.rotations = self.rotations.saturating_add(1);
            return Ok(());
        }

        let oldest = rotated_log_path(&self.path, self.policy.max_files);
        let _ = std::fs::remove_file(&oldest);
        for idx in (1..self.policy.max_files).rev() {
            let from = rotated_log_path(&self.path, idx);
            let to = rotated_log_path(&self.path, idx + 1);
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }
        let first = rotated_log_path(&self.path, 1);
        if self.path.exists() {
            std::fs::rename(&self.path, &first)?;
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.written_bytes = 0;
        self.rotations = self.rotations.saturating_add(1);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn rotations(&self) -> u64 {
        self.rotations
    }
}

pub(crate) fn rotated_log_path(base: &Path, index: usize) -> PathBuf {
    PathBuf::from(format!("{}.{}", base.display(), index))
}
