#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;
use bootloader::{entry_point, BootInfo};
use os::allocator;
use core::sync::atomic::Ordering;

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
fn test_spawn_process_lifecycle() {
    let script = "globalThis.x = 42; console.log('spawn boot');";
    
    // Test that process gets spawned correctly into PROCESS_LIST
    let pid = os::process::spawn_process("test_app.js", script);
    
    {
        let list = os::process::PROCESS_LIST.lock();
        assert!(list.contains_key(&pid));
        let process = list.get(&pid).unwrap();
        assert_eq!(process.name, "test_app.js");
        assert_eq!(process.pid, pid);
    }
    
    // Test that the active foreground process shifted to this new application!
    let foreground = os::process::ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst);
    assert_eq!(foreground, pid);
}

#[test_case]
fn test_sandbox_isolation() {
    let pid1 = os::process::spawn_process("app1.js", "globalThis.secret = 10;");
    let pid2 = os::process::spawn_process("app2.js", "globalThis.secret = 20;");
    
    // Grab the sandboxes entirely independently
    let (sandbox1_arc, sandbox2_arc) = {
        let list = os::process::PROCESS_LIST.lock();
        (list.get(&pid1).unwrap().sandbox.clone(), list.get(&pid2).unwrap().sandbox.clone())
    };
    
    // Evaluate memory scopes. They should not leak variables!
    let val1 = sandbox1_arc.lock().eval("globalThis.secret");
    let val2 = sandbox2_arc.lock().eval("globalThis.secret");
    
    assert_eq!(val1.unwrap(), "10");
    assert_eq!(val2.unwrap(), "20");
}

#[test_case]
fn test_ipc_queue_receives_message() {
    let pid = os::process::spawn_process("ipc_test.js", "");
    {
        let list = os::process::PROCESS_LIST.lock();
        let proc = list.get(&pid).unwrap();
        proc.ipc_queue.lock().push(alloc::string::String::from("hello_ipc"));
    }
    let list = os::process::PROCESS_LIST.lock();
    let q = list.get(&pid).unwrap().ipc_queue.lock();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0], "hello_ipc");
}

#[test_case]
fn test_ipc_queue_multiple_messages_ordered() {
    let pid = os::process::spawn_process("ipc_order.js", "");
    {
        let list = os::process::PROCESS_LIST.lock();
        let proc = list.get(&pid).unwrap();
        let mut q = proc.ipc_queue.lock();
        q.push(alloc::string::String::from("msg_a"));
        q.push(alloc::string::String::from("msg_b"));
        q.push(alloc::string::String::from("msg_c"));
    }
    let list = os::process::PROCESS_LIST.lock();
    let q = list.get(&pid).unwrap().ipc_queue.lock();
    assert_eq!(q.len(), 3);
    assert_eq!(q[0], "msg_a");
    assert_eq!(q[1], "msg_b");
    assert_eq!(q[2], "msg_c");
}

#[test_case]
fn test_process_dead_flag_on_kill() {
    let pid = os::process::spawn_process("dead_flag_test.js", "");
    {
        let list = os::process::PROCESS_LIST.lock();
        assert!(!list.get(&pid).unwrap().dead, "process should start alive");
    }
    os::process::kill_process_and_cleanup(pid);
    {
        let list = os::process::PROCESS_LIST.lock();
        // kill_process_and_cleanup sets dead=true; reap_dead_processes removes it later
        assert!(list.get(&pid).unwrap().dead, "process should be marked dead after kill");
    }
}

#[test_case]
fn test_window_cleanup_on_process_kill() {
    use os::js_runtime::WINDOW_BUFFERS;

    let pid = os::process::spawn_process(
        "win_cleanup.js",
        "os.window.create(0, 0, 50, 50, 0);",
    );

    // The window should now be in WINDOW_BUFFERS owned by this pid
    let owned_before: alloc::vec::Vec<u32> = {
        WINDOW_BUFFERS.lock()
            .iter()
            .filter(|(_, w)| w.owner_pid == pid)
            .map(|(id, _)| *id)
            .collect()
    };
    assert!(!owned_before.is_empty(), "process should own at least one window before kill");

    os::process::kill_process(pid);

    let owned_after: usize = WINDOW_BUFFERS.lock()
        .iter()
        .filter(|(_, w)| w.owner_pid == pid)
        .count();
    assert_eq!(owned_after, 0, "all windows should be removed after process is killed");
}

#[test_case]
fn test_kill_process_and_fallback() {
    let starting_pid = os::process::NEXT_PID.load(Ordering::SeqCst);
    os::process::ACTIVE_FOREGROUND_PID.store(starting_pid, Ordering::SeqCst);
    
    let pid = os::process::spawn_process("kill_test.js", "1+1");
    // Assert active is now the new app
    assert_eq!(os::process::ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst), pid);
    
    // Kill the app!
    os::process::kill_process(pid);
    
    let list = os::process::PROCESS_LIST.lock();
    assert!(!list.contains_key(&pid));
    
    // Active foreground should safely fallback to PID 1 (explorer.js / shell.js)
    assert_eq!(os::process::ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst), 1);
}
