//! Linux Capabilities parsing (`--cap-drop` / `--cap-add`) and 2-phase kernel enforcement.

use crate::RuntimeError;
use std::io;
use tracing::debug;

/// OCI / Docker default safe capability bit numbers (Linux `<linux/capability.h>`).
pub const DEFAULT_KEEP_CAPS: &[u32] = &[
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
const ALL_CAPS_MASK: u64 = (1u64 << (MAX_LINUX_CAP + 1)) - 1;

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

/// Resolves a Linux capability name (with or without `CAP_` prefix, case-insensitive) to its bit.
pub fn parse_cap_bit(raw: &str) -> Option<u32> {
    let norm = raw.trim().to_ascii_uppercase();
    let name = norm.strip_prefix("CAP_").unwrap_or(&norm);
    Some(match name {
        "CHOWN" => 0,
        "DAC_OVERRIDE" => 1,
        "DAC_READ_SEARCH" => 2,
        "FOWNER" => 3,
        "FSETID" => 4,
        "KILL" => 5,
        "SETGID" => 6,
        "SETUID" => 7,
        "SETPCAP" => 8,
        "LINUX_IMMUTABLE" => 9,
        "NET_BIND_SERVICE" => 10,
        "NET_BROADCAST" => 11,
        "NET_ADMIN" => 12,
        "NET_RAW" => 13,
        "IPC_LOCK" => 14,
        "IPC_OWNER" => 15,
        "SYS_MODULE" => 16,
        "SYS_RAWIO" => 17,
        "SYS_CHROOT" => 18,
        "SYS_PTRACE" => 19,
        "SYS_PACCT" => 20,
        "SYS_ADMIN" => 21,
        "SYS_BOOT" => 22,
        "SYS_NICE" => 23,
        "SYS_RESOURCE" => 24,
        "SYS_TIME" => 25,
        "SYS_TTY_CONFIG" => 26,
        "MKNOD" => 27,
        "LEASE" => 28,
        "AUDIT_WRITE" => 29,
        "AUDIT_CONTROL" => 30,
        "SETFCAP" => 31,
        "MAC_OVERRIDE" => 32,
        "MAC_ADMIN" => 33,
        "SYSLOG" => 34,
        "WAKE_ALARM" => 35,
        "BLOCK_SUSPEND" => 36,
        "AUDIT_READ" => 37,
        "PERFMON" => 38,
        "BPF" => 39,
        "CHECKPOINT_RESTORE" => 40,
        _ => return None,
    })
}

/// Computes the effective capability bitmask starting from OCI defaults, removing `cap_drop`
/// and adding `cap_add`. Supports `"ALL"` in either list (matching Docker/Podman semantics).
pub fn compute_cap_mask(cap_drop: &[String], cap_add: &[String]) -> Result<u64, RuntimeError> {
    let mut mask: u64 = DEFAULT_KEEP_CAPS
        .iter()
        .fold(0u64, |acc, &cap| acc | (1u64 << cap));

    for item in cap_drop {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("ALL") {
            mask = 0;
            continue;
        }
        let bit = parse_cap_bit(trimmed).ok_or_else(|| {
            RuntimeError::InitFailed(format!("unknown Linux capability in --cap-drop: '{trimmed}'"))
        })?;
        mask &= !(1u64 << bit);
    }

    for item in cap_add {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.eq_ignore_ascii_case("ALL") {
            mask = ALL_CAPS_MASK;
            continue;
        }
        let bit = parse_cap_bit(trimmed).ok_or_else(|| {
            RuntimeError::InitFailed(format!("unknown Linux capability in --cap-add: '{trimmed}'"))
        })?;
        mask |= 1u64 << bit;
    }

    Ok(mask)
}

/// Phase 1: Drops all capabilities outside `keep_mask` from the kernel bounding set while
/// the init process still holds `CAP_SETPCAP` (before `setuid`).
pub fn drop_bounding_set(keep_mask: u64) {
    for cap in 0..=MAX_LINUX_CAP {
        if (keep_mask & (1u64 << cap)) == 0 {
            let _ = unsafe { libc::prctl(libc::PR_CAPBSET_DROP, cap as libc::c_ulong, 0, 0, 0) };
        }
    }
}

/// Phase 2: Intersects effective/permitted capabilities with `keep_mask` and clears inheritable.
pub fn apply_capset(keep_mask: u64) -> Result<(), RuntimeError> {
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
    debug!(keep_mask = format_args!("{keep_mask:#x}"), "applied container capability mask");
    Ok(())
}

/// Drops both bounding set and effective/permitted/inheritable sets to the computed mask.
pub fn drop_capabilities_with(cap_drop: &[String], cap_add: &[String]) -> Result<(), RuntimeError> {
    let mask = compute_cap_mask(cap_drop, cap_add)?;
    drop_bounding_set(mask);
    apply_capset(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_drop_all_and_cap_add_net_bind_service_leaves_single_bit() {
        let mask = compute_cap_mask(
            &["ALL".to_string()],
            &["cap_net_bind_service".to_string()],
        )
        .unwrap();
        assert_eq!(mask, 1u64 << 10);
    }

    #[test]
    fn cap_drop_all_leaves_zero_capabilities() {
        let mask = compute_cap_mask(&["all".to_string()], &[]).unwrap();
        assert_eq!(mask, 0);
    }

    #[test]
    fn unknown_capability_returns_error() {
        assert!(compute_cap_mask(&["NOT_A_REAL_CAP".to_string()], &[]).is_err());
    }
}
