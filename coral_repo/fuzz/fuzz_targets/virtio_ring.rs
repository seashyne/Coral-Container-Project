#![no_main]
use coral_vmm::virtio_mmio::{VirtQueue, VirtioMmioTransport};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let split = (data.len() / 2).max(8);
    let mut mem = data[split..].to_vec();
    if mem.len() < 64 {
        mem.resize(64, 0);
    }
    let num = match data[0] % 4 {
        0 => 4,
        1 => 8,
        2 => 16,
        _ => 32,
    };
    let mut q = VirtQueue {
        ready: true,
        num,
        desc_addr: u64::from(data[1]) * 4,
        avail_addr: u64::from(data[2]) * 4,
        used_addr: u64::from(data[3]) * 4,
        last_avail_idx: u16::from_le_bytes([data[4], data[5]]),
        last_used_idx: u16::from_le_bytes([data[6], data[7]]),
    };
    if let Some(chain) = q.pop_avail_chain(&mem) {
        let _ = q.push_used(&mut mem, chain.head, chain.segments.len() as u32);
    }

    let mut tr = VirtioMmioTransport::new(2, 0, 0, 1, vec![0u8; 64]);
    let mut reg_out = [0u8; 4];
    for chunk in data[8..split].chunks_exact(6) {
        let off = u64::from(u16::from_le_bytes([chunk[0], chunk[1]]) % 0x140);
        tr.write_mmio(off, &chunk[2..6]);
        tr.read_mmio(off, &mut reg_out);
    }
});
