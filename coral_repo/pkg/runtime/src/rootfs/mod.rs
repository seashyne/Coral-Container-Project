//! Container rootfs mount namespace setup, `pivot_root`, Named Volumes, and DNS/hosts mounts.

pub mod devices;
pub mod dns;
pub mod gpu_cdi;
pub mod volumes;

pub use devices::{
    discover_gpu_mounts_and_envs, discover_gpu_mounts_and_envs_in, parse_device_mount,
};
pub use gpu_cdi::{
    discover_nvidia_driver_mounts, discover_nvidia_driver_mounts_in, is_cdi_spec,
    resolve_cdi_device, resolve_cdi_device_in,
};
pub use dns::{append_host_entry_in_place, host_dns_mounts, prepare_dns_and_hosts_mounts};
pub use volumes::{
    default_volumes_dir, ensure_named_volume, is_valid_volume_name, list_named_volumes,
    parse_volume_spec, parse_volume_spec_in, remove_named_volume,
};

use crate::RuntimeError;
use crate::overlay::{OverlayDirs, mount_overlay_multi};
use coral_oci::Mount;
use nix::mount::{MntFlags, MsFlags, mount, umount2};
use nix::unistd::{chdir, pivot_root};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use tracing::{debug, warn};

const ESSENTIAL_DEVICES: &[&str] = &["null", "zero", "full", "random", "urandom"];

/// Isolates the container's mount namespace, optionally mounts Copy-on-Write OverlayFS,
/// sets up OCI mounts + `/dev`, and executes `pivot_root`.
pub fn prepare_rootfs(
    rootfs: &Path,
    overlay: Option<&OverlayDirs>,
    mounts: &[Mount],
    readonly: bool,
    rootless: bool,
) -> Result<(), RuntimeError> {
    if !rootfs.is_dir() {
        return Err(RuntimeError::InitFailed(format!(
            "rootfs '{}' does not exist or is not a directory",
            rootfs.display()
        )));
    }

    // 1. Make mount propagation private so mount events never leak to the host and pivot_root succeeds.
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_REC | MsFlags::MS_PRIVATE,
        None::<&str>,
    )
    .map_err(|e| RuntimeError::nix("make root mount private", e))?;

    // 2. Mount Copy-on-Write OverlayFS inside the container's private mount namespace (auto-unmounted on exit!).
    let active_rootfs: &Path = if let Some(ovl) = overlay {
        match mount_overlay_multi(
            &ovl.lowerdirs,
            &ovl.upperdir,
            &ovl.workdir,
            &ovl.merged,
            rootless,
        ) {
            Ok(()) => {
                debug!(merged = %ovl.merged.display(), "mounted container OverlayFS CoW rootfs");
                &ovl.merged
            }
            Err(e) => {
                warn!(err = %e, "OverlayFS mount unavailable; falling back to direct rootfs");
                rootfs
            }
        }
    } else {
        rootfs
    };

    // 3. Bind-mount active rootfs onto itself so it is guaranteed to be a mount point.
    mount(
        Some(active_rootfs),
        active_rootfs,
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .map_err(|e| RuntimeError::nix("bind-mount rootfs", e))?;

    // 4. Prepare essential /dev character nodes & standard fd symlinks.
    setup_dev_nodes(active_rootfs, mounts);

    // 5. Process OCI mounts (proc, tmpfs, bind volumes, /etc/resolv.conf) before pivoting root.
    for m in mounts {
        apply_oci_mount(active_rootfs, m, rootless)?;
    }

    // Ensure /tmp exists with world-writable sticky mode even if user omitted tmpfs mount.
    let tmp_dir = active_rootfs.join("tmp");
    if tmp_dir.is_dir() {
        let _ = fs::set_permissions(&tmp_dir, fs::Permissions::from_mode(0o1777));
    }

    // 6. Switch root using the safe "pivot_root(new_root, new_root)" idiom (same pattern as runc).
    pivot_root(active_rootfs, active_rootfs).map_err(|e| RuntimeError::nix("pivot_root", e))?;

    chdir("/").map_err(|e| RuntimeError::nix("chdir to new /", e))?;

    // 7. Detach old host rootfs mounted on "/" underneath the new root.
    umount2("/", MntFlags::MNT_DETACH)
        .map_err(|e| RuntimeError::nix("umount old root after pivot_root", e))?;

    // 8. Optionally lock rootfs read-only if requested by OCI spec.
    if readonly {
        let mut flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
        if rootless {
            flags |= MsFlags::MS_NODEV | MsFlags::MS_NOSUID;
        }
        let _ = mount(None::<&str>, "/", None::<&str>, flags, None::<&str>);
    }

    Ok(())
}

fn setup_dev_nodes(rootfs: &Path, mounts: &[Mount]) {
    let dev_dir = rootfs.join("dev");
    if !dev_dir.exists() && fs::create_dir_all(&dev_dir).is_err() {
        return;
    }

    for dev in ESSENTIAL_DEVICES {
        let host_dev = Path::new("/dev").join(dev);
        let target = dev_dir.join(dev);
        if !host_dev.exists() {
            continue;
        }
        if !target.exists()
            && OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&target)
                .is_err()
        {
            continue;
        }
        let _ = mount(
            Some(&host_dev),
            &target,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        );
    }

    // Standard POSIX/OCI `/dev/{fd,stdin,stdout,stderr}` symlinks (needed by nginx /dev/stderr, bash, python).
    for (link_name, dest) in [
        ("fd", "/proc/self/fd"),
        ("stdin", "/proc/self/fd/0"),
        ("stdout", "/proc/self/fd/1"),
        ("stderr", "/proc/self/fd/2"),
    ] {
        let link_path = dev_dir.join(link_name);
        if fs::symlink_metadata(&link_path).is_err() {
            let _ = symlink(dest, &link_path);
        }
    }

    // Best-effort `/dev/ptmx` (bind-mount host ptmx if possible so PTY allocators inside container succeed).
    let ptmx_target = dev_dir.join("ptmx");
    let host_ptmx = Path::new("/dev/ptmx");
    if host_ptmx.exists() {
        if !ptmx_target.exists() {
            let _ = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&ptmx_target);
        }
        let _ = mount(
            Some(host_ptmx),
            &ptmx_target,
            None::<&str>,
            MsFlags::MS_BIND,
            None::<&str>,
        );
    }

    // Mount tmpfs on `/dev/shm` unless explicitly provided by OCI mounts (required by Postgres / Python multiprocessing).
    let has_shm_mount = mounts.iter().any(|m| m.destination == "/dev/shm");
    if !has_shm_mount {
        let shm_dir = dev_dir.join("shm");
        if fs::create_dir_all(&shm_dir).is_ok() {
            let _ = mount(
                Some("tmpfs"),
                &shm_dir,
                Some("tmpfs"),
                MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC,
                Some("mode=1777,size=67108864"),
            );
            let _ = fs::set_permissions(&shm_dir, fs::Permissions::from_mode(0o1777));
        }
    }
}

fn apply_oci_mount(rootfs: &Path, m: &Mount, rootless: bool) -> Result<(), RuntimeError> {
    let target = safe_join_rootfs(rootfs, &m.destination)?;
    let fstype = m.typ.as_deref().unwrap_or("");
    let (flags, data) = parse_mount_options(&m.options);

    match fstype {
        "proc" => {
            fs::create_dir_all(&target)
                .map_err(|e| RuntimeError::sys("create /proc mountpoint", e))?;
            let res = mount(
                Some("proc"),
                &target,
                Some("proc"),
                flags | MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC | MsFlags::MS_NODEV,
                None::<&str>,
            );
            if let Err(e) = res {
                if rootless {
                    debug!(err = %e, "skipping proc mount in restricted rootless namespace");
                } else {
                    return Err(RuntimeError::nix("mount proc", e));
                }
            }
        }
        "tmpfs" => {
            fs::create_dir_all(&target)
                .map_err(|e| RuntimeError::sys("create tmpfs mountpoint", e))?;
            let data_opt = if data.is_empty() {
                None
            } else {
                Some(data.as_str())
            };
            mount(Some("tmpfs"), &target, Some("tmpfs"), flags, data_opt)
                .map_err(|e| RuntimeError::nix(format!("mount tmpfs at {}", m.destination), e))?;
        }
        "bind" | "none" if flags.contains(MsFlags::MS_BIND) => {
            let src = m.source.as_deref().ok_or_else(|| {
                RuntimeError::InitFailed(format!(
                    "bind mount '{}' is missing source",
                    m.destination
                ))
            })?;
            let src_path = Path::new(src);
            if src_path.is_dir() {
                fs::create_dir_all(&target)
                    .map_err(|e| RuntimeError::sys("create bind dir mountpoint", e))?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| RuntimeError::sys("create bind parent dir", e))?;
                }
                if let Ok(meta) = fs::symlink_metadata(&target)
                    && meta.file_type().is_symlink()
                {
                    let _ = fs::remove_file(&target);
                }
                if !target.exists() {
                    OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(false)
                        .open(&target)
                        .map_err(|e| RuntimeError::sys("create bind file mountpoint", e))?;
                }
            }
            let wants_ro = flags.contains(MsFlags::MS_RDONLY);
            let bind_flags = flags & !MsFlags::MS_RDONLY;
            mount(
                Some(src_path),
                &target,
                None::<&str>,
                bind_flags,
                None::<&str>,
            )
            .map_err(|e| RuntimeError::nix(format!("bind mount {}", m.destination), e))?;

            if wants_ro {
                let mut ro_flags = MsFlags::MS_BIND | MsFlags::MS_REMOUNT | MsFlags::MS_RDONLY;
                if mount(None::<&str>, &target, None::<&str>, ro_flags, None::<&str>).is_err() {
                    ro_flags |= MsFlags::MS_NOSUID | MsFlags::MS_NODEV | MsFlags::MS_NOEXEC;
                    if let Err(e) = mount(None::<&str>, &target, None::<&str>, ro_flags, None::<&str>) {
                        if !rootless {
                            return Err(RuntimeError::nix(
                                format!("remount read-only {}", m.destination),
                                e,
                            ));
                        }
                        debug!(dest = %m.destination, err = %e, "best-effort ro remount skipped in rootless ns");
                    }
                }
            }
        }
        "sysfs" => {
            if !rootless {
                fs::create_dir_all(&target)
                    .map_err(|e| RuntimeError::sys("create sysfs mountpoint", e))?;
                let _ = mount(Some("sysfs"), &target, Some("sysfs"), flags, None::<&str>);
            }
        }
        _ => {
            debug!(
                dest = %m.destination,
                fstype,
                "skipping unsupported or optional mount type in v1"
            );
        }
    }

    Ok(())
}

/// Safely joins an in-container mount destination under `rootfs`, preventing `..` escapes.
pub fn safe_join_rootfs(rootfs: &Path, destination: &str) -> Result<PathBuf, RuntimeError> {
    let mut result = rootfs.to_path_buf();
    for comp in Path::new(destination).components() {
        match comp {
            Component::RootDir | Component::CurDir => {}
            Component::Normal(part) => result.push(part),
            Component::ParentDir | Component::Prefix(_) => {
                return Err(RuntimeError::InitFailed(format!(
                    "unsafe mount destination '{destination}'"
                )));
            }
        }
    }
    Ok(result)
}

fn parse_mount_options(options: &[String]) -> (MsFlags, String) {
    let mut flags = MsFlags::empty();
    let mut data_parts = Vec::new();

    for opt in options {
        match opt.as_str() {
            "ro" => flags |= MsFlags::MS_RDONLY,
            "rw" => flags.remove(MsFlags::MS_RDONLY),
            "nosuid" => flags |= MsFlags::MS_NOSUID,
            "nodev" => flags |= MsFlags::MS_NODEV,
            "noexec" => flags |= MsFlags::MS_NOEXEC,
            "bind" => flags |= MsFlags::MS_BIND,
            "rbind" => flags |= MsFlags::MS_BIND | MsFlags::MS_REC,
            "sync" => flags |= MsFlags::MS_SYNCHRONOUS,
            "relatime" => flags |= MsFlags::MS_RELATIME,
            "strictatime" => flags |= MsFlags::MS_STRICTATIME,
            "noatime" => flags |= MsFlags::MS_NOATIME,
            "remount" => flags |= MsFlags::MS_REMOUNT,
            other => data_parts.push(other),
        }
    }

    (flags, data_parts.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prevents_path_traversal_in_mount_destinations() {
        let rootfs = Path::new("/var/lib/coral/rootfs");
        assert_eq!(
            safe_join_rootfs(rootfs, "/proc").unwrap(),
            PathBuf::from("/var/lib/coral/rootfs/proc")
        );
        assert!(safe_join_rootfs(rootfs, "/../../etc/passwd").is_err());
    }

    #[test]
    fn parses_standard_mount_options() {
        let opts = vec![
            "nosuid".into(),
            "nodev".into(),
            "mode=1777".into(),
            "size=64m".into(),
        ];
        let (flags, data) = parse_mount_options(&opts);
        assert!(flags.contains(MsFlags::MS_NOSUID | MsFlags::MS_NODEV));
        assert_eq!(data, "mode=1777,size=64m");
    }
}
