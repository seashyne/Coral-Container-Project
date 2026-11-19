//! Container security hardening: Linux Capabilities drop and Seccomp-BPF syscall filtering.

use std::io;
use tracing::debug;

use crate::RuntimeError;

/// OCI / Docker default safe capability bit numbers (Linux `<linux/capability.h>`).
const DEFAULT_KEEP_CAPS: &[u32] = &[
    0,  // CAP_CHOWN
    1,  // CAP_DAC_OVERRIDE
    3,  // CAP_FOWNER
    4,  // CAP_FSETID
    5,  // CAP_KILL
    6,  // CAP_SETGID
    7,  // CAP_SETUID
    8,  // CAP_SETPCAP
    10, // CAP_NET_BIND_SERVICE
    13, // CAP_NET_RAW
    18, // CAP_SYS_CHROOT
    27, // CAP_MKNOD
    29, // CAP_AUDIT_WRITE
    31, // CAP_SETFCAP
];

const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;
const MAX_LINUX_CAP: u32 = 41;

#[repr(C)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Drops dangerous Linux capabilities from bounding, effective, permitted, and inheritable sets,
/// retaining only the standard safe OCI/Docker container capability mask.
pub fn drop_capabilities() -> Result<(), RuntimeError> {
    let keep_mask: u64 = DEFAULT_KEEP_CAPS
        .iter()
        .fold(0u64, |acc, &cap| acc | (1u64 << cap));

    // 1. Drop disallowed capabilities from the bounding set (best-effort per cap for kernel compat).
    for cap in 0..=MAX_LINUX_CAP {
        if (keep_mask & (1u64 << cap)) == 0 {
            let _ = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
        }
    }

    // 2. Intersect current permitted/effective caps with the allowed mask via capget + capset.
    let mut hdr = CapUserHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [CapUserData::default(); 2];
    let rc = unsafe {
        libc::syscall(
            libc::SYS_capget,
            &mut hdr as *mut CapUserHeader,
            data.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(RuntimeError::sys("capget", io::Error::last_os_error()));
    }

    let keep_lo = (keep_mask & 0xffff_ffff) as u32;
    let keep_hi = (keep_mask >> 32) as u32;

    data[0].permitted &= keep_lo;
    data[0].effective &= keep_lo;
    data[0].inheritable = 0;

    data[1].permitted &= keep_hi;
    data[1].effective &= keep_hi;
    data[1].inheritable = 0;

    let rc = unsafe {
        libc::syscall(
            libc::SYS_capset,
            &hdr as *const CapUserHeader,
            data.as_ptr(),
        )
    };
    if rc != 0 {
        return Err(RuntimeError::sys("capset", io::Error::last_os_error()));
    }

    debug!("dropped container capabilities to OCI default mask");
    Ok(())
}

// --- Pure-Rust Classic BPF Seccomp Filter ---

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

/// Offset of `nr` in `struct seccomp_data`.
const OFFSET_NR: u32 = 0;
/// Offset of `arch` in `struct seccomp_data`.
const OFFSET_ARCH: u32 = 4;

#[cfg(target_arch = "x86_64")]
const EXPECTED_AUDIT_ARCH: u32 = 0xc000_003e; // AUDIT_ARCH_X86_64

#[cfg(target_arch = "aarch64")]
const EXPECTED_AUDIT_ARCH: u32 = 0xc000_00b7; // AUDIT_ARCH_AARCH64

#[repr(C)]
#[derive(Clone, Copy)]
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

fn denied_syscalls() -> &'static [i64] {
    #[cfg(target_arch = "x86_64")]
    {
        &[
            libc::SYS_kexec_load,
            libc::SYS_kexec_file_load,
            libc::SYS_init_module,
            libc::SYS_finit_module,
            libc::SYS_delete_module,
            libc::SYS_reboot,
            libc::SYS_swapon,
            libc::SYS_swapoff,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_userfaultfd,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_io_uring_setup,
            libc::SYS_open_by_handle_at,
            libc::SYS_acct,
            libc::SYS_settimeofday,
            libc::SYS_clock_settime,
            libc::SYS_sethostname,
            libc::SYS_setdomainname,
        ]
    }
    #[cfg(target_arch = "aarch64")]
    {
        &[
            libc::SYS_kexec_load,
            libc::SYS_kexec_file_load,
            libc::SYS_init_module,
            libc::SYS_finit_module,
            libc::SYS_delete_module,
            libc::SYS_reboot,
            libc::SYS_swapon,
            libc::SYS_swapoff,
            libc::SYS_mount,
            libc::SYS_umount2,
            libc::SYS_pivot_root,
            libc::SYS_ptrace,
            libc::SYS_process_vm_readv,
            libc::SYS_process_vm_writev,
            libc::SYS_bpf,
            libc::SYS_perf_event_open,
            libc::SYS_userfaultfd,
            libc::SYS_keyctl,
            libc::SYS_add_key,
            libc::SYS_request_key,
            libc::SYS_io_uring_setup,
            libc::SYS_open_by_handle_at,
            libc::SYS_acct,
            libc::SYS_settimeofday,
            libc::SYS_clock_settime,
            libc::SYS_sethostname,
            libc::SYS_setdomainname,
        ]
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        &[]
    }
}

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
fn build_default_seccomp_bpf() -> Vec<SockFilter> {
    let blocked = denied_syscalls();
    let mut prog = Vec::with_capacity(blocked.len() + 6);

    // 1. Validate architecture to prevent 32-bit/compat syscall number confusion.
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

    // 3. Jump to DENY instruction (last element) if syscall matches any denied number.
    let total_checks = blocked.len();
    for (idx, &nr) in blocked.iter().enumerate() {
        let remaining_checks = total_checks - idx - 1;
        // Skip remaining checks + the ALLOW instruction to reach DENY.
        let jump_to_deny = (remaining_checks + 1) as u8;
        prog.push(SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: jump_to_deny,
            jf: 0,
            k: nr as u32,
        });
    }

    // 4. Default ALLOW.
    prog.push(SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ALLOW,
    });

    // 5. DENY with EPERM (graceful error matching Docker's default seccomp profile).
    prog.push(SockFilter {
        code: BPF_RET | BPF_K,
        jt: 0,
        jf: 0,
        k: SECCOMP_RET_ERRNO | (libc::EPERM as u32 & 0x0000_ffff),
    });

    prog
}

/// Applies Coral's default Seccomp-BPF syscall filter right before `execve`.
/// Caller must ensure `PR_SET_NO_NEW_PRIVS` is enabled first.
pub fn apply_default_seccomp() -> Result<(), RuntimeError> {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    {
        let bpf = build_default_seccomp_bpf();
        let fprog = SockFprog {
            len: bpf.len() as u16,
            filter: bpf.as_ptr(),
        };
        let rc = unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                &fprog as *const SockFprog,
            )
        };
        if rc != 0 {
            return Err(RuntimeError::sys(
                "prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)",
                io::Error::last_os_error(),
            ));
        }
        debug!(rules = denied_syscalls().len(), "applied default seccomp-bpf filter");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_valid_seccomp_bpf_program() {
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            let prog = build_default_seccomp_bpf();
            assert!(prog.len() > 10);
            assert_eq!(prog[prog.len() - 2].k, SECCOMP_RET_ALLOW);
            assert_eq!(
                prog[prog.len() - 1].k,
                SECCOMP_RET_ERRNO | (libc::EPERM as u32)
            );
        }
    }
}
