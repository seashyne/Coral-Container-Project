//! Bounded-size append log writer with `.1`..`.N` rotation and pluggable log drivers
//! (`raw`, `json-file`, `syslog`, `journald`).

use chrono::Utc;
use serde_json::json;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};

/// Container log formatting / forwarding driver (`--log-driver` / `coral.log.driver`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogDriver {
    #[default]
    Raw,
    JsonFile,
    Syslog,
    Journald,
}

impl LogDriver {
    pub fn from_name(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "json-file" | "json" => Self::JsonFile,
            "syslog" => Self::Syslog,
            "journald" => Self::Journald,
            _ => Self::Raw,
        }
    }
}

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
    driver: LogDriver,
    tag: String,
    rotations: u64,
}

impl RotatingLogWriter {
    #[cfg(test)]
    pub fn open(path: PathBuf, policy: LogRotationPolicy) -> std::io::Result<Self> {
        Self::open_with_driver(path, policy, LogDriver::Raw, "coral")
    }

    pub fn open_with_driver(
        path: PathBuf,
        policy: LogRotationPolicy,
        driver: LogDriver,
        tag: &str,
    ) -> std::io::Result<Self> {
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
            driver,
            tag: tag.to_string(),
            rotations: 0,
        })
    }

    pub fn write_chunk(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_stream_chunk(bytes, "stdout")
    }

    pub fn write_stream_chunk(&mut self, bytes: &[u8], stream_name: &str) -> std::io::Result<()> {
        let formatted: Vec<u8> = match self.driver {
            LogDriver::Raw => bytes.to_vec(),
            LogDriver::JsonFile => {
                let text = String::from_utf8_lossy(bytes);
                let now = Utc::now().to_rfc3339();
                let mut buf = Vec::new();
                for line in text.split_inclusive('\n') {
                    let obj = json!({
                        "log": line,
                        "stream": stream_name,
                        "time": now,
                    });
                    if let Ok(mut s) = serde_json::to_vec(&obj) {
                        s.push(b'\n');
                        buf.extend_from_slice(&s);
                    }
                }
                buf
            }
            LogDriver::Syslog | LogDriver::Journald => {
                forward_to_unix_syslog(&self.tag, bytes);
                bytes.to_vec()
            }
        };

        if self.policy.max_size_bytes > 0
            && self.written_bytes > 0
            && self.written_bytes.saturating_add(formatted.len() as u64)
                > self.policy.max_size_bytes
        {
            self.rotate()?;
        }
        self.file.write_all(&formatted)?;
        self.file.flush()?;
        self.written_bytes = self.written_bytes.saturating_add(formatted.len() as u64);
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

fn forward_to_unix_syslog(tag: &str, bytes: &[u8]) {
    let Ok(sock) = UnixDatagram::unbound() else {
        return;
    };
    let text = String::from_utf8_lossy(bytes);
    for target in ["/dev/log", "/run/systemd/journal/syslog"] {
        if Path::new(target).exists() {
            for line in text.lines() {
                let msg = format!("<30>coral[{tag}]: {line}");
                let _ = sock.send_to(msg.as_bytes(), target);
            }
            break;
        }
    }
}
