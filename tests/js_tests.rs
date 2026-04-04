#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use os::js_runtime::QuickJsSandbox;
use os::allocator;

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
fn test_js_evaluation() {
    let mut sandbox = QuickJsSandbox::new().expect("Failed to create Sandbox");
    let res = sandbox.eval("1 + 1");
    assert_eq!(res.unwrap(), "2");
}

#[test_case]
fn test_js_string_concatenation() {
    let mut sandbox = QuickJsSandbox::new().unwrap();
    let res = sandbox.eval("'Hello ' + 'World'");
    assert_eq!(res.unwrap(), "Hello World");
}

#[test_case]
fn test_js_os_bindings_exist() {
    let mut sandbox = QuickJsSandbox::new().unwrap();
    let typeof_spawn = sandbox.eval("typeof os.spawn");
    assert_eq!(typeof_spawn.unwrap(), "function");

    let typeof_graphics = sandbox.eval("typeof os.graphics.fillRect");
    assert_eq!(typeof_graphics.unwrap(), "function");
}

#[test_case]
fn test_js_all_os_subobjects_present() {
    let mut sb = QuickJsSandbox::new().unwrap();
    for api in &[
        "os.graphics", "os.store", "os.window", "os.net",
        "os.mouse", "os.clipboard", "os.base64", "os.websocket",
    ] {
        let result = sb.eval(&alloc::format!("typeof {}", api));
        assert_eq!(result.unwrap(), "object", "{} should be an object", api);
    }
    for func in &[
        "os.spawn", "os.fetch", "os.exit", "os.exec",
        "os.listBin", "os.processes", "os.sendIpc", "os.uptime",
        "os.reboot", "os.shutdown", "os.sysinfo", "os.rtc",
    ] {
        let result = sb.eval(&alloc::format!("typeof {}", func));
        assert_eq!(result.unwrap(), "function", "{} should be a function", func);
    }
}

#[test_case]
fn test_js_base64_encode_known_value() {
    let mut sb = QuickJsSandbox::new().unwrap();
    // "hello" in base64 is always "aGVsbG8="
    let encoded = sb.eval("os.base64.encode('hello')");
    assert_eq!(encoded.unwrap(), "aGVsbG8=");
}

#[test_case]
fn test_js_base64_round_trip() {
    let mut sb = QuickJsSandbox::new().unwrap();
    let result = sb.eval("os.base64.decode(os.base64.encode('JSOS kernel test 123!'))");
    assert_eq!(result.unwrap(), "JSOS kernel test 123!");
}

#[test_case]
fn test_js_base64_empty_string() {
    let mut sb = QuickJsSandbox::new().unwrap();
    let encoded = sb.eval("os.base64.encode('')");
    let decoded = sb.eval("os.base64.decode('')");
    assert_eq!(encoded.unwrap(), "");
    assert_eq!(decoded.unwrap(), "");
}

#[test_case]
fn test_js_clipboard_write_then_read() {
    let mut sb = QuickJsSandbox::new().unwrap();
    sb.eval("os.clipboard.write('clipboard_test_value')").ok();
    let read = sb.eval("os.clipboard.read()");
    assert_eq!(read.unwrap(), "clipboard_test_value");
}

#[test_case]
fn test_js_clipboard_overwrite() {
    let mut sb = QuickJsSandbox::new().unwrap();
    sb.eval("os.clipboard.write('first')").ok();
    sb.eval("os.clipboard.write('second')").ok();
    let read = sb.eval("os.clipboard.read()");
    assert_eq!(read.unwrap(), "second");
}

#[test_case]
fn test_js_listbin_is_json_array_with_known_apps() {
    let mut sb = QuickJsSandbox::new().unwrap();
    // Parse the JSON and check for known built-in binaries
    let has_shell  = sb.eval("JSON.parse(os.listBin()).includes('shell.jsos')");
    let has_node   = sb.eval("JSON.parse(os.listBin()).includes('node.jsos')");
    let has_winman = sb.eval("JSON.parse(os.listBin()).includes('winman.jsos')");
    assert_eq!(has_shell.unwrap(),  "true");
    assert_eq!(has_node.unwrap(),   "true");
    assert_eq!(has_winman.unwrap(), "true");
}

#[test_case]
fn test_js_window_create_inserts_buffer() {
    use os::js_runtime::WINDOW_BUFFERS;

    let before = WINDOW_BUFFERS.lock().len();
    let mut sb = QuickJsSandbox::new().unwrap();
    let win_id = sb.eval("os.window.create(10, 10, 100, 50, 0)");
    let id: u32 = win_id.unwrap().parse().expect("window id should be a number");
    assert!(id > 0, "window id should be non-zero");

    let present = WINDOW_BUFFERS.lock().contains_key(&id);
    assert!(present, "WINDOW_BUFFERS should contain the new window");
    assert_eq!(WINDOW_BUFFERS.lock().len(), before + 1);

    // Cleanup so we don't pollute other tests
    WINDOW_BUFFERS.lock().remove(&id);
}

#[test_case]
fn test_js_processes_returns_json_array() {
    let mut sb = QuickJsSandbox::new().unwrap();
    // Should be parseable JSON and be an array
    let is_array = sb.eval("Array.isArray(JSON.parse(os.processes()))");
    assert_eq!(is_array.unwrap(), "true");
}
