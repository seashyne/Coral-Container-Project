//! Guest physical memory layout for the Linux x86-64 boot protocol.
//!
//! Builds the flat 64-bit GDT, 4-level identity page tables, kernel payload mapping,
//! kernel command line, `boot_params` (`zero_page`), and the E820 RAM map.

use super::LinuxBootOptions;
use anyhow::{Result, bail};
use crate::kvm::write_u64_le;

const GDT_ADDR: u64 = 0x0500;
const ZERO_PAGE_ADDR: u64 = 0x7000;
const PML4_ADDR: u64 = 0x9000;
const PDPT_ADDR: u64 = 0xA000;
const PD_BASE_ADDR: u64 = 0xB000; // 4 pages: 0xB000..0xEFFF -> covers 0..4 GiB
const CMDLINE_ADDR: u64 = 0x2_0000;

/// Populates guest RAM with everything the Linux 64-bit boot protocol requires and returns
/// the guest-physical load address of the kernel payload.
pub(super) fn build_guest_memory(
    mem: &mut [u8],
    bzimage: &[u8],
    kernel_payload: &[u8],
    initrd_bytes: Option<&[u8]>,
    opts: &LinuxBootOptions,
    mem_size: usize,
    mem_mb: usize,
) -> Result<usize> {
    // 3. Populate flat 64-bit GDT at 0x0500 (0x10 = __BOOT_CS, 0x18 = __BOOT_DS).
    write_u64_le(mem, GDT_ADDR as usize, 0);
    write_u64_le(mem, GDT_ADDR as usize + 8, 0);
    write_u64_le(mem, GDT_ADDR as usize + 16, 0x00AF_9A00_0000_FFFF); // 0x10: 64-bit code
    write_u64_le(mem, GDT_ADDR as usize + 24, 0x00CF_9200_0000_FFFF); // 0x18: 64-bit data

    // 4. Populate 4-level identity page tables covering 0..4 GiB via 2 MiB huge pages.
    write_u64_le(mem, PML4_ADDR as usize, PDPT_ADDR | 0x3);
    for gb in 0..4usize {
        let pd_addr = PD_BASE_ADDR + (gb as u64) * 0x1000;
        write_u64_le(mem, PDPT_ADDR as usize + gb * 8, pd_addr | 0x3);
        for entry in 0..512usize {
            let phys = ((gb as u64) << 30) + ((entry as u64) << 21);
            write_u64_le(mem, pd_addr as usize + entry * 8, phys | 0x83);
        }
    }

    // 5. Load Linux bzImage 64-bit kernel payload (at pref_address if relocatable & fits, else 1 MiB).
    let relocatable = bzimage.get(0x234).copied().unwrap_or(0) != 0;
    let pref_address = if bzimage.len() >= 0x260 {
        u64::from_le_bytes(bzimage[0x258..0x260].try_into().unwrap())
    } else {
        0x10_0000
    };
    let load_addr: usize = if relocatable
        && pref_address >= 0x10_0000
        && (pref_address as usize) + kernel_payload.len() + 0x200_0000 < mem_size
    {
        pref_address as usize
    } else {
        0x10_0000
    };
    if load_addr + kernel_payload.len() >= mem_size {
        bail!(
            "Linux kernel payload ({} bytes) does not fit in {} MiB guest RAM",
            kernel_payload.len(),
            mem_mb
        );
    }
    mem[load_addr..load_addr + kernel_payload.len()].copy_from_slice(kernel_payload);

    // 6. Copy kernel command line to 0x20000 (adding VirtIO-MMIO devices when enabled).
    let mut effective_cmdline = opts.cmdline.clone();
    if opts.disk_image.is_some() || opts.enable_net {
        effective_cmdline = effective_cmdline
            .split_whitespace()
            .filter(|t| *t != "nomodules")
            .collect::<Vec<_>>()
            .join(" ");
    }
    if opts.disk_image.is_some() && !effective_cmdline.contains("0xd0001000") {
        effective_cmdline.push_str(" virtio_mmio.device=4K@0xd0001000:5");
    }
    if opts.enable_net && !effective_cmdline.contains("0xd0002000") {
        effective_cmdline.push_str(" virtio_mmio.device=4K@0xd0002000:6");
    }
    let cmdline_bytes = effective_cmdline.as_bytes();
    let max_cmdline = 4095usize;
    let copy_len = cmdline_bytes.len().min(max_cmdline);
    mem[CMDLINE_ADDR as usize..CMDLINE_ADDR as usize + copy_len]
        .copy_from_slice(&cmdline_bytes[..copy_len]);
    mem[CMDLINE_ADDR as usize + copy_len] = 0;

    // 7. Populate Linux boot_params (zero_page) at 0x7000.
    let zp = ZERO_PAGE_ADDR as usize;
    let header_end = (0x202 + bzimage[0x201] as usize).min(0x290).min(bzimage.len());
    mem[zp + 0x1F1..zp + header_end].copy_from_slice(&bzimage[0x1F1..header_end]);
    mem[zp + 0x210] = 0xFF; // type_of_loader = undefined
    mem[zp + 0x211] |= 0x80 | 0x40 | 0x01; // CAN_USE_HEAP | KEEP_SEGMENTS | LOADED_HIGH
    mem[zp + 0x224..zp + 0x226].copy_from_slice(&0xFE00u16.to_le_bytes()); // heap_end_ptr
    mem[zp + 0x214..zp + 0x218].copy_from_slice(&(load_addr as u32).to_le_bytes()); // code32_start
    mem[zp + 0x228..zp + 0x22C].copy_from_slice(&(CMDLINE_ADDR as u32).to_le_bytes());
    mem[zp + 0x238..zp + 0x23C].copy_from_slice(&(copy_len as u32).to_le_bytes());

    // Optional initramfs placed near top of guest RAM (respecting initrd_addr_max).
    if let Some(initrd) = initrd_bytes {
        let initrd_max = if bzimage.len() >= 0x230 {
            u32::from_le_bytes(bzimage[0x22C..0x230].try_into().unwrap()) as usize
        } else {
            0x37FF_FFFF
        };
        let top = mem_size.min(initrd_max).saturating_sub(0x1_0000);
        if initrd.len() + load_addr + kernel_payload.len() + 0x10_0000 >= top {
            bail!(
                "initramfs ({} bytes) + kernel exceeds {} MiB guest RAM",
                initrd.len(),
                mem_mb
            );
        }
        let initrd_addr = (top - initrd.len()) & !0xFFF;
        mem[initrd_addr..initrd_addr + initrd.len()].copy_from_slice(initrd);
        mem[zp + 0x218..zp + 0x21C].copy_from_slice(&(initrd_addr as u32).to_le_bytes());
        mem[zp + 0x21C..zp + 0x220].copy_from_slice(&(initrd.len() as u32).to_le_bytes());
    }

    // Write E820 memory map (3 entries: low RAM, BIOS/EBDA reserved, high RAM).
    mem[zp + 0x1E8] = 3;
    write_e820_entry(mem, zp + 0x2D0, 0, 0x9_FC00, 1);
    write_e820_entry(mem, zp + 0x2D0 + 20, 0x9_FC00, 0x6_0400, 2);
    write_e820_entry(mem, zp + 0x2D0 + 40, 0x10_0000, (mem_size - 0x10_0000) as u64, 1);

    Ok(load_addr)
}

fn write_e820_entry(mem: &mut [u8], offset: usize, addr: u64, size: u64, kind: u32) {
    mem[offset..offset + 8].copy_from_slice(&addr.to_le_bytes());
    mem[offset + 8..offset + 16].copy_from_slice(&size.to_le_bytes());
    mem[offset + 16..offset + 20].copy_from_slice(&kind.to_le_bytes());
}
