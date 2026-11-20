//! Pure-Rust VirtIO-MMIO v2 Block Device (`virtio-blk`) for persistent MicroVM storage.
//!
//! Backs Linux `/dev/vda` (`virtio_mmio.device=4K@0xd0001000:5`) with a host file using
//! zero external C dependencies and strict bounds checking over guest descriptor rings.

use crate::virtio_mmio::{VIRTIO_DEV_BLK, VIRTIO_MMIO_INT_VRING, VirtioMmioTransport};
use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const VIRTIO_BLK_MMIO_BASE: u64 = 0xD000_1000;
pub const VIRTIO_BLK_MMIO_SIZE: u64 = 0x1000;
pub const VIRTIO_BLK_IRQ: u32 = 5;

const VIRTIO_BLK_F_RO: u32 = 1 << 5;
const VIRTIO_BLK_F_BLK_SIZE: u32 = 1 << 6;
const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;
const VIRTIO_BLK_T_GET_ID: u32 = 8;

const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

const SECTOR_SIZE: u64 = 512;

/// Host-backed VirtIO Block device exposed at MMIO `0xD000_1000` (IRQ 5).
pub struct VirtioBlkDevice {
    pub transport: VirtioMmioTransport,
    file: File,
    capacity_sectors: u64,
    read_only: bool,
    pub reads: u64,
    pub writes: u64,
    pub flushes: u64,
}

impl VirtioBlkDevice {
    pub fn open(path: &Path, read_only: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(!read_only)
            .open(path)
            .with_context(|| format!("open VirtIO-Blk disk '{}'", path.display()))?;
        let file_bytes = file.metadata()?.len();
        if file_bytes < SECTOR_SIZE {
            bail!(
                "VirtIO-Blk disk '{}' must be at least {} bytes (got {})",
                path.display(),
                SECTOR_SIZE,
                file_bytes
            );
        }
        let capacity_sectors = file_bytes / SECTOR_SIZE;

        let mut cfg = vec![0u8; 64];
        cfg[0..8].copy_from_slice(&capacity_sectors.to_le_bytes());
        cfg[20..24].copy_from_slice(&(SECTOR_SIZE as u32).to_le_bytes());

        let mut features_lo = VIRTIO_BLK_F_BLK_SIZE | VIRTIO_BLK_F_FLUSH;
        if read_only {
            features_lo |= VIRTIO_BLK_F_RO;
        }

        Ok(Self {
            transport: VirtioMmioTransport::new(VIRTIO_DEV_BLK, features_lo, 0, 1, cfg),
            file,
            capacity_sectors,
            read_only,
            reads: 0,
            writes: 0,
            flushes: 0,
        })
    }

    pub fn read_mmio(&self, offset: u64, data: &mut [u8]) {
        self.transport.read_mmio(offset, data);
    }

    /// Handles MMIO register write and executes pending VirtIO block requests on notify.
    /// Returns `true` if a new used-ring interrupt should be pulsed on IRQ 5.
    pub fn write_mmio(&mut self, offset: u64, data: &[u8], mem: &mut [u8]) -> bool {
        let Some(qidx) = self.transport.write_mmio(offset, data) else {
            return false;
        };
        if qidx != 0 {
            return false;
        }
        self.process_queue(mem)
    }

    fn process_queue(&mut self, mem: &mut [u8]) -> bool {
        let mut raised = false;
        while let Some(chain) = self.transport.queues[0].pop_avail_chain(mem) {
            let written_len = self.handle_chain(&chain.segments, mem);
            if self.transport.queues[0].push_used(mem, chain.head, written_len) {
                self.transport.interrupt_status |= VIRTIO_MMIO_INT_VRING;
                raised = true;
            }
        }
        raised
    }

    fn handle_chain(&mut self, segs: &[crate::virtio_mmio::DescSegment], mem: &mut [u8]) -> u32 {
        if segs.len() < 2 || segs[0].write_only || segs[0].len < 16 {
            return 0;
        }
        let hdr_off = segs[0].addr as usize;
        let hdr = &mem[hdr_off..hdr_off + 16];
        let req_type = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
        let sector = u64::from_le_bytes(hdr[8..16].try_into().unwrap());

        let status_seg = segs[segs.len() - 1];
        if !status_seg.write_only || status_seg.len == 0 {
            return 0;
        }
        let data_segs = &segs[1..segs.len() - 1];

        let mut data_written: u32 = 0;
        let status = match req_type {
            VIRTIO_BLK_T_IN => {
                let mut offset = sector.saturating_mul(SECTOR_SIZE);
                let mut ok = true;
                for seg in data_segs {
                    if !seg.write_only {
                        ok = false;
                        break;
                    }
                    let byte_len = seg.len as usize;
                    if offset.saturating_add(byte_len as u64)
                        > self.capacity_sectors.saturating_mul(SECTOR_SIZE)
                    {
                        ok = false;
                        break;
                    }
                    let dst = &mut mem[seg.addr as usize..seg.addr as usize + byte_len];
                    if self.file.seek(SeekFrom::Start(offset)).is_err()
                        || self.file.read_exact(dst).is_err()
                    {
                        ok = false;
                        break;
                    }
                    offset += byte_len as u64;
                    data_written = data_written.saturating_add(seg.len);
                }
                self.reads = self.reads.saturating_add(1);
                if ok { VIRTIO_BLK_S_OK } else { VIRTIO_BLK_S_IOERR }
            }
            VIRTIO_BLK_T_OUT => {
                if self.read_only {
                    VIRTIO_BLK_S_IOERR
                } else {
                    let mut offset = sector.saturating_mul(SECTOR_SIZE);
                    let mut ok = true;
                    for seg in data_segs {
                        if seg.write_only {
                            ok = false;
                            break;
                        }
                        let byte_len = seg.len as usize;
                        if offset.saturating_add(byte_len as u64)
                            > self.capacity_sectors.saturating_mul(SECTOR_SIZE)
                        {
                            ok = false;
                            break;
                        }
                        let src = &mem[seg.addr as usize..seg.addr as usize + byte_len];
                        if self.file.seek(SeekFrom::Start(offset)).is_err()
                            || self.file.write_all(src).is_err()
                        {
                            ok = false;
                            break;
                        }
                        offset += byte_len as u64;
                    }
                    let _ = self.file.flush();
                    self.writes = self.writes.saturating_add(1);
                    if ok { VIRTIO_BLK_S_OK } else { VIRTIO_BLK_S_IOERR }
                }
            }
            VIRTIO_BLK_T_FLUSH => {
                self.flushes = self.flushes.saturating_add(1);
                if self.file.sync_data().is_ok() {
                    VIRTIO_BLK_S_OK
                } else {
                    VIRTIO_BLK_S_IOERR
                }
            }
            VIRTIO_BLK_T_GET_ID => {
                let mut id = [0u8; 20];
                let label = b"coral-virtio-vda";
                id[..label.len()].copy_from_slice(label);
                let mut pos = 0usize;
                for seg in data_segs {
                    if !seg.write_only || pos >= id.len() {
                        break;
                    }
                    let take = (seg.len as usize).min(id.len() - pos);
                    let dst = &mut mem[seg.addr as usize..seg.addr as usize + take];
                    dst.copy_from_slice(&id[pos..pos + take]);
                    pos += take;
                    data_written = data_written.saturating_add(take as u32);
                }
                VIRTIO_BLK_S_OK
            }
            _ => VIRTIO_BLK_S_UNSUPP,
        };

        mem[status_seg.addr as usize] = status;
        data_written.saturating_add(1)
    }
}

/// Creates a sparse raw disk image on the host, optionally pre-formatted as `ext4`
/// if `format_ext4` is true and `mkfs.ext4` / `mke2fs` is installed.
pub fn create_disk_image(path: &Path, size_mb: u64, format_ext4: bool) -> Result<()> {
    if size_mb == 0 {
        bail!("disk size must be >= 1 MB");
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("create disk image '{}'", path.display()))?;
    f.set_len(size_mb.saturating_mul(1024 * 1024))?;
    drop(f);

    if format_ext4 {
        let mkfs = ["/sbin/mkfs.ext4", "/usr/sbin/mkfs.ext4", "mkfs.ext4"]
            .into_iter()
            .find(|p| !p.contains('/') || Path::new(p).exists())
            .unwrap_or("mkfs.ext4");
        let status = std::process::Command::new(mkfs)
            .arg("-F")
            .arg("-q")
            .arg(path)
            .status()
            .with_context(|| format!("run {mkfs} on '{}'", path.display()))?;
        if !status.success() {
            bail!("{mkfs} failed formatting '{}'", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::virtio_mmio::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE, write_u16_le, write_u32_le};

    fn setup_queue(blk: &mut VirtioBlkDevice, mem: &mut [u8]) {
        // desc=0x0, avail=0x1000, used=0x2000, size=16
        blk.write_mmio(0x030, &0u32.to_le_bytes(), mem);
        blk.write_mmio(0x038, &16u32.to_le_bytes(), mem);
        blk.write_mmio(0x080, &0x0000u32.to_le_bytes(), mem);
        blk.write_mmio(0x090, &0x1000u32.to_le_bytes(), mem);
        blk.write_mmio(0x0A0, &0x2000u32.to_le_bytes(), mem);
        blk.write_mmio(0x044, &1u32.to_le_bytes(), mem);
    }

    #[test]
    fn virtio_blk_persists_writes_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("data.img");
        create_disk_image(&disk, 2, false).unwrap();

        let mut mem = vec![0u8; 64 * 1024];
        {
            let mut blk = VirtioBlkDevice::open(&disk, false).unwrap();
            setup_queue(&mut blk, &mut mem);

            // Header at 0x4000: OUT (1), sector 4
            mem[0x4000..0x4004].copy_from_slice(&VIRTIO_BLK_T_OUT.to_le_bytes());
            mem[0x4008..0x4010].copy_from_slice(&4u64.to_le_bytes());
            // Data at 0x5000 (512 bytes)
            mem[0x5000..0x500D].copy_from_slice(b"CORAL_PERSIST");
            // Desc 0 (hdr) -> Desc 1 (data, readable) -> Desc 2 (status, writable)
            mem[0..8].copy_from_slice(&0x4000u64.to_le_bytes());
            write_u32_le(&mut mem, 8, 16);
            write_u32_le(&mut mem, 12, u32::from(VIRTQ_DESC_F_NEXT) | (1 << 16));

            mem[16..24].copy_from_slice(&0x5000u64.to_le_bytes());
            write_u32_le(&mut mem, 24, 512);
            write_u32_le(&mut mem, 28, u32::from(VIRTQ_DESC_F_NEXT) | (2 << 16));

            mem[32..40].copy_from_slice(&0x6000u64.to_le_bytes());
            write_u32_le(&mut mem, 40, 1);
            write_u32_le(&mut mem, 44, u32::from(VIRTQ_DESC_F_WRITE));

            mem[0x6000] = 0xFF;
            write_u16_le(&mut mem, 0x1004, 0);
            write_u16_le(&mut mem, 0x1002, 1);
            assert!(blk.write_mmio(0x050, &0u32.to_le_bytes(), &mut mem));
            assert_eq!(mem[0x6000], VIRTIO_BLK_S_OK);
            assert_eq!(blk.writes, 1);
        }

        // Reopen freshly and read sector 4 back.
        let mut blk2 = VirtioBlkDevice::open(&disk, true).unwrap();
        let mut mem2 = vec![0u8; 64 * 1024];
        setup_queue(&mut blk2, &mut mem2);
        mem2[0x4000..0x4004].copy_from_slice(&VIRTIO_BLK_T_IN.to_le_bytes());
        mem2[0x4008..0x4010].copy_from_slice(&4u64.to_le_bytes());

        mem2[0..8].copy_from_slice(&0x4000u64.to_le_bytes());
        write_u32_le(&mut mem2, 8, 16);
        write_u32_le(&mut mem2, 12, u32::from(VIRTQ_DESC_F_NEXT) | (1 << 16));

        mem2[16..24].copy_from_slice(&0x5000u64.to_le_bytes());
        write_u32_le(&mut mem2, 24, 512);
        write_u32_le(
            &mut mem2,
            28,
            u32::from(VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE) | (2 << 16),
        );

        mem2[32..40].copy_from_slice(&0x6000u64.to_le_bytes());
        write_u32_le(&mut mem2, 40, 1);
        write_u32_le(&mut mem2, 44, u32::from(VIRTQ_DESC_F_WRITE));

        write_u16_le(&mut mem2, 0x1004, 0);
        write_u16_le(&mut mem2, 0x1002, 1);
        assert!(blk2.write_mmio(0x050, &0u32.to_le_bytes(), &mut mem2));
        assert_eq!(mem2[0x6000], VIRTIO_BLK_S_OK);
        assert_eq!(&mem2[0x5000..0x500D], b"CORAL_PERSIST");
    }
}
