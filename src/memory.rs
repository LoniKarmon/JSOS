// src/memory.rs

use x86_64::{
    structures::paging::{OffsetPageTable, PageTable},
    VirtAddr,
};
use core::sync::atomic::{AtomicU64, Ordering};

pub static PHYS_MEM_OFFSET: AtomicU64 = AtomicU64::new(0);

/// Translate a virtual address to its 32-bit physical address for DMA use.
///
/// This implementation walks the page tables directly using *shared* references,
/// avoiding the UB that arose from calling `init()` (which requires `&mut PageTable`)
/// multiple times and thus creating aliased mutable references.
pub fn virt_to_phys(virt: u64) -> u32 {
    let offset = PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    if offset == 0 {
        panic!("virt_to_phys called before PHYS_MEM_OFFSET initialized");
    }

    let phys_u64 = unsafe { walk_page_tables(virt, offset) };

    // RTL8139 is a 32-bit PCI device — DMA addresses must fit in 32 bits.
    assert!(
        phys_u64 <= u32::MAX as u64,
        "DMA address {:#x} is above 4 GiB — RTL8139 cannot reach it",
        phys_u64
    );

    phys_u64 as u32
}

/// Walk the 4-level page table hierarchy to translate `virt` → physical address.
///
/// Uses shared (`&`) references to each level's `PageTable`, so multiple
/// concurrent calls are sound — no aliased `&mut` is created.
unsafe fn walk_page_tables(virt: u64, phys_mem_offset: u64) -> u64 {
    use x86_64::registers::control::Cr3;
    use x86_64::structures::paging::page_table::PageTableEntry;

    let va = VirtAddr::new(virt);
    let phys_base = VirtAddr::new(phys_mem_offset);

    // Helper: given a physical frame address, get a shared ref to its PageTable.
    let table_at = |frame_phys: u64| -> &'static PageTable {
        let virt = phys_base + frame_phys;
        unsafe { &*virt.as_ptr::<PageTable>() }
    };

    // Helper: follow an entry to the next-level table, panicking on errors.
    let follow = |entry: &PageTableEntry, level: u8| -> &'static PageTable {
        if entry.is_unused() {
            panic!("virt_to_phys {:#x}: L{} entry is not present", virt, level);
        }
        let frame = entry.frame().unwrap_or_else(|_| {
            panic!("virt_to_phys {:#x}: L{} entry is a huge page (unsupported)", virt, level)
        });
        table_at(frame.start_address().as_u64())
    };

    let (l4_frame, _) = Cr3::read();
    let l4 = table_at(l4_frame.start_address().as_u64());
    let l3 = follow(&l4[va.p4_index()], 4);
    let l2 = follow(&l3[va.p3_index()], 3);
    let l1 = follow(&l2[va.p2_index()], 2);

    let l1_entry = &l1[va.p1_index()];
    if l1_entry.is_unused() {
        panic!("virt_to_phys {:#x}: L1 entry is not present", virt);
    }
    let frame = l1_entry.frame().unwrap_or_else(|_| {
        panic!("virt_to_phys {:#x}: L1 entry is a huge page (unsupported)", virt)
    });

    frame.start_address().as_u64() + u64::from(va.page_offset())
}

/// Initialize a new OffsetPageTable.
///
/// # Safety
/// Caller must guarantee that all physical memory is mapped at `physical_memory_offset`,
/// and that this function is called **exactly once**. A second call would alias the
/// `&mut PageTable` returned by `active_level_4_table`, which is undefined behaviour.
pub unsafe fn init(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let level_4_table = active_level_4_table(physical_memory_offset);
    OffsetPageTable::new(level_4_table, physical_memory_offset)
}

unsafe fn active_level_4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;
    let (l4_frame, _) = Cr3::read();
    let phys = l4_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    &mut *virt.as_mut_ptr::<PageTable>()
}
