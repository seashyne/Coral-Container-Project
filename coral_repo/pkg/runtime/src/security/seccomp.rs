//! Pure-Rust Classic BPF Seccomp-BPF compiler supporting:
//! - Default-Deny OCI Allowlist (default for all non-privileged containers)
//! - Custom OCI Seccomp JSON profiles (`--security-opt seccomp=/path/to/profile.json`)
//! - `unconfined` opt-out (`--security-opt seccomp=unconfined`)

use super::seccomp_allowlist::{default_allowed_syscalls, resolve_syscall_name};
use crate::RuntimeError;
use serde::Deserialize;
use std::io;
use std::path::{Path, PathBuf};
use tracing::debug;

const BPF_LD: u16 = 0x00;
const BPF_W: u16 = 0x00;
const BPF_ABS: u16 = 0x20;
const BPF_JMP: u16 = 0x05;
const BPF_JEQ: u16 = 0x10;
const BPF_K: u16 = 0x00;
const BPF_RET: u16 = 0x06;

const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;

const OFFSET_NR: u32 = 0;
const OFFSET_ARCH: u32 = 4;

#[cfg(target_arch = "x86_64")]
const EXPECTED_AUDIT_ARCH: u32 = 0xc000_003e;

#[cfg(target_arch = "aarch64")]
const EXPECTED_AUDIT_ARCH: u32 = 0xc000_00b7;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SeccompMode {
    /// Strict Default-Deny OCI Allowlist (recommended default).
    #[default]
    DefaultAllowlist,
    /// Custom OCI Seccomp JSON profile path on the host.
    CustomProfile(PathBuf),
    /// Pre-loaded OCI Seccomp JSON content (read before `pivot_root`).
    CustomProfileJson(String),
    /// Disabled (`--security-opt seccomp=unconfined`).
    Unconfined,
}

impl SeccompMode {
    /// Reads any host-side JSON file before `pivot_root` so container init can compile it after pivot.
    pub fn preload_before_pivot(self) -> Result<Self, RuntimeError> {
        match self {
            Self::CustomProfile(path) => {
                let raw = std::fs::read_to_string(&path).map_err(|e| {
                    RuntimeError::sys(format!("read seccomp profile '{}'", path.display()), e)
                })?;
                Ok(Self::CustomProfileJson(raw))
            }
            other => Ok(other),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OciSeccompProfile {
    default_action: String,
    #[serde(default)]
    syscalls: Vec<OciSyscallRule>,
}

#[derive(Debug, Deserialize)]
struct OciSyscallRule {
    names: Vec<String>,
    action: String,
}

fn map_oci_action(action: &str) -> u32 {
    match action.trim() {
        "SCMP_ACT_ALLOW" => SECCOMP_RET_ALLOW,
        "SCMP_ACT_KILL" | "SCMP_ACT_KILL_PROCESS" | "SCMP_ACT_KILL_THREAD" => {
            SECCOMP_RET_KILL_PROCESS
        }
        _ => SECCOMP_RET_ERRNO | (libc::EPERM as u32),
    }
}

/// Builds a Classic BPF seccomp program given a default return action and `(syscall_nr, action)`
/// rules. Note that `SockFilter.jt`/`jf` are `u8`, so we structure jumps using exact relative
/// offsets (`jt = 0, jf = 1` followed by an inline `RET`) which supports arbitrarily many rules
/// without overflowing `u8` jump distances!
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn compile_seccomp_bpf(default_ret: u32, rules: &[(i64, u32)]) -> Vec<SockFilter> {
    let mut prog = Vec::with_capacity(rules.len() * 2 + 5);

    // 1. Validate AUDIT_ARCH to prevent 32-bit/compat syscall number confusion.
    prog.push(SockFilter {
        code: BPF_LD | BPF_W | BPF_ABS,
        jt: 0,
        jf: 0,
        k: OFFSET_ARCH,
    });
    prog.push(SockFilter {
        code: BPF_JMP | BPF_JEQ | BPF_K,
        jt: 1,
        jf: 0,
        k: EXPECTED_AUDIT_ARCH,
    });
    prog.push(SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_KILL_PROCESS,
    });

    // 2. Load syscall number (`seccomp_data.nr`).
    prog.push(SockFilter {
        code: BPF_LD | BPF_W | BPF_ABS,
        jt: 0,
        jf: 0,
        k: OFFSET_NR,
    });

    // 3. Match each rule with a 2-instruction sequence (JEQ -> inline RET, else skip 1).
    for &(nr, action) in rules {
        prog.push(SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 0,
            jf: 1,
            k: nr as u32,
        });
        prog.push(SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: action,
        });
    }

    // 4. Default action for any unmatched syscall.
    prog.push(SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: default_ret,
    });

    prog
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn build_default_allowlist_bpf() -> Vec<SockFilter> {
    let rules: Vec<(i64, u32)> = default_allowed_syscalls()
        .into_iter()
        .map(|nr| (nr, SECCOMP_RET_ALLOW))
        .collect();
    compile_seccomp_bpf(SECCOMP_RET_ERRNO | (libc::EPERM as u32), &rules)
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn build_custom_profile_bpf_from_str(raw: &str, label: &str) -> Result<Vec<SockFilter>, RuntimeError> {
    let profile: OciSeccompProfile = serde_json::from_str(raw).map_err(|e| {
        RuntimeError::InitFailed(format!("parse OCI seccomp profile '{label}': {e}"))
    })?;
    let default_ret = map_oci_action(&profile.default_action);
    let mut rules = Vec::new();
    for rule in &profile.syscalls {
        let act = map_oci_action(&rule.action);
        for name in &rule.names {
            if let Some(nr) = resolve_syscall_name(name) {
                rules.push((nr, act));
            }
        }
    }
    Ok(compile_seccomp_bpf(default_ret, &rules))
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn build_custom_profile_bpf(path: &Path) -> Result<Vec<SockFilter>, RuntimeError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| RuntimeError::sys(format!("read seccomp profile '{}'", path.display()), e))?;
    build_custom_profile_bpf_from_str(&raw, &path.display().to_string())
}

/// Applies the requested Seccomp-BPF policy. Caller must enable `PR_SET_NO_NEW_PRIVS` first.
pub fn apply_seccomp(mode: &SeccompMode) -> Result<(), RuntimeError> {
    if matches!(mode, SeccompMode::Unconfined) {
        debug!("seccomp unconfined requested; skipping BPF installation");
        return Ok(());
    }

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let bpf = match mode {
            SeccompMode::DefaultAllowlist => build_default_allowlist_bpf(),
            SeccompMode::CustomProfile(path) => build_custom_profile_bpf(path)?,
            SeccompMode::CustomProfileJson(raw) => {
                build_custom_profile_bpf_from_str(raw, "<preloaded>")?
            }
            SeccompMode::Unconfined => unreachable!(),
        };
        if bpf.len() > 4096 {
            return Err(RuntimeError::InitFailed(format!(
                "seccomp BPF program exceeds kernel 4096-instruction cap ({})",
                bpf.len()
            )));
        }
        let fprog = SockFprog {
            len: bpf.len() as u16,
            filter: bpf.as_ptr(),
        };
        let rc = unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &fprog as *const SockFprog,
                0,
                0,
            )
        };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "prctl(PR_SET_SECCOMP)",
                io::Error::last_os_error(),
            ));
        }
        debug!(
            instructions = bpf.len(),
            ?mode,
            "applied default-deny seccomp-bpf filter"
        );
    }
    Ok(())
}

/// Convenience wrapper installing the default strict OCI allowlist.
pub fn apply_default_seccomp() -> Result<(), RuntimeError> {
    apply_seccomp(&SeccompMode::DefaultAllowlist)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure-Rust cBPF interpreter to verify seccomp decisions deterministically in unit tests.
    fn eval_bpf(prog: &[SockFilter], arch: u32, nr: u32) -> u32 {
        let mut a: u32 = 0;
        let mut pc: usize = 0;
        while pc < prog.len() {
            let ins = prog[pc];
            match ins.code {
                c if c == (BPF_LD | BPF_W | BPF_ABS) => {
                    a = if ins.k == OFFSET_ARCH { arch } else { nr };
                    pc += 1;
                }
                c if c == (BPF_JMP | BPF_JEQ | BPF_K) => {
                    pc += 1 + if a == ins.k { ins.jt as usize } else { ins.jf as usize };
                }
                c if c == (BPF_RET | BPF_K) => return ins.k,
                other => panic!("unexpected BPF opcode {other:#x}"),
            }
        }
        panic!("BPF program terminated without RET");
    }

    #[test]
    fn default_allowlist_allows_safe_syscalls_and_blocks_dangerous_and_unknown() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let prog = build_default_allowlist_bpf();
            assert!(prog.len() < 4096);
            // Wrong architecture is killed immediately.
            assert_eq!(eval_bpf(&prog, 0x4000_0003, libc::SYS_read as u32), SECCOMP_RET_KILL_PROCESS);
            // Standard safe syscalls allowed.
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_read as u32), SECCOMP_RET_ALLOW);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_execve as u32), SECCOMP_RET_ALLOW);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_socket as u32), SECCOMP_RET_ALLOW);
            // Dangerous syscalls denied with EPERM.
            let deny = SECCOMP_RET_ERRNO | (libc::EPERM as u32);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_bpf as u32), deny);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_mount as u32), deny);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_ptrace as u32), deny);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_io_uring_setup as u32), deny);
            // Future / unknown syscall number denied by default!
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, 9999), deny);
        }
    }

    #[test]
    fn parses_custom_oci_seccomp_json_profile() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let tmp = std::env::temp_dir().join("coral-seccomp-profile-test.json");
            let json = r#"{
              "defaultAction": "SCMP_ACT_ERRNO",
              "syscalls": [
                { "names": ["read", "write", "exit_group"], "action": "SCMP_ACT_ALLOW" },
                { "names": ["ptrace"], "action": "SCMP_ACT_KILL_PROCESS" }
              ]
            }"#;
            std::fs::write(&tmp, json).unwrap();
            let prog = build_custom_profile_bpf(&tmp).unwrap();
            let _ = std::fs::remove_file(&tmp);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_read as u32), SECCOMP_RET_ALLOW);
            assert_eq!(eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_ptrace as u32), SECCOMP_RET_KILL_PROCESS);
            assert_eq!(
                eval_bpf(&prog, EXPECTED_AUDIT_ARCH, libc::SYS_socket as u32),
                SECCOMP_RET_ERRNO | (libc::EPERM as u32)
            );
        }
    }
}
