# JSOS Implementation Plan

This document outlines the planned features and improvements for JSOS, categorized by impact and complexity. Each item includes the estimated effort, reasoning, core mission, and definition of done (DOD).

---

## High Impact & Self-Contained

### Build Warning Zero-Tolerance
- **Estimate:** < 1 day
- **Reasoning:** `cargo check` and QuickJS C files currently emit numerous `unused_mut`, `dead_code`, and unused parameter warnings.
- **Mission:** Enforce strict style guidelines to keep the bare-metal kernel fully clean and deterministic.
- **DOD:** `cargo build --target x86_64-os.json` compiles with `0 warnings emitted`.

### DNS Resolver
- **Estimate:** 1–2 days
- **Reasoning:** `os.fetch` currently requires a hardcoded IP or relies on QEMU user-mode NAT DNS passthrough. On real hardware or a different network, any hostname lookup fails silently. A stub resolver using UDP port 53 against a configurable nameserver (default `8.8.8.8`) would make fetch work universally.
- **Mission:** Resolve hostnames to IPs inside the kernel before opening TCP connections.
- **DOD:** `os.fetch("https://example.com/")` resolves the hostname via DNS and connects without needing a hardcoded IP. Configurable via `os.net.setDns(ip)`.

### NTP Clock Sync
- **Estimate:** 1 day
- **Reasoning:** The RTC is read once at boot and never corrected. After a few hours, drift accumulates and `os.rtc()` returns stale time. NTP is a single UDP exchange — send a 48-byte packet, read the 64-bit timestamp in the response.
- **Mission:** Sync the kernel clock to real time at boot.
- **DOD:** After network is up, the kernel sends one NTP request to `pool.ntp.org` and corrects `os.rtc()`. Serial log confirms sync and offset.

### Audio (PC Speaker / AC97)
- **Estimate:** 1–2 days (PC Speaker), 3–4 days (AC97)
- **Reasoning:** PC speaker is trivial PIT manipulation (~50 lines). Timer/interrupt infrastructure already exists. AC97 is a proper PCI device with DMA, similar to RTL8139 but less documented.
- **Mission:** Enable sound output for system alerts and applications.
- **DOD:** `os.audio.beep(freq, durationMs)` and `os.audio.playWav(data)` produce audible sound in QEMU.

### IPC Broadcast
- **Estimate:** < 1 day
- **Reasoning:** Currently `os.sendIpc` targets a single PID. Many OS-level events (theme change, network up/down, low memory) are useful to all running apps simultaneously. Broadcast is a single loop over `PROCESS_LIST`.
- **Mission:** Allow a process to send a message to all running processes at once.
- **DOD:** `os.broadcast(msg)` delivers the message to every process's `on_ipc` handler except the sender. Verified by spawning two apps and observing both receive the message.

---

## Expanding the Platform

### `os.fs` JS Bindings (VFS)
- **Estimate:** 2–3 days
- **Reasoning:** `src/fs/ramfs.rs` implements an in-memory key/byte-store but is not exposed to JS. A proper hierarchical path namespace (backed by RamFS for `/tmp` and JSKV for `/data`) would enable module resolution by path and give apps a clear storage model instead of flat keys.
- **Mission:** Expose a unified filesystem API to JS with path-based addressing.
- **DOD:** `os.fs.write("/tmp/data.bin", bytes)`, `os.fs.read("/data/config.json")`, and `os.fs.list("/")` work. Paths under `/tmp` are session-scoped; `/data` persists via JSKV.

### JSOS Package Manager (JPM)
- **Estimate:** 3–5 days
- **Reasoning:** We have `os.fetch` and `os.store`. A CLI program (`jpm install fetch-polyfill`) can download JS bundles from a remote registry and persist them into the OS for global use.
- **Mission:** Make the OS extensible at runtime without recompiling the Rust kernel to embed apps.
- **DOD:** Running `jpm install snake` in the shell downloads the game from the web, saves it to JSKV, and adds it to the launcher. `jpm list` shows installed packages.

### App Permissions / Capability System
- **Estimate:** 3–4 days
- **Reasoning:** Every `.jsos` app currently has full access to all `os.*` APIs including network, storage, and process spawning. A capability model would let the OS deny network access to untrusted scripts or require confirmation before `os.store.delete` wipes data.
- **Mission:** Sandbox JS apps at the API level using a per-process capability set.
- **DOD:** Apps declare required capabilities in a header comment (`// @capabilities: network, storage`). The kernel enforces them. An app without `network` capability gets a JS exception when calling `os.fetch`. `os.spawn` accepts an optional capabilities argument.

### Web Workers (single-core cooperative)
- **Estimate:** 3–4 days
- **Reasoning:** Some apps need background computation (e.g., image processing, compression) without blocking rendering. Since JSOS is single-core, true parallel execution isn't possible, but a second `QuickJsSandbox` per app — polled in the background and communicating via message passing — gives the cooperative equivalent of a Worker.
- **Mission:** Let a `.jsos` app offload heavy computation to a background sandbox.
- **DOD:** `const w = new Worker("worker.jsos")` spawns a background sandbox. `w.postMessage(data)` and `w.onmessage` work via the existing IPC queue. The main app's render loop continues uninterrupted while the worker runs.

### Boot Configuration
- **Estimate:** 1 day
- **Reasoning:** Currently the kernel always boots into `winman.jsos`. There is no way to change the default app, screen resolution, or network config without recompiling. These settings fit naturally in a reserved JSKV key read during `kernel_main`.
- **Mission:** Make key boot parameters configurable at runtime via a JSKV-backed config.
- **DOD:** A `boot.cfg` JSKV key (JSON) controls: default init process, preferred screen resolution, DNS server, and hostname. `shell.jsos` has a `config` command to edit it. Changes take effect on next reboot.

---

## Developer Experience

### In-OS CPU/Memory Profiler
- **Estimate:** 2–3 days
- **Reasoning:** There is no way to measure how long a JS process actually takes per frame, how many bytes it has allocated, or which timer is firing most frequently. The kernel already tracks `TICKS` and QuickJS exposes `JS_GetMemoryUsage`. A `sysman.jsos` panel could surface this live.
- **Mission:** Expose per-process execution time and heap usage to JS.
- **DOD:** `os.profile(pid)` returns `{ heapUsed, heapTotal, gcRuns, preemptions }`. `sysman.jsos` shows a live graph of per-process CPU% and heap usage, updating at 2 Hz.

### JS Userland Standard Library (`libjsos`)
- **Estimate:** 2–3 days
- **Reasoning:** Every `.jsos` app manually implements `on_key`, `on_ipc`, window setup, and render loops. The boilerplate is nearly identical across all apps.
- **Mission:** Extract common boilerplate into a reusable imported library.
- **DOD:** Apps can `import { Window, EventLoop } from 'libjsos'` which abstracts raw IPC and pixel math, reducing app code sizes by half. Library is stored in JSKV under `libjsos.js`.

### Immediate-Mode GUI Toolkit (IMGUI)
- **Estimate:** 5–7 days
- **Reasoning:** Building graphical tools requires manually tracking X/Y coordinates, scroll states, and hitboxes for every button, list, and scrollbar.
- **Mission:** Build an ImGui-style widget framework on top of `libjsos` for rapid UI development.
- **DOD:** `if (ui.button("Click Me")) { ... }` inside a `render()` loop handles drawing and input routing automatically. At minimum: button, label, text input, scrollable list, checkbox.

### Timer API & Event Loop Overhaul
- **Estimate:** 2–3 days
- **Reasoning:** The current `setTimeout`/`setInterval` polyfill evaluates raw strings and passes `__PID` around to track timer ownership. This is fragile — a misformatted ID silently drops the timer.
- **Mission:** Rewrite the timer implementation natively using QuickJS opaque pointers so JS timers are safe, fast, and decoupled from `eval()` string manipulation.
- **DOD:** `setTimeout(fn, ms)` and `setInterval(fn, ms)` use native bindings. `clearTimeout` and `clearInterval` work correctly. The `globalThis.__timers` polyfill hack is removed.

---

## Long-term / Ambitious

### Full Browser Engine
- **Estimate:** 2–4 weeks
- **Reasoning:** CSS layout (flexbox, block, inline) is genuinely hard. The current pure-JS engine in `demo_browser.jsos` is a pragmatic ceiling for now.
- **Mission:** Implement a CSS-driven layout engine to render real HTML documents.
- **DOD:** A `browser.jsos` that renders basic Flexbox layouts correctly, including `<img>` tags via the image decoder.

### Compositor Damage Rectangles
- **Estimate:** 3–5 days
- **Reasoning:** Every frame redraws all windows in full even when nothing changed. On a 1920×1080 framebuffer that is 8 MB of memcpy per frame at 60 fps.
- **Mission:** Only blit changed regions to the framebuffer.
- **DOD:** CPU usage when all windows are idle drops by >80% measured via the profiler. Moving a single window only redraws the affected rectangle.

### WebAssembly (WASM) Runtime
- **Estimate:** 2–3 weeks
- **Reasoning:** QuickJS lacks WASM support. Integrating a `no_std` interpreter like `wasmi` alongside QuickJS would allow JS to instantiate and run pre-compiled C/Rust/Zig binaries safely in userspace.
- **Mission:** Bring heavy computational workloads to the OS without sacrificing the sandbox.
- **DOD:** `WebAssembly.instantiateStreaming` executes a basic factorial WASM module. Interop between the WASM module and `os.*` APIs works via imported JS functions.

### SMP / Multi-core
- **Estimate:** 2–3 weeks
- **Reasoning:** APIC programming, IPIs, per-core GDT/IDT/TSS, and lock-free data structures. High risk of complex race conditions. Requires auditing every `spin::Mutex` for contention.
- **Mission:** Run multiple `QuickJsSandbox` instances on separate physical cores simultaneously.
- **DOD:** Two JS processes execute in parallel on two cores. `os.sysinfo()` reports per-core load. No deadlocks under a 60-second stress test.

### JS JIT Compiler
- **Estimate:** 1–2 months
- **Reasoning:** Writing a bytecode → x86-64 backend for QuickJS is a significant undertaking, but the QuickJS bytecode format is well-documented.
- **Mission:** Boost JS performance by compiling hot code paths to native machine code.
- **DOD:** SunSpider or equivalent benchmark shows >5× speedup over interpreted mode.

### Hardware Abstraction Layer for JS
- **Estimate:** 5–7 days
- **Reasoning:** Exposing PCI and I/O ports to JS for userspace driver development. Requires a strict capability system (see App Permissions above) to prevent arbitrary hardware access.
- **Mission:** Enable hardware driver development entirely in JavaScript for rapid prototyping.
- **DOD:** `os.hw.io.write8(port, val)` and `os.hw.pci.enumerate()` work for trusted apps; unauthorized access throws a JS exception.

---

## Technical Debt

### Fast FFI API Bindings
- **Estimate:** 4–5 days
- **Reasoning:** All FFI data is passed by manually typechecking `JSValue` items and parsing `argc` in every `js_os.*` function. The result is dense, repetitive code prone to wrong-cast bugs.
- **Mission:** Build a declarative mapping macro to auto-generate type-checking and conversions for `os.*` bindings.
- **DOD:** `js_runtime.rs` boilerplate is reduced by 60%. New bindings are defined via a single `#[jsos_bind]` annotation instead of manual `argv.offset(n)` chains.

### Unified Unicode Support
- **Estimate:** 3–5 days
- **Reasoning:** Text rendering relies on an ASCII bitmap font. Multibyte UTF-8 characters and emoji are silently dropped or corrupted.
- **Mission:** Implement full Unicode support in the kernel text renderer.
- **DOD:** `console.log("Hello 🌍")` renders correctly in both serial output and `os.window.drawString`. At minimum covers Latin Extended, CJK, and common emoji blocks.
