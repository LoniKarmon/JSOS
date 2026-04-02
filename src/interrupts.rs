// src/interrupts.rs

use core::sync::atomic::Ordering;
use crate::println;
use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use pic8259::ChainedPics;
use spin;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Ticks per second — 18 until LAPIC timer is calibrated, then 100.
/// All time calculations should use this instead of hardcoded 18.
pub static TICKS_PER_SEC: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(18);

/// Set to true when the LAPIC timer is active as the tick source.
/// When true, the timer handler sends LAPIC EOI instead of PIC EOI.
pub static LAPIC_TIMER_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// xAPIC MMIO virtual address of the LAPIC EOI register (offset 0x0B0 from LAPIC base).
/// 0 = use x2APIC MSR 0x80B instead.
static LAPIC_EOI_VIRT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

// NOTE: ALT_PRESSED / SHIFT_PRESSED / CTRL_PRESSED / HEBREW_LAYOUT have been
// moved to keyboard.rs where they are actually updated and read. The copies
// that used to live here were dead code after the keyboard refactor.

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
    Mouse = PIC_1_OFFSET + 12, // IRQ 12
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();

        idt.breakpoint.set_handler_fn(breakpoint_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);

        // Double fault uses IST slot 0 — a dedicated emergency stack.
        // This guarantees the handler runs even if the kernel stack overflowed.
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
        }

        idt[InterruptIndex::Timer.as_u8().into()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8().into()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_u8().into()].set_handler_fn(mouse_interrupt_handler);

        idt
    };
}

pub fn init_idt() {
    IDT.load();
}

/// Unmask the hardware IRQs the kernel actually uses.
/// Must be called AFTER `PICS.lock().initialize()` — initialize() resets both
/// PIC IMRs to 0xFF (all masked), so any write_masks before it is overwritten.
///
/// Master PIC mask 0b1111_1000: unmask IRQ0 (timer), IRQ1 (keyboard), IRQ2 (cascade)
/// Slave  PIC mask 0b1110_1111: unmask IRQ12 (mouse), keep rest masked
pub fn unmask_irqs() {
    unsafe {
        PICS.lock().write_masks(0b1111_1000, 0b1110_1111);
    }
}

// ── Exception handlers ────────────────────────────────────────────────────

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    let faulting_address = Cr2::read();

    crate::serial_println!("EXCEPTION: PAGE FAULT");
    crate::serial_println!("  Faulting address : {:?}", faulting_address);
    crate::serial_println!("  Error code       : {:?}", error_code);
    crate::serial_println!("  Stack frame      : {:#?}", stack_frame);

    // Render a visible BSOD so page faults are obvious during development.
    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(writer) = crate::framebuffer::FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.background_color = (0, 0, 170);
            writer.foreground_color = (255, 255, 255);
            writer.clear_screen();
        }
    });

    println!("\n\n*** KERNEL PANIC - PAGE FAULT ***\n");
    println!("Faulting address : {:?}", faulting_address);
    println!("Error code       : {:?}", error_code);
    println!("\n{:#?}", stack_frame);

    crate::framebuffer::swap_buffers();
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    // This handler runs on IST stack 0 — safe even if the kernel stack overflowed.
    crate::serial_println!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);

    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(writer) = crate::framebuffer::FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.background_color = (170, 0, 0);
            writer.foreground_color = (255, 255, 255);
            writer.clear_screen();
        }
    });

    crate::println!("\n\n*** KERNEL PANIC - DOUBLE FAULT ***\n");
    crate::println!("A severe hardware exception has occurred.\n");
    crate::println!("{:#?}\n", stack_frame);
    crate::println!("JSOS is halting immediately.");

    crate::framebuffer::swap_buffers();
    loop {
        x86_64::instructions::hlt();
    }
}

/// Configure the Local APIC so PIC (8259) interrupts reach the CPU on real hardware.
///
/// On UEFI systems the I/O APIC is the default interrupt controller.  For the
/// legacy 8259 PIC to deliver IRQs (e.g. IRQ0 timer) the BSP's Local APIC
/// LINT0 pin must be set to **ExtINT** mode.  UEFI often leaves LINT0 in NMI
/// or disabled mode after ExitBootServices, so the PIT never ticks and TICKS
/// stays at 0 forever.
///
/// The LAPIC MMIO region (0xFEE00000) is not RAM and is NOT included in the
/// bootloader's physical-memory offset mapping.  We must explicitly map the
/// 4 KiB LAPIC page before touching it; otherwise the first volatile read
/// triggers a page fault on real hardware (QEMU maps more address space so
/// it goes unnoticed there).
pub fn init_lapic_lint0(
    mapper: &mut impl x86_64::structures::paging::Mapper<x86_64::structures::paging::Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<x86_64::structures::paging::Size4KiB>,
) {
    use core::sync::atomic::Ordering;
    use x86_64::registers::model_specific::Msr;
    use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame};
    use x86_64::{PhysAddr, VirtAddr};

    // IA32_APIC_BASE MSR: bit 10 = EXTD (x2APIC enabled), bits 12+ = physical base.
    let apic_base_msr = unsafe { Msr::new(0x1B).read() };
    let x2apic_mode = (apic_base_msr >> 10) & 1 == 1;

    if x2apic_mode {
        // ── x2APIC mode (OVMF/UEFI on modern hardware) ───────────────────
        // LAPIC registers are accessed via MSRs 0x800–0x8FF, not MMIO.
        unsafe {
            // SVR (MSR 0x80F): ensure APIC software-enable bit is set.
            let svr = Msr::new(0x80F).read();
            Msr::new(0x80F).write(svr | 0x100);

            // LVT LINT0 (MSR 0x835): ExtINT, not masked.
            Msr::new(0x835).write(0x0000_0700);

            // LVT LINT1 (MSR 0x836): NMI, not masked.
            Msr::new(0x836).write(0x0000_0400);
        }

        // Use the LAPIC timer as the tick source.  This works on both UEFI and
        // BIOS boots, unlike the 8259 PIC IRQ0 path which UEFI breaks.
        let ticks_10ms = calibrate_lapic_timer_x2apic();
        unsafe {
            // DCR (MSR 0x83E): divide-by-1 (0xB).
            Msr::new(0x83E).write(0xB);
            // LVT Timer (MSR 0x832): periodic mode (bit 17), vector 32.
            Msr::new(0x832).write(0x0002_0020);
            // Initial Count (MSR 0x838): fires every ticks_10ms LAPIC cycles.
            Msr::new(0x838).write(ticks_10ms as u64);
        }
        // LAPIC_EOI_VIRT = 0 signals x2APIC MSR EOI path in the handler.
        TICKS_PER_SEC.store(100, Ordering::SeqCst);
        LAPIC_TIMER_ACTIVE.store(true, Ordering::SeqCst);
        unsafe { mask_pic_irq0(); }
        crate::serial_println!("[LAPIC] x2APIC — LAPIC timer active, {}t/10ms.", ticks_10ms);
        return;
    }

    // ── xAPIC mode (BIOS / SeaBIOS) — MMIO at phys_offset + apic_phys ───
    let apic_phys = apic_base_msr & 0xFFFF_FFFF_FFFF_F000;

    // Map the LAPIC frame.  QEMU maps the full address space so map_to returns
    // AlreadyMapped there; on real hardware we create the mapping here.
    let phys_offset = crate::memory::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    let apic_virt = phys_offset + apic_phys;

    let phys_frame = PhysFrame::containing_address(PhysAddr::new(apic_phys));
    let virt_page  = Page::containing_address(VirtAddr::new(apic_virt));

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE;

    unsafe {
        match mapper.map_to(virt_page, phys_frame, flags, frame_allocator) {
            Ok(flush) => flush.flush(),
            Err(_) => {
                crate::serial_println!("[LAPIC] MMIO page already mapped, reusing.");
            }
        }
    }

    unsafe {
        // SVR (offset 0x0F0): bit 8 = APIC software enable.
        let svr_ptr = (apic_virt + 0x0F0) as *mut u32;
        let svr = core::ptr::read_volatile(svr_ptr);
        core::ptr::write_volatile(svr_ptr, svr | 0x100);

        // LVT LINT0 (offset 0x350): ExtINT, not masked.
        let lint0_ptr = (apic_virt + 0x350) as *mut u32;
        core::ptr::write_volatile(lint0_ptr, 0x0000_0700);

        // LVT LINT1 (offset 0x360): NMI, not masked.
        let lint1_ptr = (apic_virt + 0x360) as *mut u32;
        core::ptr::write_volatile(lint1_ptr, 0x0000_0400);
    }

    let ticks_10ms = calibrate_lapic_timer_xapic(apic_virt);
    unsafe {
        // DCR (offset 0x3E0): divide-by-1.
        core::ptr::write_volatile((apic_virt + 0x3E0) as *mut u32, 0xB);
        // LVT Timer (offset 0x320): periodic mode (bit 17), vector 32.
        core::ptr::write_volatile((apic_virt + 0x320) as *mut u32, 0x0002_0020);
        // Initial Count (offset 0x380).
        core::ptr::write_volatile((apic_virt + 0x380) as *mut u32, ticks_10ms);
    }
    // Store the EOI register address (LAPIC base + 0x0B0) for the timer handler.
    LAPIC_EOI_VIRT.store(apic_virt + 0x0B0, Ordering::SeqCst);
    TICKS_PER_SEC.store(100, Ordering::SeqCst);
    LAPIC_TIMER_ACTIVE.store(true, Ordering::SeqCst);
    unsafe { mask_pic_irq0(); }
    crate::serial_println!("[LAPIC] xAPIC — MMIO {:#x}, LAPIC timer active, {}t/10ms.", apic_virt, ticks_10ms);
}

/// Mask PIC IRQ0 (timer) so the 8259 PIC doesn't also fire at vector 32
/// while the LAPIC timer is in use.  Harmless on UEFI (IRQ0 wasn't reaching
/// the CPU anyway); prevents double-ticks on SeaBIOS where it was.
unsafe fn mask_pic_irq0() {
    use x86_64::instructions::port::Port;
    let mut port: Port<u8> = Port::new(0x21); // Master PIC IMR
    let mask = port.read();
    port.write(mask | 0x01); // bit 0 = IRQ0
}

/// Read the CMOS RTC Update-In-Progress flag (bit 7 of register 0x0A).
/// UIP is set for ~1.5 ms once per second while the RTC updates its registers.
/// The trailing edge (1→0 transition) marks the exact start of a new second.
unsafe fn cmos_uip() -> bool {
    use x86_64::instructions::port::Port;
    let mut idx: Port<u8> = Port::new(0x70);
    let mut dat: Port<u8> = Port::new(0x71);
    idx.write(0x0A);
    (dat.read() & 0x80) != 0
}

/// Calibrate the LAPIC timer against the RTC crystal oscillator.
/// Measures LAPIC ticks over exactly one RTC second (crystal-accurate).
/// Returns ticks per 10 ms, or 0 on failure (caller falls back to PIT).
///
/// Uses the UIP flag (port 0x70/0x71 reg 0x0A bit 7): UIP pulses high for
/// ~1.5 ms each second.  We latch the LAPIC counter at two consecutive
/// falling edges (1→0 transitions) — exactly 1.000 000 s apart.
///
/// Each CMOS I/O read takes ~0.5–2 µs, so 2 000 000 iterations ≥ 1–4 s.
unsafe fn calibrate_lapic_rtc(
    set_lapic_max: &dyn Fn(),
    read_lapic_current: &dyn Fn() -> u32,
) -> u32 {
    crate::serial_println!("[LAPIC] Waiting for RTC second boundary...");

    // --- sync: find first falling edge of UIP ---
    // 1. Wait for UIP=1 (update starting), timeout ~2 s
    let mut uip_found = false;
    for _ in 0..2_000_000u64 {
        if cmos_uip() { uip_found = true; break; }
    }
    if !uip_found {
        crate::serial_println!("[LAPIC] RTC UIP never went high — skipping RTC calibration.");
        return 0;
    }
    // 2. Wait for UIP=0 (update done = exact second boundary), timeout ~10 ms
    for _ in 0..20_000u64 { if !cmos_uip() { break; } }

    // --- start measurement ---
    set_lapic_max();

    // --- wait for next falling edge (~1 second later) ---
    for _ in 0..2_000_000u64 { if cmos_uip()  { break; } }
    for _ in 0..20_000u64    { if !cmos_uip() { break; } }

    let current = read_lapic_current();
    let ticks_per_sec = 0xFFFF_FFFFu32.wrapping_sub(current);
    crate::serial_println!("[LAPIC] RTC calibration: {} ticks/sec = {} ticks/10ms",
        ticks_per_sec, ticks_per_sec / 100);

    // Sanity: LAPIC bus frequency 1 MHz–2 GHz
    if ticks_per_sec < 1_000_000 || ticks_per_sec > 2_000_000_000 {
        crate::serial_println!("[LAPIC] RTC result out of range — falling back to PIT.");
        return 0;
    }
    ticks_per_sec / 100
}

/// Use PIT channel 2 as a polled reference to measure how many LAPIC timer
/// ticks occur in ~10 ms.  Channel 2 output is readable on port 0x61 bit 5
/// without needing interrupts, so this works before the IDT is live.
///
/// If the PIT doesn't respond within the spin limit (e.g. very slow bus or
/// emulator quirk) we fall back to 1 000 000 ticks, which is ~10 ms on a
/// ~100 MHz LAPIC bus — close enough for a wall-clock display.
/// Run a single 10 ms PIT-gated LAPIC measurement.  Returns the number of
/// LAPIC cycles elapsed, or 0 on PIT timeout (caller should discard).
unsafe fn lapic_measure_xapic(apic_virt: u64) -> u32 {
    use x86_64::instructions::port::Port;

    core::ptr::write_volatile((apic_virt + 0x3E0) as *mut u32, 0xB);
    core::ptr::write_volatile((apic_virt + 0x320) as *mut u32, 0x0001_0020);
    core::ptr::write_volatile((apic_virt + 0x380) as *mut u32, 0xFFFF_FFFF);

    let mut p61: Port<u8> = Port::new(0x61);
    let old61 = p61.read();
    // Gate off, then on — ensures a clean start even if OUT was already high.
    p61.write(old61 & !0x01);
    let mut cmd: Port<u8> = Port::new(0x43);
    cmd.write(0xB0); // ch2, lsb/msb, mode 0, binary
    let mut data: Port<u8> = Port::new(0x42);
    data.write(0x9C);
    data.write(0x2E);
    p61.write((old61 & !0x02) | 0x01); // gate on, speaker off

    let mut spin: u64 = 0;
    loop {
        if p61.read() & 0x20 != 0 { break; }
        core::hint::spin_loop();
        spin += 1;
        if spin > 50_000_000 { p61.write(old61); return 0; }
    }
    p61.write(old61);

    let current = core::ptr::read_volatile((apic_virt + 0x390) as *const u32);
    0xFFFF_FFFFu32.wrapping_sub(current)
}

fn calibrate_lapic_timer_xapic(apic_virt: u64) -> u32 {
    // Primary: calibrate against RTC crystal (immune to SMI inflation).
    let rtc = unsafe { calibrate_lapic_rtc(
        &|| unsafe {
            core::ptr::write_volatile((apic_virt + 0x3E0) as *mut u32, 0xB);
            core::ptr::write_volatile((apic_virt + 0x320) as *mut u32, 0x0001_0020);
            core::ptr::write_volatile((apic_virt + 0x380) as *mut u32, 0xFFFF_FFFF);
        },
        &|| unsafe { core::ptr::read_volatile((apic_virt + 0x390) as *const u32) },
    )};
    if rtc > 0 { return rtc; }

    // Fallback: PIT-gated measurement (3 runs, minimum).
    crate::serial_println!("[LAPIC] Using PIT fallback calibration.");
    let mut best = u32::MAX;
    for _ in 0..3 {
        let m = unsafe { lapic_measure_xapic(apic_virt) };
        if m > 1000 && m < best { best = m; }
    }
    if best == u32::MAX {
        crate::serial_println!("[LAPIC] xAPIC PIT calibration failed — using default 1M ticks/10ms.");
        1_000_000
    } else {
        best
    }
}

unsafe fn lapic_measure_x2apic() -> u32 {
    use x86_64::instructions::port::Port;
    use x86_64::registers::model_specific::Msr;

    Msr::new(0x83E).write(0xB);
    Msr::new(0x832).write(0x0001_0020);
    Msr::new(0x838).write(0xFFFF_FFFF);

    let mut p61: Port<u8> = Port::new(0x61);
    let old61 = p61.read();
    p61.write(old61 & !0x01); // gate off for clean start
    let mut cmd: Port<u8> = Port::new(0x43);
    cmd.write(0xB0);
    let mut data: Port<u8> = Port::new(0x42);
    data.write(0x9C);
    data.write(0x2E);
    p61.write((old61 & !0x02) | 0x01); // gate on, speaker off

    let mut spin: u64 = 0;
    loop {
        if p61.read() & 0x20 != 0 { break; }
        core::hint::spin_loop();
        spin += 1;
        if spin > 50_000_000 { p61.write(old61); return 0; }
    }
    p61.write(old61);

    let current = (Msr::new(0x839).read() & 0xFFFF_FFFF) as u32;
    0xFFFF_FFFFu32.wrapping_sub(current)
}

/// Same calibration as above but using x2APIC MSRs.
fn calibrate_lapic_timer_x2apic() -> u32 {
    use x86_64::registers::model_specific::Msr;

    // Primary: RTC crystal calibration.
    let rtc = unsafe { calibrate_lapic_rtc(
        &|| unsafe {
            Msr::new(0x83E).write(0xB);
            Msr::new(0x832).write(0x0001_0020);
            Msr::new(0x838).write(0xFFFF_FFFF);
        },
        &|| unsafe { (Msr::new(0x839).read() & 0xFFFF_FFFF) as u32 },
    )};
    if rtc > 0 { return rtc; }

    // Fallback: PIT.
    crate::serial_println!("[LAPIC] Using PIT fallback calibration (x2APIC).");
    let mut best = u32::MAX;
    for _ in 0..3 {
        let m = unsafe { lapic_measure_x2apic() };
        if m > 1000 && m < best { best = m; }
    }
    if best == u32::MAX {
        crate::serial_println!("[LAPIC] x2APIC PIT calibration failed — using default 1M ticks/10ms.");
        1_000_000
    } else {
        best
    }
}

// ── Hardware interrupt handlers ───────────────────────────────────────────

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    TICKS.fetch_add(1, Ordering::Relaxed);

    if LAPIC_TIMER_ACTIVE.load(Ordering::Relaxed) {
        // LAPIC timer generated this interrupt — send EOI to the LAPIC, not the PIC.
        let eoi_virt = LAPIC_EOI_VIRT.load(Ordering::Relaxed);
        unsafe {
            if eoi_virt != 0 {
                // xAPIC mode: write 0 to the LAPIC EOI MMIO register.
                core::ptr::write_volatile(eoi_virt as *mut u32, 0);
            } else {
                // x2APIC mode: write 0 to MSR 0x80B (IA32_X2APIC_EOI).
                x86_64::registers::model_specific::Msr::new(0x80B).write(0);
            }
        }
    } else {
        unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8()); }
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let scancode: u8 = unsafe { Port::new(0x60u16).read() };
    let _ = crate::keyboard::push_scancode(scancode);

    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8()); }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let status: u8 = unsafe { Port::new(0x64u16).read() };

    // Bit 0: output buffer has data.
    // Bit 5: data is from the mouse aux device (not the keyboard).
    // Only read if BOTH are set — otherwise we'd steal keyboard scancodes.
    if status & 0x01 != 0 && status & 0x20 != 0 {
        let byte: u8 = unsafe { Port::new(0x60u16).read() };
        crate::mouse::push_mouse_byte(byte);
    }

    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::Mouse.as_u8()); }
}
