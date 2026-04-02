#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{entry_point, BootInfo};
use core::panic::PanicInfo;
use os::{println, serial_println};

entry_point!(test_kernel_main);

fn test_kernel_main(_boot_info: &'static mut BootInfo) -> ! {
    serial_println!("basic_boot entry point reached!");
    test_main();
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os::test_panic_handler(info)
}

#[test_case]
fn test_println() {
    println!("Testing println macro via serial and framebuffer!");
}
