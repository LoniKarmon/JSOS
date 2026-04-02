use x86_64::registers::control::{Cr0, Cr4};

pub fn enable_sse() {
    unsafe {
        let mut cr0 = Cr0::read_raw();
        cr0 &= !(1 << 2); // clear EMULATE_COPROCESSOR
        cr0 |= 1 << 1; // set MONITOR_COPROCESSOR
        Cr0::write_raw(cr0);

        let mut cr4 = Cr4::read_raw();
        cr4 |= 1 << 9; // set OSFXSR
        cr4 |= 1 << 10; // set OSXMMEXCPT_ENABLE
        Cr4::write_raw(cr4);
    }
}
