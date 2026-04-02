// src/iommu.rs
//
// Walks the ACPI DMAR table to find all Intel VT-d hardware units and
// disables their DMA-remapping translations.  This lets the kernel use
// DMA (xHCI event rings, command rings, etc.) without setting up proper
// IOMMU domains.  Called once at boot before any DMA-capable drivers.

use crate::memory::PHYS_MEM_OFFSET;
use core::sync::atomic::Ordering;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

/// Entry point: given the RSDP physical address from BootInfo, locate the
/// ACPI DMAR table and disable translations on every DRHD unit found.
pub fn disable_vtd(
    rsdp_phys: u64,
    mapper: &mut impl x86_64::structures::paging::Mapper<Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<Size4KiB>,
) {
    if rsdp_phys == 0 {
        crate::serial_println!("[IOMMU] No RSDP address — skipping VT-d disable.");
        return;
    }

    let phys_offset = PHYS_MEM_OFFSET.load(Ordering::Relaxed);

    let dmar_phys = match find_dmar_phys(rsdp_phys, phys_offset) {
        Some(p) => p,
        None => {
            crate::serial_println!("[IOMMU] No DMAR table in ACPI — VT-d not present.");
            return;
        }
    };

    crate::serial_println!("[IOMMU] DMAR table at phys {:#x}", dmar_phys);
    disable_drhd_units(dmar_phys, phys_offset, mapper, frame_allocator);
}

// ── ACPI helpers ─────────────────────────────────────────────────────────────

/// All ACPI tables are in physical memory mapped by the bootloader at
/// phys_offset + phys_addr.  This helper converts a physical address to the
/// corresponding virtual address used for reads.
#[inline]
fn p2v(phys: u64, phys_offset: u64) -> u64 {
    phys_offset + phys
}

/// Walk RSDP → XSDT (ACPI 2.0+) and return the physical address of the
/// first table with signature "DMAR", or None if not found.
fn find_dmar_phys(rsdp_phys: u64, phys_offset: u64) -> Option<u64> {
    let rsdp = p2v(rsdp_phys, phys_offset);

    // RSDP offset 15 = revision (0 = ACPI 1.0, 2 = ACPI 2.0+).
    // ACPI structs are packed — fields may be unaligned, so use read_unaligned.
    let revision = unsafe { core::ptr::read_unaligned((rsdp + 15) as *const u8) };
    if revision < 2 {
        // ACPI 1.0 RSDT uses 32-bit pointers; UEFI always provides ACPI 2.0+.
        crate::serial_println!("[IOMMU] ACPI 1.0 RSDP (revision={}), skipping.", revision);
        return None;
    }

    // RSDP offset 24 = XSDT physical address (u64).
    let xsdt_phys = unsafe { core::ptr::read_unaligned((rsdp + 24) as *const u64) };
    crate::serial_println!("[IOMMU] XSDT at phys {:#x}", xsdt_phys);

    let xsdt = p2v(xsdt_phys, phys_offset);
    // SDT header offset 4 = table length (u32).
    let xsdt_len = unsafe { core::ptr::read_unaligned((xsdt + 4) as *const u32) } as u64;

    // XSDT entries are 64-bit physical addresses starting after the 36-byte header.
    let num_entries = (xsdt_len.saturating_sub(36)) / 8;
    for i in 0..num_entries {
        let entry_phys =
            unsafe { core::ptr::read_unaligned((xsdt + 36 + i * 8) as *const u64) };
        if entry_phys == 0 {
            continue;
        }
        let entry = p2v(entry_phys, phys_offset);
        let sig = unsafe { core::ptr::read_unaligned(entry as *const [u8; 4]) };
        if &sig == b"DMAR" {
            return Some(entry_phys);
        }
    }
    None
}

// ── VT-d DMAR parsing ────────────────────────────────────────────────────────

/// Walk the DMAR remapping-structure list and disable translations on every
/// DRHD (type 0) entry.
fn disable_drhd_units(
    dmar_phys: u64,
    phys_offset: u64,
    mapper: &mut impl x86_64::structures::paging::Mapper<Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<Size4KiB>,
) {
    let dmar = p2v(dmar_phys, phys_offset);
    // ACPI structs are packed — fields may be unaligned, so use read_unaligned.
    let dmar_len =
        unsafe { core::ptr::read_unaligned((dmar + 4) as *const u32) } as u64;

    // Remapping structures start at offset 48:
    //   36 bytes SDT header + 1 HostAddressWidth + 1 Flags + 10 reserved
    let mut off = 48u64;
    while off + 4 <= dmar_len {
        let s = dmar + off;
        let remap_type = unsafe { core::ptr::read_unaligned(s as *const u16) };
        let remap_len  = unsafe { core::ptr::read_unaligned((s + 2) as *const u16) } as u64;

        if remap_len == 0 {
            break; // malformed table — stop walking
        }

        if remap_type == 0 {
            // DRHD: offset 8 = Register Base Address (u64)
            let reg_base = unsafe { core::ptr::read_unaligned((s + 8) as *const u64) };
            disable_one_drhd(reg_base, phys_offset, mapper, frame_allocator);
        }

        off += remap_len;
    }
}

/// Map the 4 KiB VT-d register page for one DRHD unit, then clear the
/// Translation Enable bit in its Global Command Register.
fn disable_one_drhd(
    reg_base_phys: u64,
    phys_offset: u64,
    mapper: &mut impl x86_64::structures::paging::Mapper<Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<Size4KiB>,
) {
    let reg_virt = phys_offset + reg_base_phys;

    // Map the register page (MMIO — must be uncacheable).
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
    let frame = PhysFrame::containing_address(PhysAddr::new(reg_base_phys));
    let page  = Page::containing_address(VirtAddr::new(reg_virt));
    unsafe {
        match mapper.map_to(page, frame, flags, frame_allocator) {
            Ok(flush) => flush.flush(),
            Err(_)    => {} // already mapped
        }
    }

    // GSTS (offset 0x1C): bit 31 = TES (Translation Enable Status).
    let gsts = unsafe { core::ptr::read_volatile((reg_virt + 0x1C) as *const u32) };
    if (gsts >> 31) & 1 == 0 {
        crate::serial_println!(
            "[IOMMU] DRHD {:#x}: translations already off.",
            reg_base_phys
        );
        return;
    }

    crate::serial_println!(
        "[IOMMU] DRHD {:#x}: disabling DMA translations (GSTS={:#010x})…",
        reg_base_phys,
        gsts
    );

    // GCMD (offset 0x18): writing 0 clears TE (and all other command bits).
    unsafe {
        core::ptr::write_volatile((reg_virt + 0x18) as *mut u32, 0);
    }

    // Poll GSTS until TES clears (hardware confirms translations are off).
    let mut spins = 0u32;
    loop {
        let s = unsafe { core::ptr::read_volatile((reg_virt + 0x1C) as *const u32) };
        if (s >> 31) & 1 == 0 {
            crate::serial_println!("[IOMMU] DRHD {:#x}: translations disabled.", reg_base_phys);
            break;
        }
        spins += 1;
        if spins > 2_000_000 {
            crate::serial_println!(
                "[IOMMU] DRHD {:#x}: TIMEOUT disabling (GSTS={:#010x}).",
                reg_base_phys,
                s
            );
            break;
        }
        core::hint::spin_loop();
    }
}
