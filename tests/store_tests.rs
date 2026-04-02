#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use os::fs::store::STORE;
use os::allocator;
use x86_64::VirtAddr;

entry_point!(main);

fn main(boot_info: &'static mut BootInfo) -> ! {
    // Initialize Memory allocation so BTreeMap and Vec survive in the test
    let phys_mem_offset =
        x86_64::VirtAddr::new(boot_info.physical_memory_offset.into_option().unwrap());
    let mut mapper = unsafe { os::memory::init(phys_mem_offset) };
    let mut frame_allocator =
        unsafe { allocator::BootInfoFrameAllocator::init(&boot_info.memory_regions) };
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");

    test_main();
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os::test_panic_handler(info)
}

#[test_case]
fn test_object_store_read_write() {
    let mut store = STORE.lock();
    let domain = store.get_domain("test_app");
    
    // Test Write
    domain.lock().set("my_var", alloc::vec![1, 2, 3]);
    
    // Test Read
    let val = domain.lock().get("my_var");
    assert_eq!(val, Some(alloc::vec![1, 2, 3]));
}

#[test_case]
fn test_object_store_delete() {
    let mut store = STORE.lock();
    let domain = store.get_domain("test_app");
    
    domain.lock().set("temp_var", alloc::vec![9, 9]);
    assert_eq!(domain.lock().delete("temp_var"), true);
    assert_eq!(domain.lock().get("temp_var"), None);
}

#[test_case]
fn test_ramfs_list_keys() {
    let mut store = STORE.lock();
    let domain = store.get_domain("test_list_keys");
    domain.lock().set("alpha", alloc::vec![1]);
    domain.lock().set("beta",  alloc::vec![2]);
    let keys = domain.lock().list();
    assert!(keys.contains(&alloc::string::String::from("alpha")));
    assert!(keys.contains(&alloc::string::String::from("beta")));
    assert_eq!(keys.len(), 2);
}

#[test_case]
fn test_ramfs_overwrite_key_returns_latest() {
    let mut store = STORE.lock();
    let domain = store.get_domain("test_overwrite");
    domain.lock().set("key", alloc::vec![1, 2, 3]);
    domain.lock().set("key", alloc::vec![9, 8, 7]);
    let val = domain.lock().get("key");
    assert_eq!(val, Some(alloc::vec![9, 8, 7]));
}

#[test_case]
fn test_ramfs_missing_key_returns_none() {
    let mut store = STORE.lock();
    let domain = store.get_domain("test_missing");
    let val = domain.lock().get("does_not_exist");
    assert_eq!(val, None);
}

#[test_case]
fn test_object_store_domain_isolation() {
    let mut store = STORE.lock();
    let domain_a = store.get_domain("isolation_a");
    let domain_b = store.get_domain("isolation_b");

    domain_a.lock().set("shared_key", alloc::vec![0xAA]);
    domain_b.lock().set("shared_key", alloc::vec![0xBB]);

    assert_eq!(domain_a.lock().get("shared_key"), Some(alloc::vec![0xAA]));
    assert_eq!(domain_b.lock().get("shared_key"), Some(alloc::vec![0xBB]));
}

#[test_case]
fn test_object_store_list_domains_contains_created() {
    let mut store = STORE.lock();
    store.get_domain("sentinel_domain_xyz");
    let domains = store.list_domains();
    assert!(domains.contains(&alloc::string::String::from("sentinel_domain_xyz")));
}

#[test_case]
fn test_object_store_quickjs_exec() {
    // Write a script into the system store
    {
        let mut store = STORE.lock();
        let domain = store.get_domain("system");
        let script = "console.log('hello from test');";
        domain.lock().set("my_script", alloc::vec::Vec::from(script.as_bytes()));
    }
    
    // Evaluate it through QuickJS, dropping the Sandbox immediately to catch leaks
    if let Some(mut sandbox) = os::js_runtime::QuickJsSandbox::new() {
        let result = sandbox.eval("os.exec('my_script')");
        assert_eq!(result, "undefined"); // Expect 'undefined' with properly mapped values
    } else {
        panic!("Failed to initialize QuickJsSandbox in test");
    }
}

