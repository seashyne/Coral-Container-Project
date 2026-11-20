//! Shared VirtIO-MMIO v2 (modern VirtIO 1.x) transport registers and split-virtqueue
//! descriptor/avail/used ring engine used by `virtio-blk` and `virtio-net`.

use std::sync::atomic::{Ordering, fence};

pub const MMIO_MAGIC: u32 = 0x7472_6976; // "virt"
pub const MMIO_VERSION: u32 = 2; // VirtIO 1.x modern MMIO
pub const VENDOR_CORAL: u32 = 0x4352_4C21; // "CRL!"

pub const VIRTIO_DEV_NET: u32 = 1;
pub const VIRTIO_DEV_BLK: u32 = 2;

pub const VIRTIO_F_VERSION_1_HI: u32 = 1 << 0; // Feature bit 32 (word 1 bit 0)
pub const VIRTIO_MMIO_INT_VRING: u32 = 1 << 0;

pub const VIRTQ_DESC_F_NEXT: u16 = 1 << 0;
pub const VIRTQ_DESC_F_WRITE: u16 = 1 << 1;

const QUEUE_NUM_MAX: u16 = 256;

/// One validated scatter-gather segment from a VirtIO descriptor chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescSegment {
    pub addr: u64,
    pub len: u32,
    pub write_only: bool,
}

/// Complete validated descriptor chain popped from a split-virtqueue available ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescChain {
    pub head: u16,
    pub segments: Vec<DescSegment>,
}

/// Split-virtqueue state tracked by the hypervisor for a single queue index.
#[derive(Debug, Clone)]
pub struct VirtQueue {
    pub ready: bool,
    pub num: u16,
    pub desc_addr: u64,
    pub avail_addr: u64,
    pub used_addr: u64,
    pub last_avail_idx: u16,
    pub last_used_idx: u16,
}

impl Default for VirtQueue {
    fn default() -> Self {
        Self {
            ready: false,
            num: QUEUE_NUM_MAX,
            desc_addr: 0,
            avail_addr: 0,
            used_addr: 0,
            last_avail_idx: 0,
            last_used_idx: 0,
        }
    }
}

impl VirtQueue {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Pops the next unprocessed descriptor chain from the guest's available ring.
    ///
    /// Returns `None` cleanly if the ring is empty or if any guest address/length/index
    /// is out-of-bounds or forms a cycle, ensuring hostile guests cannot crash the VMM.
    pub fn pop_avail_chain(&mut self, mem: &[u8]) -> Option<DescChain> {
        if !self.ready || self.num == 0 || !self.num.is_power_of_two() || self.num > QUEUE_NUM_MAX {
            return None;
        }
        fence(Ordering::Acquire);
        let avail_idx = read_u16_le(mem, self.avail_addr.checked_add(2)?)?;
        if avail_idx == self.last_avail_idx {
            return None;
        }

        let mask = self.num - 1;
        let ring_slot = (self.last_avail_idx & mask) as u64;
        let head = read_u16_le(mem, self.avail_addr.checked_add(4 + ring_slot * 2)?)?;
        if head >= self.num {
            self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
            return None;
        }

        let mut segments = Vec::with_capacity(4);
        let mut idx = head;
        let mut steps = 0u16;
        loop {
            if idx >= self.num || steps >= self.num {
                // Cycle or out-of-range descriptor index: drop malformed chain safely.
                self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
                return None;
            }
            let desc_off = self.desc_addr.checked_add(u64::from(idx) * 16)?;
            let buf_addr = read_u64_le(mem, desc_off)?;
            let buf_len = read_u32_le(mem, desc_off.checked_add(8)?)?;
            let flags = read_u16_le(mem, desc_off.checked_add(12)?)?;
            let next = read_u16_le(mem, desc_off.checked_add(14)?)?;

            let end = buf_addr.checked_add(u64::from(buf_len))?;
            if end as usize > mem.len() {
                self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
                return None;
            }

            segments.push(DescSegment {
                addr: buf_addr,
                len: buf_len,
                write_only: (flags & VIRTQ_DESC_F_WRITE) != 0,
            });

            if (flags & VIRTQ_DESC_F_NEXT) == 0 {
                break;
            }
            idx = next;
            steps = steps.saturating_add(1);
        }

        self.last_avail_idx = self.last_avail_idx.wrapping_add(1);
        Some(DescChain { head, segments })
    }

    /// Writes a completed descriptor chain into the guest's used ring.
    pub fn push_used(&mut self, mem: &mut [u8], head: u16, written_len: u32) -> bool {
        if !self.ready || self.num == 0 || !self.num.is_power_of_two() {
            return false;
        }
        let mask = self.num - 1;
        let ring_slot = (self.last_used_idx & mask) as u64;
        let Some(elem_off) = self.used_addr.checked_add(4 + ring_slot * 8) else {
            return false;
        };
        if !write_u32_le(mem, elem_off, u32::from(head)) {
            return false;
        }
        if !write_u32_le(mem, elem_off + 4, written_len) {
            return false;
        }
        fence(Ordering::Release);
        self.last_used_idx = self.last_used_idx.wrapping_add(1);
        let Some(idx_off) = self.used_addr.checked_add(2) else {
            return false;
        };
        if !write_u16_le(mem, idx_off, self.last_used_idx) {
            return false;
        }
        fence(Ordering::SeqCst);
        true
    }
}

/// Generic VirtIO-MMIO v2 register bank wrapping N queues and device config space (`0x100..`).
#[derive(Debug, Clone)]
pub struct VirtioMmioTransport {
    pub device_id: u32,
    pub features_lo: u32,
    pub features_hi: u32,
    pub device_features_sel: u32,
    pub driver_features_lo: u32,
    pub driver_features_hi: u32,
    pub driver_features_sel: u32,
    pub queue_sel: usize,
    pub queues: Vec<VirtQueue>,
    pub status: u32,
    pub interrupt_status: u32,
    pub config: Vec<u8>,
}

impl VirtioMmioTransport {
    pub fn new(device_id: u32, features_lo: u32, features_hi: u32, num_queues: usize, config: Vec<u8>) -> Self {
        Self {
            device_id,
            features_lo,
            features_hi: features_hi | VIRTIO_F_VERSION_1_HI,
            device_features_sel: 0,
            driver_features_lo: 0,
            driver_features_hi: 0,
            driver_features_sel: 0,
            queue_sel: 0,
            queues: vec![VirtQueue::default(); num_queues],
            status: 0,
            interrupt_status: 0,
            config,
        }
    }

    pub fn read_mmio(&self, offset: u64, data: &mut [u8]) {
        data.fill(0);
        if offset >= 0x100 {
            let cfg_off = (offset - 0x100) as usize;
            for (i, dst) in data.iter_mut().enumerate() {
                if let Some(b) = self.config.get(cfg_off + i) {
                    *dst = *b;
                }
            }
            return;
        }

        let val: u32 = match offset {
            0x000 => MMIO_MAGIC,
            0x004 => MMIO_VERSION,
            0x008 => self.device_id,
            0x00C => VENDOR_CORAL,
            0x010 => match self.device_features_sel {
                0 => self.features_lo,
                1 => self.features_hi,
                _ => 0,
            },
            0x034 => self
                .queues
                .get(self.queue_sel)
                .map(|_| u32::from(QUEUE_NUM_MAX))
                .unwrap_or(0),
            0x044 => self
                .queues
                .get(self.queue_sel)
                .map(|q| u32::from(q.ready))
                .unwrap_or(0),
            0x060 => self.interrupt_status,
            0x070 => self.status,
            0x0FC => 0,
            _ => 0,
        };
        let bytes = val.to_le_bytes();
        let n = data.len().min(4);
        data[..n].copy_from_slice(&bytes[..n]);
    }

    /// Processes an MMIO register write. Returns `Some(queue_idx)` when `QueueNotify` is written.
    pub fn write_mmio(&mut self, offset: u64, data: &[u8]) -> Option<usize> {
        let mut buf = [0u8; 4];
        let n = data.len().min(4);
        buf[..n].copy_from_slice(&data[..n]);
        let val = u32::from_le_bytes(buf);

        match offset {
            0x014 => self.device_features_sel = val,
            0x020 => match self.driver_features_sel {
                0 => self.driver_features_lo = val,
                1 => self.driver_features_hi = val,
                _ => {}
            },
            0x024 => self.driver_features_sel = val,
            0x030 => self.queue_sel = val as usize,
            0x038 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.num = (val as u16).min(QUEUE_NUM_MAX);
                }
            }
            0x044 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.ready = val != 0;
                }
            }
            0x050 => return Some(val as usize),
            0x064 => self.interrupt_status &= !val,
            0x070 => {
                if val == 0 {
                    self.status = 0;
                    self.interrupt_status = 0;
                    for q in &mut self.queues {
                        q.reset();
                    }
                } else {
                    self.status = val;
                }
            }
            0x080 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.desc_addr = (q.desc_addr & 0xFFFF_FFFF_0000_0000) | u64::from(val);
                }
            }
            0x084 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.desc_addr = (q.desc_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(val) << 32);
                }
            }
            0x090 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.avail_addr = (q.avail_addr & 0xFFFF_FFFF_0000_0000) | u64::from(val);
                }
            }
            0x094 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.avail_addr = (q.avail_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(val) << 32);
                }
            }
            0x0A0 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.used_addr = (q.used_addr & 0xFFFF_FFFF_0000_0000) | u64::from(val);
                }
            }
            0x0A4 => {
                if let Some(q) = self.queues.get_mut(self.queue_sel) {
                    q.used_addr = (q.used_addr & 0x0000_0000_FFFF_FFFF) | (u64::from(val) << 32);
                }
            }
            _ => {}
        }
        None
    }
}

pub fn read_u16_le(mem: &[u8], addr: u64) -> Option<u16> {
    let off = usize::try_from(addr).ok()?;
    let slice = mem.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]))
}

pub fn read_u32_le(mem: &[u8], addr: u64) -> Option<u32> {
    let off = usize::try_from(addr).ok()?;
    let slice = mem.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

pub fn read_u64_le(mem: &[u8], addr: u64) -> Option<u64> {
    let off = usize::try_from(addr).ok()?;
    let slice = mem.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes(slice.try_into().ok()?))
}

pub fn write_u16_le(mem: &mut [u8], addr: u64, val: u16) -> bool {
    let Ok(off) = usize::try_from(addr) else {
        return false;
    };
    let Some(end) = off.checked_add(2) else {
        return false;
    };
    let Some(slice) = mem.get_mut(off..end) else {
        return false;
    };
    slice.copy_from_slice(&val.to_le_bytes());
    true
}

pub fn write_u32_le(mem: &mut [u8], addr: u64, val: u32) -> bool {
    let Ok(off) = usize::try_from(addr) else {
        return false;
    };
    let Some(end) = off.checked_add(4) else {
        return false;
    };
    let Some(slice) = mem.get_mut(off..end) else {
        return false;
    };
    slice.copy_from_slice(&val.to_le_bytes());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_virtqueue_rejects_cycles_and_oob_buffers() {
        let mut mem = vec![0u8; 4096];
        let mut q = VirtQueue {
            ready: true,
            num: 8,
            desc_addr: 0,
            avail_addr: 256,
            used_addr: 512,
            last_avail_idx: 0,
            last_used_idx: 0,
        };

        // Cyclic descriptor: desc 0 points to desc 1, desc 1 points back to desc 0.
        write_u32_le(&mut mem, 12, u32::from(VIRTQ_DESC_F_NEXT) | (1u32 << 16));
        write_u32_le(&mut mem, 28, u32::from(VIRTQ_DESC_F_NEXT)); // next = 0
        write_u16_le(&mut mem, 256 + 4, 0);
        write_u16_le(&mut mem, 256 + 2, 1);
        assert!(q.pop_avail_chain(&mem).is_none());

        // Out-of-bounds buffer pointer in descriptor.
        write_u32_le(&mut mem, 0, 0xFFFF_F000);
        write_u32_le(&mut mem, 8, 4096);
        write_u32_le(&mut mem, 12, 0);
        write_u16_le(&mut mem, 256 + 6, 0);
        write_u16_le(&mut mem, 256 + 2, 2);
        assert!(q.pop_avail_chain(&mem).is_none());

        // Valid descriptor chain succeeds and push_used advances used idx.
        mem[0..8].copy_from_slice(&1024u64.to_le_bytes());
        write_u32_le(&mut mem, 8, 512);
        write_u32_le(&mut mem, 12, u32::from(VIRTQ_DESC_F_WRITE));
        write_u16_le(&mut mem, 256 + 8, 0);
        write_u16_le(&mut mem, 256 + 2, 3);
        let chain = q.pop_avail_chain(&mem).expect("valid descriptor chain");
        assert_eq!(chain.head, 0);
        assert_eq!(chain.segments[0].len, 512);
        assert!(q.push_used(&mut mem, chain.head, 128));
        assert_eq!(read_u16_le(&mem, 512 + 2), Some(1));
    }
}
