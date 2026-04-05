#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]
#![feature(abi_x86_interrupt)]
#![feature(step_trait)]
#![feature(c_variadic)]

extern crate alloc;
extern crate log;

// Minimal logger that routes `log` crate output to serial for TLS debugging.
struct SerialLogger;
impl log::Log for SerialLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            serial_println!("[TLS {}] {}", record.level(), record.args());
        }
    }
    fn flush(&self) {}
}
static SERIAL_LOGGER: SerialLogger = SerialLogger;

pub fn init_logger() {
    log::set_logger(&SERIAL_LOGGER).ok();
    log::set_max_level(log::LevelFilter::Warn);
}

pub mod allocator;
pub mod gdt;
pub mod js_runtime;
pub mod bindings;
pub mod process;
pub mod framebuffer;
pub mod graphics;
pub mod fs;
pub mod pci;
pub mod net;
pub mod interrupts;
pub mod memory;
pub mod qemu_exit;
pub mod serial;
pub mod shell;
pub mod sse;
pub mod power;
pub mod mouse;
pub mod keyboard;
pub mod sysinfo;
pub mod ata;
pub mod storage;
pub mod xhci;
pub mod image;
pub mod iommu;
pub mod canvas;

use core::panic::PanicInfo;

pub trait Testable {
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    crate::qemu_exit::exit_qemu(crate::qemu_exit::QemuExitCode::Success);
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    crate::qemu_exit::exit_qemu(crate::qemu_exit::QemuExitCode::Failed);
    loop {}
}

#[cfg(test)]
use bootloader::{entry_point, BootInfo};

#[cfg(test)]
entry_point!(test_kernel_main);

#[cfg(test)]
fn test_kernel_main(_boot_info: &'static mut BootInfo) -> ! {
    test_main();
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    test_panic_handler(info)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
