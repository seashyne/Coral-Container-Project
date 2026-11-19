//! Linux `newc` (SVR4 `070701`) gzipped initramfs builder and Linux x86-64 bzImage
//! boot-header validator for booting container rootfs trees inside a KVM MicroVM.

use anyhow::{Context, Result, bail};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::fs;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Parsed Linux x86-64 bzImage header (`setup_header` magic `"HdrS"` at `0x202`).
#[derive(Debug, Clone)]
pub struct BzImageInfo {
    pub protocol_major: u8,
    pub protocol_minor: u8,
    pub setup_sects: u8,
    pub total_size: usize,
}

/// Validates a Linux x86-64 bzImage file on disk.
pub fn inspect_bzimage(kernel_path: &Path) -> Result<BzImageInfo> {
    let bytes = fs::read(kernel_path)
        .with_context(|| format!("read guest kernel '{}'", kernel_path.display()))?;
    if bytes.len() < 0x250 {
        bail!("kernel image '{}' is too small to be a Linux bzImage", kernel_path.display());
    }
    if &bytes[0x202..0x206] != b"HdrS" {
        bail!("kernel image '{}' missing Linux x86-64 'HdrS' boot signature", kernel_path.display());
    }
    let version = u16::from_le_bytes([bytes[0x206], bytes[0x207]]);
    let setup_sects = if bytes[0x1F1] == 0 { 4 } else { bytes[0x1F1] };
    Ok(BzImageInfo {
        protocol_major: (version >> 8) as u8,
        protocol_minor: (version & 0xFF) as u8,
        setup_sects,
        total_size: bytes.len(),
    })
}

/// Packs `rootfs_dir` plus an executable `/coral-init` launcher script into a gzipped
/// Linux `newc` CPIO initramfs image ready to boot via KVM / QEMU / Firecracker.
pub fn build_initramfs_cpio_gz(
    rootfs_dir: &Path,
    cmd: &[String],
    out_path: &Path,
) -> Result<u64> {
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let out_file = fs::File::create(out_path)
        .with_context(|| format!("create initramfs '{}'", out_path.display()))?;
    let mut gz = GzEncoder::new(out_file, Compression::fast());
    let mut ino = 1u32;

    // Emit `/coral-init` PID 1 launcher script inside the MicroVM guest kernel.
    let quoted_cmd = cmd
        .iter()
        .map(|s| format!("'{}'", s.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    let init_script = format!(
        "#!/bin/sh\n\
         mount -t proc proc /proc 2>/dev/null || true\n\
         mount -t sysfs sysfs /sys 2>/dev/null || true\n\
         mount -t devtmpfs devtmpfs /dev 2>/dev/null || true\n\
         {}\n\
         EC=$?\n\
         echo \"__CORAL_VMM_EXIT__=$EC\"\n\
         poweroff -f 2>/dev/null || reboot -f 2>/dev/null || exit $EC\n",
        if quoted_cmd.is_empty() { "/bin/sh".into() } else { quoted_cmd }
    );
    write_cpio_entry(&mut gz, "coral-init", 0o100755, ino, init_script.as_bytes())?;
    ino += 1;

    append_dir_to_cpio(&mut gz, rootfs_dir, Path::new(""), &mut ino)?;

    // Standard `newc` trailer entry.
    write_cpio_entry(&mut gz, "TRAILER!!!", 0, 0, &[])?;
    let file = gz.finish()?;
    Ok(file.metadata()?.len())
}

fn append_dir_to_cpio<W: Write>(
    writer: &mut W,
    root: &Path,
    rel: &Path,
    ino: &mut u32,
) -> Result<()> {
    let full = root.join(rel);
    for entry in fs::read_dir(&full)? {
        let entry = entry?;
        let name = entry.file_name();
        let child_rel = rel.join(&name);
        let child_full = entry.path();
        let meta = fs::symlink_metadata(&child_full)?;
        let cpio_name = child_rel.to_string_lossy().replace('\\', "/");
        if cpio_name == "coral-init" {
            continue;
        }
        let mode = meta.mode();
        *ino += 1;

        if meta.file_type().is_symlink() {
            let target = fs::read_link(&child_full)?;
            write_cpio_entry(
                writer,
                &cpio_name,
                0o120777,
                *ino,
                target.to_string_lossy().as_bytes(),
            )?;
        } else if meta.is_dir() {
            write_cpio_entry(writer, &cpio_name, (mode & 0o7777) | 0o040000, *ino, &[])?;
            append_dir_to_cpio(writer, root, &child_rel, ino)?;
        } else if meta.is_file() {
            let data = fs::read(&child_full)?;
            write_cpio_entry(writer, &cpio_name, (mode & 0o7777) | 0o100000, *ino, &data)?;
        }
    }
    Ok(())
}

fn write_cpio_entry<W: Write>(
    writer: &mut W,
    name: &str,
    mode: u32,
    ino: u32,
    data: &[u8],
) -> Result<()> {
    let name_bytes = name.as_bytes();
    let namesize = name_bytes.len() + 1; // include NUL terminator
    let filesize = data.len();
    let nlink = if (mode & 0o170000) == 0o040000 { 2 } else { 1 };

    let header = format!(
        "070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}",
        ino,
        mode,
        0u32, // uid
        0u32, // gid
        nlink,
        0u32, // mtime
        filesize,
        0u32, // devmajor
        0u32, // devminor
        0u32, // rdevmajor
        0u32, // rdevminor
        namesize,
        0u32, // check
    );
    writer.write_all(header.as_bytes())?;
    writer.write_all(name_bytes)?;
    writer.write_all(&[0u8])?;
    let header_and_name = 110 + namesize;
    let pad1 = (4 - (header_and_name % 4)) % 4;
    if pad1 > 0 {
        writer.write_all(&[0u8; 4][..pad1])?;
    }
    if !data.is_empty() {
        writer.write_all(data)?;
        let pad2 = (4 - (filesize % 4)) % 4;
        if pad2 > 0 {
            writer.write_all(&[0u8; 4][..pad2])?;
        }
    }
    Ok(())
}
