# JSOS Implementation Plan

This document outlines the planned features and improvements for JSOS, categorized by impact and complexity. Each item includes the estimated effort, reasoning, core mission, and definition of done (DOD).

---

## Completed

### AI Agent OS Engineering Toolchain ✓
- **Reasoning:** Defined structured workflows for Antigravity and Claude Code to enable continuous background testing and localized deep-dive refactoring.
- **Result:** `.agents` and `.claude` workspace directories configured to enforce `no_std`, FFI limits, and zero-allocation Rust logic.

### Crash Notification Overlay ✓
- **Reasoning:** Previous approach drew toast notifications on `desktopWin` which was always behind app windows in z-order, making them invisible.
- **Result:** Crash notifications are now a kernel-level overlay drawn directly onto the back buffer after all process rendering, before `swap_buffers()`. Guaranteed to appear on top of all windows. Toast shows ~5 seconds in the top-right corner. `push_notification()` / `draw_notification_overlay()` in `js_runtime.rs`, called from the main loop.

### Process Auto-Exit ✓
- **Reasoning:** Scripts that ran to completion without calling `os.exit()` and without registering any event handlers stayed alive as ghost processes with zombie windows.
- **Result:** After the initial `eval()` in `spawn_process()`, the kernel checks `process_has_windows(pid)` and `process_has_timers(pid)`. If neither is true, the process is automatically killed. Apps with a proper event loop (window + on_key/on_ipc/timers) are unaffected.

### Process Error Isolation ✓
- **Reasoning:** `eval()` now returns `Result<String, String>`; `execute_pending_jobs()` returns `Result<(), String>`. All event dispatch sites in `poll_processes()`, `poll_timers()`, and `spawn_process()` check for errors.
- **Result:** An uncaught JS exception kills only the offending process via `crash_process()`, which pushes a kernel overlay notification. The kernel keeps running.

### Built-in Binaries Seeded to JSKV ✓
- **Reasoning:** User scripts need to live alongside built-in apps in the same namespace; `os.spawn` needed a unified lookup path supporting user overrides.
- **Result:** All built-in `.jsos` scripts are written to the JSKV disk at every boot in `main.rs`. `os.spawn` reads from JSKV first (allowing user-supplied scripts to shadow built-ins), falling back to the in-memory BINS map. `os.listBin` lists `.jsos` keys from JSKV. The `ls` command filters `.jsos` keys from the `[Objects]` section to avoid duplicates.

### Window Drawing Primitives ✓
- **Reasoning:** Bresenham's line and midpoint circle algorithms added to `framebuffer.rs`.
- **Result:** `os.window.drawLine`, `os.window.drawCircle`, and `os.window.fillCircle` fully implemented and exposed to JS. Tested via `drawtest.jsos`.

### Persistent Storage ✓
- **Reasoning:** ATA driver + custom JSKV format on a 64MB second disk. 128-key MFT, survives reboots.
- **Result:** `os.store.set/get/list/delete` fully implemented in `src/storage.rs`. Data persists across reboots via `jskv.img`.

### HTTP Server ✓
- **Reasoning:** TCP listener built on smoltcp in `net/mod.rs`. Request parser and static file serving implemented.
- **Result:** `os.net.listen` and `os.net.serveStatic` work. Accessible from the host network.

### ES Module Support (partial) ✓
- **Reasoning:** QuickJS module loader wired up using `os.store` as the resolution backend.
- **Result:** Modules stored via `os.store` can be imported. Full `import` from file paths pending a proper VFS.

---

## High Impact & Self-Contained Plans

### Build Warning Zero-Tolerance
- **Estimate:** < 1 day
- **Reasoning:** `cargo check` and QuickJS C files currently emit numerous `unused_mut`, `dead_code` (`Superblock`), and unused parameter warnings.
- **Mission:** Enforce strict style guidelines to keep the bare-metal kernel fully clean and deterministic.
- **DOD:** `cargo build --target x86_64-os.json` compiles with `0 warnings emitted`.

### Better Fonts (Bitmap Atlas / TrueType) ✓
- **Estimate:** 2–3 days
- **Reasoning:** Generating the atlas at build time is easy; integrating it into `framebuffer.rs` cleanly without breaking existing paths takes iteration. TrueType rasterization (e.g., `fontdue`) adds time.
- **Mission:** Replace the current 8x8 bitmap font with a higher-resolution atlas or dynamic TrueType renderer for improved readability.
- **DOD:** Text rendered in windows can use at least two sizes (e.g., 8x8 and 16x16) without pixelation.

### Unified Unicode Support
- **Estimate:** 3–5 days
- **Reasoning:** Currently the OS text rendering relies mostly on an ASCII 8x8 bitmap font and doesn't handle complex UTF-8 multibyte characters. We need font files, parsers, and QuickJS text processing integration.
- **Mission:** Implement full Unicode and emoji support in the kernel text renderer.
- **DOD:** Javascript can call `console.log("Hello 🌍")` and it renders correctly in both the console and within graphical `os.window.drawString` bounds.

### Audio (PC Speaker / AC97)
- **Estimate:** 1–2 days (PC Speaker), 3–4 days (AC97)
- **Reasoning:** PC speaker is trivial PIT manipulation (~50 lines). Timer/interrupt infrastructure already exists. AC97 is a proper PCI device with DMA, similar to RTL8139 but less documented.
- **Mission:** Enable sound output for system alerts and applications.
- **DOD:** A `os.audio.beep(freq, durationMs)` function and a `os.audio.playWav(data)` function produce audible sound in QEMU.

### Pixel Buffer API ✓
- **Estimate:** 2–3 days
- **Reasoning:** Currently JS can only draw via high-level `os.window` calls. A raw pixel buffer exposed as a `Uint32Array` would enable pixel-level rendering (games, image display, custom renderers) and is a prerequisite for image decoding and zero-copy graphics. The kernel already manages per-window pixel buffers.
- **Mission:** Expose a window's pixel buffer directly to JS for high-performance direct pixel manipulation.
- **DOD:** `os.window.getPixelBuffer(winId)` returns a `Uint32Array`; writes to it appear on screen after `os.window.flush(winId)`.

### Image Decoding (PNG/JPEG) ✓
- **Estimate:** 2–3 days
- **Reasoning:** `png` and `jpeg-decoder` crates exist in `no_std` variants. Depends on Pixel Buffer API for the blit path.
- **Mission:** Provide a native way for JS apps to load and display standard image formats.
- **DOD:** `os.graphics.drawImage("path.png", x, y)` works for standard PNG files stored in `os.store`.

### System Notifications API ✓
- **Reasoning:** Reused the existing crash notification overlay infrastructure (`push_notification` / `draw_notification_overlay`). Added an `is_crash: bool` field to distinguish red crash toasts from blue info toasts.
- **Result:** `os.notify("msg")` pushes a blue kernel-level overlay toast (~3 s). Stacks with crash notifications (max 4 visible). Zero new rendering code required.

---

## Expanding the Platform

### Global JS Environment Normalization
- **Estimate:** 1 day
- **Reasoning:** Standard ECMA environments export constants like `globalThis` and `window`. Embedded scripts (e.g. `crasher.jsos`) throw `ReferenceError: globalthis is not defined` if these aren't bound in the sandbox.
- **Mission:** Provide standard ES2020 globals so third-party Javascript tools (like bundle formatters) execute seamlessly out of the box.
- **DOD:** References to `globalThis` correctly point to the `window` context without raising exceptions.

### Canvas 2D API ✓
- **Estimate:** 3–5 days
- **Reasoning:** Depends on Window Drawing Primitives being complete. Work involves arcs, beziers, transforms, and a stateful context object (e.g., `os.window.getContext(winId)`).
- **Mission:** Expose a standard-passing subset of the HTML5 Canvas API for richer graphics.
- **DOD:** A JS script can draw a complex path with fills, strokes, arcs, and rotations using a familiar Canvas-like API.

### `os.fs` JS Bindings (RamFS)
- **Estimate:** 1 day
- **Reasoning:** `src/fs/ramfs.rs` implements an in-memory key/byte-store but is not exposed to JS. Adding bindings would give apps a session-scoped scratch space distinct from the persistent `os.store`, and could back ES module resolution.
- **Mission:** Expose the RamFS to JS for temporary file/object storage within a session.
- **DOD:** `os.fs.write("tmp/data.bin", bytes)` and `os.fs.read("tmp/data.bin")` work within a session; data is lost on reboot (by design).

### USB HID (XHCI) ✓
- **Estimate:** 5–7 days
- **Reasoning:** XHCI is a complex spec. Requires descriptor parsing, ring buffer management, and debugging against QEMU.
- **Mission:** Add support for physical mice and keyboards via USB on real hardware or modern QEMU profiles.
- **DOD:** Mouse movement and keyboard clicks work when using a USB device instead of PS/2.

### Preemptive Scheduling
- **Estimate:** 4–6 days
- **Reasoning:** Context switching is tricky: saving/restoring registers, per-process kernel stacks, and timer handler modifications.
- **Mission:** Ensure system responsiveness by forcefully switching between running JS tasks on timer interrupts.
- **DOD:** An infinite loop in one JS app doesn't freeze the mouse or other windows.

### JSOS Package Manager (JPM)
- **Estimate:** 3–5 days
- **Reasoning:** We have `os.fetch` and `os.store`. A CLI program (`jpm install fetch-polyfill`) can download JS bundles from a remote registry and persist them into the OS for global use.
- **Mission:** Make the OS extensible at runtime without needing to recompile the Rust kernel to embed apps.
- **DOD:** Running `jpm install snake` in the shell downloads the game from the web, saves it, and adds it to the launcher.

---

## Long-term / Ambitious Plans

### TLS ALPN Support (HTTP/1.1 Negotiation)
- **Estimate:** 1–2 days
- **Reasoning:** embedded-tls 0.17 sends no ALPN extension in the ClientHello. Servers that default to HTTP/2 (e.g. Akamai/Microsoft CDN) accept the TLS handshake but immediately send a `close_notify` when they receive an HTTP/1.1 request, because no `h2`/`http/1.1` was negotiated. The fix is to advertise `"http/1.1"` via ALPN so these servers fall back to HTTP/1.1 instead of closing. `os.fetch` already works against servers that don't enforce ALPN (GitHub, most VPS hosts), so this is a compatibility gap rather than a total breakage.
- **Mission:** Make `os.fetch` work against HTTP/2-capable CDNs (Akamai, Cloudflare, AWS CloudFront) by advertising `http/1.1` in the TLS ClientHello.
- **Approach:**
  1. Check if embedded-tls has gained ALPN support in a newer release (`TlsConfig::with_alpn` or similar). If so, bump the version in `Cargo.toml`.
  2. If not available upstream, fork or patch the `TlsConfig` builder to include an ALPN extension (`0x00 0x10`) in the ClientHello with the single protocol `"http/1.1"`.
  3. Apply the change in both TLS handshake sites in `src/net/mod.rs` (fetch jobs at state 6, WebSocket jobs at the equivalent state).
  4. Test against `learn.microsoft.com` and `cdn.jsdelivr.net` — both enforce h2 without ALPN.
- **DOD:** `os.fetch("https://learn.microsoft.com/...")` returns a response body instead of `Error: TLS Read Failed: MissingHandshake`.

### Full Browser Engine
- **Estimate:** 2–4 weeks
- **Reasoning:** CSS layout (flexbox, block, inline) is genuinely hard. The current pure-JS engine in `demo_browser.jsos` is a pragmatic ceiling for now.
- **Mission:** Implement a CSS-driven layout engine (Reflow/Repaint) to render real HTML documents.
- **DOD:** A `browser.jsos` that can render basic Flexbox layouts correctly.

### Zero-Copy Shared Graphics
- **Estimate:** 4–5 days
- **Reasoning:** Extends the Pixel Buffer API by mapping kernel-managed window buffers directly into the JS heap to eliminate per-frame blit overhead entirely.
- **Mission:** Map window pixels into JS as a `Uint32Array` for absolute maximum rendering performance.
- **DOD:** Stress test app achieves 60fps full-screen noise at high resolutions.

### WebAssembly (WASM) Runtime
- **Estimate:** 2–3 weeks
- **Reasoning:** QuickJS lacks WASM support. Integrating a `no_std` interpreter like `wasmi` alongside QuickJS would allow JS to instantiate and run pre-compiled C/Rust/Zig binaries safely in userspace.
- **Mission:** Bring heavy computational workloads (video decoding, complex games) to the OS without sacrificing the sandbox.
- **DOD:** `WebAssembly.instantiateStreaming` is implemented and can execute a basic factorial WASM module.

### SMP / Multi-core
- **Estimate:** 2–3 weeks
- **Reasoning:** APIC programming, IPIs, per-core GDT/IDT/TSS, and lock-free data structures. High risk of complex race conditions.
- **Mission:** Utilize multiple CPU cores for parallel JS execution (WebWorkers style).
- **DOD:** Multiple `QuickJsSandbox` instances running on separate physical cores simultaneously.

### JS JIT Compiler
- **Estimate:** 1–2 months
- **Reasoning:** Writing a bytecode → x86-64 backend for QuickJS is a significant undertaking.
- **Mission:** Boost JS performance by compiling hot code paths to native machine code.
- **DOD:** Benchmark suite (e.g., SunSpider) shows a >5x speedup over interpreted mode.

### Hardware Abstraction Layer (HAL) for JS
- **Estimate:** 5–7 days
- **Reasoning:** Exposing PCI and I/O ports to JS for "User-space Driver" development. Requires a strict permission/capability system.
- **Mission:** Enable hardware driver development entirely in JavaScript for rapid prototyping.
- **DOD:** `os.hw.io.write8` works for trusted apps; unauthorized access is blocked.

---

## Technical Debt & Refactoring

### Timer API & Event Loop Overhaul
- **Estimate:** 2–3 days
- **Reasoning:** The current `setTimeout`/`setInterval` polyfill relies on evaluating raw strings and passing `__PID` around to track timer ownership. This is fragile and specifically caused the kernel to freeze formatting string primitives into `u64` IDs.
- **Mission:** Rewrite the timer implementation natively in Rust using QuickJS opaque pointers (or proper C-closures) so JS timers are safe, fast, and completely decoupled from `eval()` string manipulation.
- **DOD:** JavaScript timers use native bindings, removing the `globalThis.__timers` polyfill hack entirely.

### Compositor Damage Rectangles (Dirty Rects)
- **Estimate:** 3–5 days
- **Reasoning:** Current drawing semantics effectively invalidate/render full windows or the whole screen, and the mouse cursor uses an inefficient pixel-save/restore back buffer. 
- **Mission:** Implement damage tracking (dirty rectangles) in the Rust window system and `winman.jsos` so that only changed pixels are blitted to the framebuffer.
- **DOD:** UI rendering cycles only draw modified regions. CPU usage drops drastically when windows are idle.

### Fast FFI API Bindings 
- **Estimate:** 4–5 days
- **Reasoning:** JSOS passes all FFI data by manually typechecking `JSValue` items, parsing `argc`, and doing dynamic copies in every single `js_os.*` function inside `js_runtime.rs`. This results in dense, repetitive code that is prone to bugs (like unused args or bad casts).
- **Mission:** Build a declarative mapping macro (similar to `rquickjs`) to auto-generate the type-checking and conversions for all `os.*` bindings.
- **DOD:** `js_runtime.rs` boilerplate is reduced by 60%, and JS-to-Rust API bindings can be defined via a single `#[jsos_bind]` annotation.

### JS Userland Standard Library (`libjsos`)
- **Estimate:** 2–3 days
- **Reasoning:** Currently, every `.jsos` app (like `shell.jsos`) manually implements `globalThis.on_key`, `globalThis.on_ipc`, and handles drawing primitives mathematically with raw `os.window.drawRect` calls.
- **Mission:** Extract common boilerplate into a reusable imported library.
- **DOD:** Apps can `import { Window, EventLoop } from 'libjsos'` which abstracts away raw IPC and pixel math, reducing app code sizes by half.

### Immediate-Mode GUI Toolkit (IMGUI)
- **Estimate:** 5–7 days
- **Reasoning:** Building graphical tools requires manually tracking X/Y coordinates, scroll states, and hitboxes for every single button, list, and scrollbar.
- **Mission:** Build an ImGui-style or simple widget framework on top of `libjsos` for rapid UI development.
- **DOD:** Developers can call `if (ui.button("Click Me")) { ... }` inside a `render()` loop, handling drawing and input routing atomically.
