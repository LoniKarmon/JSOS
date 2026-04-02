// src/power.rs — reboot and shutdown for bare-metal x86_64
//
// Reboot:  ACPI reset register (FADT) → keyboard controller 0xFE → triple fault
// Shutdown: ACPI S5 sleep state (PM1a/PM1b control block, SLP_TYP from DSDT)

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// Physical address of the RSDP, set once at boot from BootInfo.
pub static RSDP_PHYS: AtomicU64 = AtomicU64::new(0);

// ── ACPI helpers ─────────────────────────────────────────────────────────────

fn phys_offset() -> u64 {
    crate::memory::PHYS_MEM_OFFSET.load(Ordering::Relaxed)
}

fn p2v(phys: u64) -> u64 {
    phys_offset() + phys
}

/// Walk RSDP → XSDT (ACPI 2.0) or RSDT (ACPI 1.0), return the physical
/// address of the first SDT table whose 4-byte signature matches `sig`.
unsafe fn find_table(sig: &[u8; 4]) -> u64 {
    let rsdp_phys = RSDP_PHYS.load(Ordering::Relaxed);
    if rsdp_phys == 0 {
        return 0;
    }
    let rsdp = p2v(rsdp_phys);
    let revision = core::ptr::read_unaligned((rsdp + 15) as *const u8);

    // Try XSDT (ACPI 2.0+, 64-bit pointers) first.
    if revision >= 2 {
        let xsdt_phys = core::ptr::read_unaligned((rsdp + 24) as *const u64);
        if xsdt_phys != 0 {
            let xsdt = p2v(xsdt_phys);
            let len = core::ptr::read_unaligned((xsdt + 4) as *const u32) as u64;
            let n   = (len.saturating_sub(36)) / 8;
            for i in 0..n {
                let ep = core::ptr::read_unaligned((xsdt + 36 + i * 8) as *const u64);
                if ep == 0 { continue; }
                let s = core::ptr::read_unaligned(p2v(ep) as *const [u8; 4]);
                if &s == sig { return ep; }
            }
        }
    }

    // Fall back to RSDT (ACPI 1.0, 32-bit pointers).
    let rsdt_phys = core::ptr::read_unaligned((rsdp + 16) as *const u32) as u64;
    if rsdt_phys != 0 {
        let rsdt = p2v(rsdt_phys);
        let len  = core::ptr::read_unaligned((rsdt + 4) as *const u32) as u64;
        let n    = (len.saturating_sub(36)) / 4;
        for i in 0..n {
            let ep = core::ptr::read_unaligned((rsdt + 36 + i * 4) as *const u32) as u64;
            if ep == 0 { continue; }
            let s = core::ptr::read_unaligned(p2v(ep) as *const [u8; 4]);
            if &s == sig { return ep; }
        }
    }
    0
}

/// Generic Address Structure (GAS, 12 bytes):
///   [0]    AddressSpaceId  (0=SystemMemory, 1=SystemIO)
///   [1..3] bit-width / bit-offset / access-size
///   [4..11] Address (u64)
struct Gas {
    space:   u8,
    address: u64,
}

unsafe fn read_gas(ptr: u64) -> Gas {
    Gas {
        space:   core::ptr::read_unaligned(ptr as *const u8),
        address: core::ptr::read_unaligned((ptr + 4) as *const u64),
    }
}

unsafe fn gas_write_byte(gas: &Gas, value: u8) {
    match gas.space {
        0 => core::ptr::write_volatile(p2v(gas.address) as *mut u8, value),
        1 => { let mut p: Port<u8> = Port::new(gas.address as u16); p.write(value); }
        _ => {}
    }
}

// ── _S5_ / SLP_TYP parsing ───────────────────────────────────────────────────

/// Search the DSDT AML body for the `_S5_` object and return the first
/// SLP_TYP byte value.  Returns 5 (the most common default) on failure.
unsafe fn read_s5_slp_typ(dsdt_phys: u64) -> u8 {
    let dsdt     = p2v(dsdt_phys);
    let dsdt_len = core::ptr::read_unaligned((dsdt + 4) as *const u32) as u64;
    let end      = dsdt + dsdt_len;

    let mut ptr = dsdt + 36; // skip SDT header
    while ptr + 8 < end {
        let b = core::ptr::read_unaligned(ptr as *const [u8; 4]);
        if &b == b"_S5_" {
            // Look forward for PackageOp (0x12) within 4 bytes.
            let mut p = ptr + 4;
            let mut found_pkg = false;
            for _ in 0..4 {
                if core::ptr::read_unaligned(p as *const u8) == 0x12 {
                    found_pkg = true;
                    break;
                }
                p += 1;
            }
            if !found_pkg { ptr += 1; continue; }
            p += 1; // skip PackageOp

            // Package length: encoded in 1–4 bytes; top 2 bits of first byte
            // give extra byte count.
            let pkg_lead = core::ptr::read_unaligned(p as *const u8);
            let extra    = (pkg_lead >> 6) as u64;
            p += 1 + extra; // skip PkgLength

            // NumElements
            let n = core::ptr::read_unaligned(p as *const u8);
            p += 1;
            if n < 1 { ptr += 1; continue; }

            // First element: BytePrefix (0x0A) + value, or small integer (0–7).
            let b0 = core::ptr::read_unaligned(p as *const u8);
            let slp = if b0 == 0x0A {
                core::ptr::read_unaligned((p + 1) as *const u8)
            } else if b0 <= 7 {
                b0
            } else {
                5
            };
            crate::serial_println!("[power] _S5_: SLP_TYP5 = {}", slp);
            return slp;
        }
        ptr += 1;
    }
    crate::serial_println!("[power] _S5_ not found — using SLP_TYP5 = 5");
    5
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn reboot() {
    unsafe {
        // 1. ACPI reset register (FADT flags bit 10 = RESET_REG_SUP).
        let fadt_phys = find_table(b"FACP");
        if fadt_phys != 0 {
            let fadt     = p2v(fadt_phys);
            let fadt_len = core::ptr::read_unaligned((fadt + 4) as *const u32) as u64;
            let flags    = core::ptr::read_unaligned((fadt + 112) as *const u32);
            if fadt_len > 128 && (flags >> 10) & 1 == 1 {
                let gas        = read_gas(fadt + 116);
                let reset_val  = core::ptr::read_unaligned((fadt + 128) as *const u8);
                crate::serial_println!("[power] ACPI reset: val={:#x} space={} addr={:#x}",
                    reset_val, gas.space, gas.address);
                gas_write_byte(&gas, reset_val);
                for _ in 0..2_000_000u64 { core::hint::spin_loop(); }
            }
        }

        // 2. Keyboard controller pulse (legacy, works on most BIOS/some UEFI).
        crate::serial_println!("[power] KBC reboot pulse...");
        let mut kbc: Port<u8> = Port::new(0x64);
        for _ in 0..0xFFFFu32 {
            if kbc.read() & 0x02 == 0 { break; }
        }
        kbc.write(0xFEu8);
        for _ in 0..2_000_000u64 { core::hint::spin_loop(); }

        // 3. Triple fault (guaranteed reset on all x86_64).
        crate::serial_println!("[power] Triple fault reset...");
        let null_idtr = [0u8; 10];
        core::arch::asm!(
            "lidt [{0}]",
            "ud2",
            in(reg) null_idtr.as_ptr(),
            options(nostack, noreturn),
        );
    }
}

pub fn shutdown() {
    unsafe {
        let fadt_phys = find_table(b"FACP");
        if fadt_phys != 0 {
            let fadt      = p2v(fadt_phys);
            let fadt_len  = core::ptr::read_unaligned((fadt + 4) as *const u32) as u64;
            let hdr_rev   = core::ptr::read_unaligned((fadt + 8) as *const u8);

            // PM1a_CNT_BLK at FADT offset 64 (legacy I/O port, 32-bit field).
            let pm1a_port = core::ptr::read_unaligned((fadt + 64) as *const u32) as u16;
            let pm1b_port = core::ptr::read_unaligned((fadt + 68) as *const u32) as u16;

            // Prefer the 64-bit X_DSDT pointer (FADT offset 140) on ACPI 2.0+.
            let dsdt_phys: u64 = if hdr_rev >= 2 && fadt_len > 148 {
                let x = core::ptr::read_unaligned((fadt + 140) as *const u64);
                if x != 0 { x } else {
                    core::ptr::read_unaligned((fadt + 40) as *const u32) as u64
                }
            } else {
                core::ptr::read_unaligned((fadt + 40) as *const u32) as u64
            };

            let slp_typ5 = if dsdt_phys != 0 { read_s5_slp_typ(dsdt_phys) } else { 5 };
            let val: u16 = (1u16 << 13) | ((slp_typ5 as u16) << 10);

            crate::serial_println!(
                "[power] ACPI shutdown: PM1a={:#x} PM1b={:#x} SLP_TYP5={} val={:#x}",
                pm1a_port, pm1b_port, slp_typ5, val
            );

            if pm1a_port != 0 {
                let mut p: Port<u16> = Port::new(pm1a_port);
                p.write(val);
            }
            if pm1b_port != 0 {
                let mut p: Port<u16> = Port::new(pm1b_port);
                p.write(val);
            }
            for _ in 0..10_000_000u64 { core::hint::spin_loop(); }
        }
    }

    // Fallback: halt.
    crate::println!("It is now safe to turn off your computer.");
    loop {
        x86_64::instructions::interrupts::disable();
        x86_64::instructions::hlt();
    }
}
