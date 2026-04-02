// src/sysinfo.rs — CPUID, memory info, and CMOS real-time clock for JSOS

use alloc::string::String;
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// Total physical RAM in bytes, set once at boot from the memory map.
pub static TOTAL_PHYS_RAM: AtomicU64 = AtomicU64::new(0);

// ── CPUID helpers ─────────────────────────────────────────────────────────

fn cpuid_vendor() -> String {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let result = core::arch::x86_64::__cpuid(0);
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&result.ebx.to_le_bytes());
        bytes[4..8].copy_from_slice(&result.edx.to_le_bytes());
        bytes[8..12].copy_from_slice(&result.ecx.to_le_bytes());
        String::from_utf8_lossy(&bytes).trim().into()
    }
    #[cfg(not(target_arch = "x86_64"))]
    String::from("Unknown")
}

fn cpuid_brand() -> String {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // Check if extended CPUID leaves are supported
        let ext = core::arch::x86_64::__cpuid(0x80000000);
        if ext.eax < 0x80000004 {
            return String::from("Unknown CPU");
        }

        let mut bytes = [0u8; 48];
        for (i, leaf) in [0x80000002u32, 0x80000003, 0x80000004].iter().enumerate() {
            let result = core::arch::x86_64::__cpuid(*leaf);
            let off = i * 16;
            bytes[off..off+4].copy_from_slice(&result.eax.to_le_bytes());
            bytes[off+4..off+8].copy_from_slice(&result.ebx.to_le_bytes());
            bytes[off+8..off+12].copy_from_slice(&result.ecx.to_le_bytes());
            bytes[off+12..off+16].copy_from_slice(&result.edx.to_le_bytes());
        }
        String::from_utf8_lossy(&bytes).trim_matches(|c: char| c == '\0' || c == ' ').trim().into()
    }
    #[cfg(not(target_arch = "x86_64"))]
    String::from("Unknown CPU")
}

fn cpuid_frequency() -> (u32, u32) {
    // Returns (base_mhz, max_mhz). Uses CPUID leaf 0x16 if available.
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let max_leaf = core::arch::x86_64::__cpuid(0).eax;
        if max_leaf >= 0x16 {
            let freq = core::arch::x86_64::__cpuid(0x16);
            let base = freq.eax as u32; // base frequency in MHz
            let max = freq.ebx as u32;  // max frequency in MHz
            if base > 0 {
                return (base, max);
            }
        }
        // Fallback: try to estimate from TSC over a short spin
        // We use PIT ticks as a rough reference: ~18.2 Hz
        let start_ticks = crate::interrupts::TICKS.load(Ordering::Relaxed);
        let start_tsc: u64;
        core::arch::asm!("rdtsc", out("eax") _, out("edx") _, options(nomem, nostack));
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        start_tsc = ((hi as u64) << 32) | (lo as u64);

        // Wait for ~2 ticks (use the actual tick rate)
        while crate::interrupts::TICKS.load(Ordering::Relaxed) < start_ticks + 2 {
            core::hint::spin_loop();
        }

        let end_lo: u32;
        let end_hi: u32;
        core::arch::asm!("rdtsc", out("eax") end_lo, out("edx") end_hi, options(nomem, nostack));
        let end_tsc = ((end_hi as u64) << 32) | (end_lo as u64);

        let elapsed_tsc = end_tsc.saturating_sub(start_tsc);
        // 2 ticks at TICKS_PER_SEC Hz = 2/TICKS_PER_SEC seconds
        let hz = crate::interrupts::TICKS_PER_SEC.load(Ordering::Relaxed).max(1) as f64;
        let mhz = (elapsed_tsc as f64 * hz / 2.0 / 1_000_000.0) as u32;
        (mhz, mhz)
    }
    #[cfg(not(target_arch = "x86_64"))]
    (0, 0)
}

fn cpuid_features() -> Vec<&'static str> {
    let mut features = Vec::new();
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let result = core::arch::x86_64::__cpuid(1);
        let ecx = result.ecx;
        let edx = result.edx;

        // EDX features
        if edx & (1 << 0)  != 0 { features.push("FPU"); }
        if edx & (1 << 23) != 0 { features.push("MMX"); }
        if edx & (1 << 25) != 0 { features.push("SSE"); }
        if edx & (1 << 26) != 0 { features.push("SSE2"); }

        // ECX features
        if ecx & (1 << 0)  != 0 { features.push("SSE3"); }
        if ecx & (1 << 9)  != 0 { features.push("SSSE3"); }
        if ecx & (1 << 19) != 0 { features.push("SSE4.1"); }
        if ecx & (1 << 20) != 0 { features.push("SSE4.2"); }
        if ecx & (1 << 25) != 0 { features.push("AES-NI"); }
        if ecx & (1 << 28) != 0 { features.push("AVX"); }
        if ecx & (1 << 30) != 0 { features.push("RDRAND"); }

        // Check extended features (leaf 7)
        let max_leaf = core::arch::x86_64::__cpuid(0).eax;
        if max_leaf >= 7 {
            let ext = core::arch::x86_64::__cpuid_count(7, 0);
            if ext.ebx & (1 << 5)  != 0 { features.push("AVX2"); }
            if ext.ebx & (1 << 16) != 0 { features.push("AVX-512"); }
        }
    }
    features
}

// ── Memory helpers ────────────────────────────────────────────────────────

fn get_heap_info() -> (usize, usize) {
    // Returns (used_bytes, total_bytes)
    let total = crate::allocator::HEAP_SIZE;
    let free = crate::allocator::ALLOCATOR.lock().free();
    (total - free, total)
}

// ── Public API ────────────────────────────────────────────────────────────

/// Returns a JSON string with system information.
pub fn get_sysinfo() -> String {
    let vendor = cpuid_vendor();
    let brand = cpuid_brand();
    let (base_mhz, max_mhz) = cpuid_frequency();
    let features = cpuid_features();
    let total_ram = TOTAL_PHYS_RAM.load(Ordering::Relaxed);
    let total_ram_mb = total_ram / (1024 * 1024);
    let (heap_used, heap_total) = get_heap_info();
    let heap_used_mb = heap_used / (1024 * 1024);
    let heap_total_mb = heap_total / (1024 * 1024);

    let features_json = features
        .iter()
        .map(|f| format!("\"{}\"", f))
        .collect::<Vec<String>>()
        .join(",");

    format!(
        concat!(
            "{{",
            "\"vendor\":\"{}\",",
            "\"brand\":\"{}\",",
            "\"base_mhz\":{},",
            "\"max_mhz\":{},",
            "\"features\":[{}],",
            "\"total_ram_mb\":{},",
            "\"heap_used_mb\":{},",
            "\"heap_total_mb\":{}",
            "}}"
        ),
        vendor, brand, base_mhz, max_mhz, features_json,
        total_ram_mb, heap_used_mb, heap_total_mb
    )
}

// ── CMOS RTC ──────────────────────────────────────────────────────────────

fn cmos_read(reg: u8) -> u8 {
    unsafe {
        let mut addr: Port<u8> = Port::new(0x70);
        let mut data: Port<u8> = Port::new(0x71);
        addr.write(reg);
        data.read()
    }
}

fn bcd_to_bin(bcd: u8) -> u8 {
    (bcd & 0x0F) + ((bcd >> 4) * 10)
}

/// Returns a JSON string with the current RTC date and time.
pub fn get_rtc() -> String {
    // Wait for update-not-in-progress
    while cmos_read(0x0A) & 0x80 != 0 {}

    let sec   = cmos_read(0x00);
    let min   = cmos_read(0x02);
    let hour  = cmos_read(0x04);
    let day   = cmos_read(0x07);
    let month = cmos_read(0x08);
    let year  = cmos_read(0x09);

    // Check register B to see if BCD mode
    let reg_b = cmos_read(0x0B);
    let is_bcd = (reg_b & 0x04) == 0;

    let (s, m, h, d, mo, y) = if is_bcd {
        (
            bcd_to_bin(sec),
            bcd_to_bin(min),
            bcd_to_bin(hour & 0x7F),
            bcd_to_bin(day),
            bcd_to_bin(month),
            bcd_to_bin(year),
        )
    } else {
        (sec, min, hour & 0x7F, day, month, year)
    };

    // Handle 12-hour mode
    let h = if (reg_b & 0x02) == 0 && (hour & 0x80) != 0 {
        // 12-hour mode, PM flag set
        (h % 12) + 12
    } else {
        h
    };

    let full_year = 2000u16 + y as u16;

    format!(
        "{{\"h\":{},\"m\":{},\"s\":{},\"day\":{},\"month\":{},\"year\":{}}}",
        h, m, s, d, mo, full_year
    )
}
