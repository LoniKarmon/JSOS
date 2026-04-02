// src/gdt.rs — Global Descriptor Table + Task State Segment
//
// Sets up a TSS with a dedicated IST stack for the double fault handler.
// This ensures the double fault handler has a clean stack even when the
// kernel stack has overflowed (which is the most common cause of double faults).

use lazy_static::lazy_static;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

/// IST slot 0 is used for the double fault handler emergency stack.
pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

/// 20 KiB emergency stack — enough for the double fault handler + BSOD rendering.
const DOUBLE_FAULT_STACK_SIZE: usize = 4096 * 5;

/// The emergency stack itself. Static so it lives for the lifetime of the kernel.
static mut DOUBLE_FAULT_STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            let stack_start = VirtAddr::from_ptr(unsafe { DOUBLE_FAULT_STACK.as_ptr() });
            stack_start + DOUBLE_FAULT_STACK_SIZE as u64
        };
        tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        // A kernel data segment is REQUIRED so SS points to a valid descriptor
        // in our GDT. Without it, the first iretq (returning from any interrupt)
        // validates SS against our new GDT, finds the bootloader's old selector
        // missing, fires a GPF -> double fault -> triple fault -> silent KVM reset.
        let data_selector = gdt.add_entry(Descriptor::kernel_data_segment());
        let tss_selector  = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, data_selector, tss_selector })
    };
}

/// Load the GDT and TSS. Must be called before `interrupts::init_idt()`.
pub fn init() {
    use x86_64::instructions::segmentation::{Segment, CS, SS};
    use x86_64::instructions::tables::load_tss;

    GDT.0.load();
    unsafe {
        // Reload CS with our kernel code selector.
        CS::set_reg(GDT.1.code_selector);
        // Reload SS with our kernel data selector.
        // iretq restores SS from the interrupt stack frame and validates it
        // against the current GDT. If SS is stale (pointing to the bootloader's
        // now-gone data segment), the very first interrupt after lgdt will
        // triple-fault silently under KVM.
        SS::set_reg(GDT.1.data_selector);
        // Load the TSS so the CPU knows where the IST emergency stacks live.
        load_tss(GDT.1.tss_selector);
    }
}