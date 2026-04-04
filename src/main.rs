#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(os::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
extern crate alloc;

use bootloader::{entry_point, BootInfo};

use os::{allocator, framebuffer, graphics::Graphics, interrupts, iommu, memory, println, serial_println, sse, serial::primitive_serial_print};

#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    unsafe {
        primitive_serial_print("\n\n!!! KERNEL PANIC !!!\n");
        primitive_serial_print("A panic occurred in the kernel. Check serial for details.\n");
    }

    serial_println!("PANIC: {}", info);

    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(writer) = os::framebuffer::FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.background_color = (0, 0, 170);
            writer.foreground_color = (255, 255, 255);
            writer.clear_screen();
        }
    });

    println!("\n\n*** JAVASCRIPT OPERATING SYSTEM - CRITICAL FAULT ***\n\n");
    println!("A fatal error has occurred and JSOS has been halted.\n\n");
    println!("{}\n\n", info);
    println!("If this is the first time you've seen this stop error screen, restart your computer.");
    println!("If this screen appears again, check for newly installed hardware or software.");

    os::framebuffer::swap_buffers();
    loop {
        x86_64::instructions::hlt();
    }
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    os::test_panic_handler(info)
}

getrandom::register_custom_getrandom!(getrandom_custom);

pub fn getrandom_custom(dest: &mut [u8]) -> Result<(), getrandom::Error> {
    // Use RDRAND — the hardware RNG built into every x86-64 CPU since Ivy Bridge.
    // We run with -cpu host in QEMU so RDRAND is always available.
    // The loop retries on the rare occasion RDRAND returns a transient failure.
    use core::arch::x86_64::_rdrand64_step;

    let mut i = 0;
    while i < dest.len() {
        let mut val: u64 = 0;
        let success = unsafe { _rdrand64_step(&mut val) };
        if success == 1 {
            for &b in val.to_ne_bytes().iter().take(dest.len() - i) {
                dest[i] = b;
                i += 1;
            }
        }
        // success == 0 means the hardware buffer was momentarily empty; just retry.
    }
    Ok(())
}

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    sse::enable_sse();
    serial_println!("Kernel started!");

    let phys_offset_u64 = boot_info.physical_memory_offset.into_option().unwrap();
    let phys_mem_offset = x86_64::VirtAddr::new(phys_offset_u64);

    memory::PHYS_MEM_OFFSET.store(phys_offset_u64, core::sync::atomic::Ordering::Relaxed);

    let mut mapper = unsafe { memory::init(phys_mem_offset) };
    serial_println!("Memory mapped");
    let mut frame_allocator =
        unsafe { allocator::BootInfoFrameAllocator::init(&boot_info.memory_regions) };
    serial_println!("Frame allocator constructed");
    allocator::init_heap(&mut mapper, &mut frame_allocator).expect("heap initialization failed");
    serial_println!("Heap init complete");

    // Disable Intel VT-d / AMD IOMMU translations so DMA-capable devices
    // (xHCI, etc.) can write to kernel memory without IOMMU domain setup.
    let rsdp_addr = boot_info.rsdp_addr.into_option().unwrap_or(0);
    os::power::RSDP_PHYS.store(rsdp_addr, core::sync::atomic::Ordering::Relaxed);
    iommu::disable_vtd(rsdp_addr, &mut mapper, &mut frame_allocator);
    serial_println!("IOMMU check complete.");

    // Compute total physical RAM from boot info memory regions
    {
        use bootloader::boot_info::MemoryRegionKind;
        let total: u64 = boot_info.memory_regions.iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.end - r.start)
            .sum();
        os::sysinfo::TOTAL_PHYS_RAM.store(total, core::sync::atomic::Ordering::Relaxed);
        serial_println!("Total physical RAM: {} MiB", total / (1024 * 1024));
    }

    if let Some(framebuffer) = boot_info.framebuffer.as_mut() {
        let info = framebuffer.info();
        framebuffer::init(framebuffer.buffer_mut(), info);
    }
    serial_println!("Framebuffer initialized.");

    let (screen_w, screen_h) = framebuffer::get_resolution();
    Graphics::fill_rect(0, 0, screen_w, screen_h, 20, 30, 40);
    Graphics::fill_rect(0, screen_h.saturating_sub(30), screen_w, 30, 15, 15, 15);
    framebuffer::save_bg();

    Graphics::set_foreground_color(100, 200, 255);
    println!("Welcome to JSOS!\nA JavaScript-Oriented bare-metal operating system!\n");
    println!("Type 'help' to see what you can do.\n");
    Graphics::set_foreground_color(220, 220, 220);

    os::net::init();

    // GDT must be loaded before IDT — the IDT double fault entry references
    // the TSS which is registered in the GDT.
    os::gdt::init();
    serial_println!("GDT/TSS loaded.");

    interrupts::init_idt();
    unsafe { interrupts::PICS.lock().initialize() };
    interrupts::unmask_irqs();
    interrupts::init_lapic_lint0(&mut mapper, &mut frame_allocator);

    serial_println!("IDT loaded, PIC initialized, LAPIC LINT0 configured.");

    os::mouse::MOUSE.lock().init();

    // Set up JSKV persistent storage driver
    os::storage::init();

    serial_println!("[main] starting xhci::init");
    // Initialize USB xHCI controllers
    os::xhci::init(&mut mapper, &mut frame_allocator);
    serial_println!("[main] xhci::init done");


    // Seed built-in binaries into persistent storage so they can be spawned
    // dynamically and user scripts can live alongside them.
    // Always overwrites to keep built-ins in sync with the compiled kernel.
    {
        const BUILTIN_BINS: &[(&str, &str)] = &[
            ("shell.jsos",        include_str!("jsos/shell.jsos")),
            ("node.jsos",         include_str!("jsos/node.jsos")),
            ("snake.jsos",        include_str!("jsos/snake.jsos")),
            ("winman.jsos",       include_str!("jsos/winman.jsos")),
            ("demo_browser.jsos", include_str!("jsos/demo_browser.jsos")),
            ("webremote.jsos",    include_str!("jsos/webremote.jsos")),
            ("sysman.jsos",       include_str!("jsos/sysman.jsos")),
            ("drawtest.jsos",     include_str!("jsos/drawtest.jsos")),
            ("pixeldemo.jsos",    include_str!("jsos/pixeldemo.jsos")),
            ("imageview.jsos",    include_str!("jsos/imageview.jsos")),
            ("seriallog.jsos",    include_str!("jsos/seriallog.jsos")),
            ("canvastest.jsos",   include_str!("jsos/canvastest.jsos")),
            ("stress_test.jsos",  include_str!("jsos/stress_test.jsos")),
            ("libjsos.js",        include_str!("js/libjsos.js")),
        ];
        for (name, source) in BUILTIN_BINS {
            os::storage::write_object(name, source.as_bytes());
        }

        const BUILTIN_BINARIES: &[(&str, &[u8])] = &[
            ("gallery1.png", include_bytes!("gallery1.png")),
            ("gallery2.bmp", include_bytes!("gallery2.bmp")),
            ("gallery3.jpg", include_bytes!("gallery3.jpg")),
        ];
        for (name, source) in BUILTIN_BINARIES {
            os::storage::write_object(name, source);
        }
        serial_println!("[main] Built-in binaries seeded to storage");
    }

    serial_println!("[main] starting spawn_process");
    os::process::spawn_process("winman.jsos", include_str!("jsos/winman.jsos"));
    os::process::spawn_process("seriallog.jsos", include_str!("jsos/seriallog.jsos"));
    serial_println!("[main] spawn_process returned");

    os::shell::init();
    serial_println!("[main] shell::init returned");

    framebuffer::swap_buffers();
    serial_println!("[main] first frame pushed to display");

    x86_64::instructions::interrupts::enable();
    serial_println!("[main] interrupts enabled - entering main loop");

    loop {
        os::keyboard::process_keyboard_queue();
        os::mouse::process_mouse_queue();
        os::xhci::poll_usb_devices();
        os::net::poll_network();
        os::js_runtime::poll_timers();
        os::process::poll_processes();
        os::process::reap_dead_processes();


        os::js_runtime::draw_all_decorations();
        os::js_runtime::draw_notification_overlay();
        os::js_runtime::draw_cursor_overlay();
        framebuffer::swap_buffers();

        let has_active_fetches = !os::net::FETCH_JOBS.lock().is_empty();
        let has_active_servers = !os::net::SERVER_JOBS.lock().is_empty();
        let has_active_ws = !os::net::WEBSOCKET_JOBS.lock().is_empty();
        if !has_active_fetches && !has_active_servers && !has_active_ws {
            x86_64::instructions::hlt();
        }
    }
}
