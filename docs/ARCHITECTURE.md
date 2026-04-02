# JSOS Architecture Guide

This document is intended for developers and AI agents working on the JSOS codebase. It outlines the structural design, core components, data flow, and constraints of the JSOS operating system.

## 1. High-Level System Overview

JSOS is an experimental, bare-metal x86_64 operating system.
- **Implementation Language**: Core OS is written in Rust (`no_std`). Userspace applications are written in JavaScript.
- **JavaScript Engine**: Uses [QuickJS-NG](https://github.com/quickjs-ng/quickjs) compiled as a freestanding C library (`libquickjs.a`) and linked via FFI.
- **Execution Model**: The OS operates entirely in kernel space (Ring 0). JS applications run as isolated `QuickJsSandbox` instances within the kernel, providing logical isolation instead of hardware ring isolation.

## 2. Core Subsystems

### 2.1 Boot and Main Loop (`src/main.rs`)
- **Boot Sequence**: `kernel_main` is the entry point. It initializes hardware subsystems in a strict order:
  1. SSE (SIMD instructions)
  2. Memory allocator (Kernel Heap)
  3. Framebuffer (VGA)
  4. Network interface (RTL8139)
  5. GDT (Global Descriptor Table)
  6. IDT & PIC (Interrupts)
  7. Storage (ATA disk)
  8. Built-in binary seeding (all `.jsos` scripts written to JSKV)
- **Init Process**: After initialization, it spawns `winman.jsos` as the first process (PID 1).
- **Event Loop**: JSOS is strictly event-driven. The main loop polls hardware (keyboard, mouse, network, timers) and processes (JS events), then draws overlay layers (crash notifications, cursor) and swaps the graphics framebuffers. It halts the CPU (`hlt`) only when no network I/O is pending.

### 2.2 Process & Sandbox Model (`src/process.rs`)
- **Isolation**: Each JS application runs within its own `QuickJsSandbox`. A process consists of a PID, a name, the JS sandbox, and an IPC message queue.
- **Scheduling**: Driven by `poll_processes()` which executes pending JS events/promises for each process every frame. The foreground process is always run last so it gets the final say on the rendered frame.
- **Focus**: The foreground process receives keyboard input. The window manager (`winman.jsos`) receives global mouse events.
- **Error Isolation**: `eval()` and `execute_pending_jobs()` return `Result<_, String>`. An uncaught JS exception calls `crash_process()`, which kills only the offending process and pushes a kernel overlay notification. The kernel never panics due to a JS error.
- **Auto-Exit**: After a process's initial `eval()` succeeds, the kernel checks whether it registered any windows or timers. If neither is present, the process is immediately killed — preventing ghost processes from scripts that run to completion without an event loop.
- **Crash Notifications**: `crash_process()` calls `push_notification(name, error)` in `js_runtime.rs`, which stores the message with a tick-based TTL. `draw_notification_overlay()` renders active toasts directly to the framebuffer after all process rendering, ensuring they are always visible on top.

### 2.3 JavaScript Runtime & FFI (`src/js_runtime.rs`)
- The bridge between Rust kernel space and JS userspace.
- Exposes all kernel capabilities (graphics, network, disk) via the global `os.*` namespace.
- **Memory Management**: Uses QuickJS reference counting (`JS_FreeValue`, `JS_DupValue`). Mismanagement of pointers across the FFI boundary causes kernel panics.
- See `API.md` for a full list of exposed `os.*` bindings.

### 2.4 Graphics & Compositing (`src/framebuffer.rs`, `src/graphics.rs`)
- **Double Buffering**: Operations draw to a back buffer, which is copied to the front (display) buffer once per frame via `swap_buffers()`.
- **Text Rendering**: Uses u8g2 bitmap fonts.
- **Drawing Primitives**: Bresenham's line algorithm (`draw_line`), midpoint circle algorithm (`draw_circle`, `fill_circle`) implemented in `framebuffer.rs` and exposed to JS via `os.window.drawLine`, `os.window.drawCircle`, `os.window.fillCircle`.
- **Kernel Overlays**: After all JS process rendering, the kernel draws overlay layers in order: (1) crash notifications (`draw_notification_overlay`), (2) hardware cursor (`draw_cursor_overlay`). These are drawn last and are always on top of everything.
- **JSKV Binary Store**: All built-in `.jsos` applications are seeded into the JSKV persistent disk at every boot. `os.spawn` resolves binaries from JSKV first, falling back to the in-memory BINS map. This allows user-written scripts to shadow or extend built-ins.

### 2.5 Networking (`src/net/mod.rs`, `src/net/rtl8139.rs`)
- **Driver**: Custom RTL8139 driver using DMA.
- **Stack**: Uses `smoltcp` for the TCP/IP stack.
- **Capabilities**: Supports raw TCP listening, HTTP(S) fetching (via `embedded-tls`), and WebSocket connections. Tracked via global `Mutex` arrays.

### 2.6 Persistent Storage (`src/storage.rs`)
- **Format**: Custom "JSKV" format on a 64MB ATA disk (QEMU's second drive: `jskv.img`).
- **Structure**: FAT-style allocation with a 128-key Master File Table (MFT). Key-Value based operations exposed to JS via `os.store.*`.

## 3. Important Design Constraints

When modifying the system, adhere to these constraints:

1. **`no_std` Compliance**: The Rust core has no standard library access. Use the `alloc` crate for collections (`Vec`, `String`, etc.). POSIX/C functions required by QuickJS are mocked in `src/bindings/mod.rs` and `quickjs/freestanding.c`.
2. **Single-threaded (No SMP)**: JSOS uses a single core.
   - Global state is protected by `spin::Mutex`.
   - Interrupts must be disabled during critical sections using `x86_64::instructions::interrupts::without_interrupts`.
3. **Unsafe FFI Boundaries**: Calling QuickJS C functions is `unsafe`. Ensure valid memory lifetimes, check return values for JS exceptions, and carefully manage JS_Value refcounts to prevent memory leaks or use-after-free panics.
4. **Toolchain specifics**: JSOS relies on a custom `x86_64` crate fork (`x86_64/`). Do not update this dependency blindly. Building QuickJS requires `clang`.

## 4. Agent / Developer Workflow: Extending the OS

### Adding a New OS Native API
1. Implement the Rust logic.
2. Define a C-compatible wrapper function exported with `#[no_mangle] pub unsafe extern "C" fn`.
3. In `src/js_runtime.rs`, wrap the C function logic.
4. Use `JS_NewCFunction` to map the wrapper to a JS variable.
5. Attach it to the `os` global object or one of its namespaces in `init_os_module`.
6. Update `API.md` to document the new binding.

### Adding a New Embedded JS Application
1. Create the `app_name.jsos` file in `src/jsos/`.
2. Add an entry to `BUILTIN_BINS` in `src/main.rs`:
   ```rust
   ("app_name.jsos", include_str!("jsos/app_name.jsos")),
   ```
3. Also add it to the fallback `BINS` map in `src/js_runtime.rs` (used if JSKV is unavailable).
4. The app will be automatically visible in `os.listBin()`, launchable via `os.spawn("app_name.jsos")`, and appear in the winman taskbar on next boot.
