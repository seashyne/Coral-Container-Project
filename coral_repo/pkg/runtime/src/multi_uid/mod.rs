//! Multi-UID Rootless User Namespace support via `/etc/subuid`, `/etc/subgid`,
//! `newuidmap`, and `newgidmap`, with automatic single-UID fallback.

mod apply;
mod cleanup;
mod parse;
mod probe;
#[cfg(test)]
mod tests;

use crate::RuntimeError;
use coral_oci::LinuxIdMapping;
use std::ffi::CStr;
use std::fs;
use std::path::PathBuf;

const SUBUID_PATH: &str = "/etc/subuid";
const SUBGID_PATH: &str = "/etc/subgid";

/// Subordinate UID or GID range parsed from `/etc/subuid` or `/etc/subgid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubIdRange {
    pub start: u32,
    pub count: u32,
}

/// Host Multi-UID rootless capability summary (`subuid`/`subgid` + `newuidmap`/`newgidmap`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultiUidSupport {
    pub host_uid: u32,
    pub host_gid: u32,
    pub username: Option<String>,
    pub subuid_ranges: Vec<SubIdRange>,
    pub subgid_ranges: Vec<SubIdRange>,
    pub newuidmap: Option<PathBuf>,
    pub newgidmap: Option<PathBuf>,
    pub no_new_privs: bool,
}

impl MultiUidSupport {
    /// Probes the current Linux host for Multi-UID rootless support.
    pub fn detect() -> Self {
        let host_uid = unsafe { libc::geteuid() };
        let host_gid = unsafe { libc::getegid() };
        let username = resolve_username(host_uid);
        let subuid_ranges = fs::read_to_string(SUBUID_PATH)
            .map(|raw| parse::parse_subid_ranges(&raw, username.as_deref(), host_uid))
            .unwrap_or_default();
        let subgid_ranges = fs::read_to_string(SUBGID_PATH)
            .map(|raw| parse::parse_subid_ranges(&raw, username.as_deref(), host_uid))
            .unwrap_or_default();

        Self {
            host_uid,
            host_gid,
            username,
            subuid_ranges,
            subgid_ranges,
            newuidmap: probe::find_executable("newuidmap"),
            newgidmap: probe::find_executable("newgidmap"),
            no_new_privs: probe::has_no_new_privs(),
        }
    }

    /// Returns `true` when unprivileged Multi-UID mapping can execute via `newuidmap`/`newgidmap`.
    pub fn is_available(&self) -> bool {
        self.host_uid != 0
            && !self.no_new_privs
            && !self.subuid_ranges.is_empty()
            && !self.subgid_ranges.is_empty()
            && self.newuidmap.is_some()
            && self.newgidmap.is_some()
    }

    /// Builds container UID mappings (`0 -> host_uid`, `1.. -> subuid...`).
    pub fn uid_mappings(&self) -> Vec<LinuxIdMapping> {
        parse::build_subid_mappings(self.host_uid, &self.subuid_ranges, parse::MAX_CONTAINER_SUBIDS)
    }

    /// Builds container GID mappings (`0 -> host_gid`, `1.. -> subgid...`).
    pub fn gid_mappings(&self) -> Vec<LinuxIdMapping> {
        parse::build_subid_mappings(self.host_gid, &self.subgid_ranges, parse::MAX_CONTAINER_SUBIDS)
    }

    /// Human-readable reason when Multi-UID rootless is inactive, for diagnostics.
    pub fn unavailable_reason(&self) -> &'static str {
        if self.host_uid == 0 {
            "running as host root (Multi-UID helper not needed)"
        } else if self.no_new_privs {
            "PR_SET_NO_NEW_PRIVS is active (setuid newuidmap disabled)"
        } else if self.newuidmap.is_none() || self.newgidmap.is_none() {
            "newuidmap/newgidmap not installed or on a nosuid mount (install uidmap)"
        } else if self.subuid_ranges.is_empty() || self.subgid_ranges.is_empty() {
            "no subordinate UID/GID ranges allocated in /etc/subuid and /etc/subgid"
        } else {
            "available"
        }
    }
}

/// Resolves the POSIX username for `uid`, falling back to `$USER` / `$LOGNAME`.
pub fn resolve_username(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    loop {
        let rc = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr().cast::<libc::c_char>(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 65_536 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc == 0 && !result.is_null() && !pwd.pw_name.is_null() {
            let name = unsafe { CStr::from_ptr(pwd.pw_name) }
                .to_string_lossy()
                .into_owned();
            if !name.is_empty() {
                return Some(name);
            }
        }
        break;
    }

    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Parses subordinate ID ranges matching `username` or numeric `uid` from `/etc/subuid` or `/etc/subgid`.
pub fn parse_subid_ranges(content: &str, username: Option<&str>, uid: u32) -> Vec<SubIdRange> {
    parse::parse_subid_ranges(content, username, uid)
}

/// Constructs OCI ID mappings placing `0 -> host_id (1)` followed by subordinate ranges for `1..`.
pub fn build_subid_mappings(
    host_id: u32,
    ranges: &[SubIdRange],
    max_subids: u32,
) -> Vec<LinuxIdMapping> {
    parse::build_subid_mappings(host_id, ranges, max_subids)
}

/// Formats `LinuxIdMapping` entries as command-line arguments (`[cid, hid, size, ...]`) for
/// `newuidmap` / `newgidmap`.
pub fn format_helper_args(pid: i32, mappings: &[LinuxIdMapping]) -> Vec<String> {
    parse::format_helper_args(pid, mappings)
}

/// Applies UserNS UID/GID mappings for `pid`, using `newuidmap`/`newgidmap` whenever mappings
/// require unprivileged subordinate ranges or multiple extents, and falling back to direct
/// `/proc/<pid>/{setgroups,uid_map,gid_map}` writes for root or single-UID self-mapping.
pub fn apply_userns_mappings(
    pid: i32,
    uid_mappings: &[LinuxIdMapping],
    gid_mappings: &[LinuxIdMapping],
    format_direct: impl Fn(&[LinuxIdMapping]) -> String,
) -> Result<(), RuntimeError> {
    apply::apply_userns_mappings(pid, uid_mappings, gid_mappings, format_direct)
}

/// Removes `path` recursively, automatically retrying inside an unshared Multi-UID UserNS
/// (`newuidmap`/`newgidmap`) if subordinate-UID-owned subdirectories cause `PermissionDenied`.
pub fn remove_dir_all_rootless_safe(path: &std::path::Path) -> std::io::Result<()> {
    cleanup::remove_dir_all_rootless_safe(path)
}
