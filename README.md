# JSOS — A JavaScript Operating System

**JSOS** is an experimental bare-metal operating system for x86_64, written in Rust, where every application is JavaScript. It boots to a graphical desktop, runs a windowing compositor, and lets you write, edit, and launch JS programs — all without a single line of userspace C.

Under the hood, a custom-patched build of [QuickJS-NG](https://github.com/quickjs-ng/quickjs) is compiled as a freestanding C library and linked directly into the kernel via FFI. Each JS application runs in its own sandboxed `QuickJsSandbox` instance with full error isolation — a crash in one app never brings down the system.

---

## What Can It Do?

- **Graphical Desktop** — Focus-follows-mouse compositor (`winman.jsos`) with a taskbar, app launcher, system tray, window dragging, and z-ordering. Title bars and close buttons are rendered by the kernel.
- **Interactive Shell** — A full terminal with command history (persisted across reboots), scrollback, word-jumping, a built-in text editor, and inline JavaScript evaluation.
- **Networking** — TCP/IP stack (via `smoltcp`) with DHCP, DNS, HTTPS (`embedded-tls`), and WebSocket support. `fetch()` works out of the box, including redirect following and TLS session resumption.
- **Persistent Storage** — Custom key-value filesystem (JSKV) on a 64 MB ATA disk. Survives reboots. Exposed to JS as `os.store.get/set/list/delete`.
- **Image Decoding** — Native PNG, JPEG, and BMP decoding rendered directly to window pixel buffers.
- **Unicode & Emoji** — Multi-font fallback rendering chain for Latin, CJK, symbols, and emoticons.
- **USB Support** — xHCI host controller driver for USB keyboards and mice.
- **Real Hardware** — Boots on physical x86_64 PCs via BIOS or UEFI. Networking requires an RTL8139-compatible NIC.

---

## Built-in Applications

All apps are plain JavaScript files (`.jsos`) embedded in the kernel image and automatically seeded to disk at boot. You can edit them live from the shell.

| App | Description |
|---|---|
| `shell.jsos` | Interactive terminal with 20+ commands, history, scrollback, and a built-in text editor |
| `node.jsos` | JavaScript REPL — type expressions, see results |
| `snake.jsos` | Classic Snake game with keyboard controls |
| `demo_browser.jsos` | Experimental HTML renderer — fetches and displays simple web pages |
| `calculator.jsos` | On-screen calculator |
| `imageview.jsos` | Image viewer with PNG/JPEG/BMP support |
| `sysman.jsos` | System resource monitor (memory, processes, windows) |
| `webremote.jsos` | Hosts a web dashboard accessible from the host machine on port 8080 |
| `drawtest.jsos` | Graphics primitives demo (lines, circles, fills) |
| `pixeldemo.jsos` | Raw pixel buffer manipulation showcase |
| `fontdemo.jsos` | Unicode and font rendering test |
| `seriallog.jsos` | Background serial output logger |

---

## Getting Started

### Prerequisites

| Tool | Purpose |
|---|---|
| **Rust nightly** | Kernel compilation (`no_std`, build-std) |
| **Clang** | Compiling the QuickJS C source |
| **QEMU** | Emulation and testing |
| **`llvm-objcopy`** | Shipped with `rustup component add llvm-tools-preview` |

The required Rust toolchain is pinned in `rust-toolchain.toml`. Just clone and build — Cargo handles the rest.

### Build & Run

**Windows (PowerShell):**
```powershell
cargo run
```

**Linux / macOS:**
```bash
cargo run
```

The build script compiles both BIOS and UEFI bootloader images, creates a 64 MB persistent storage disk (`jskv.img`), and launches QEMU with an emulated RTL8139 NIC, USB HID devices, and serial output piped to your terminal.

### QEMU Flags

The launch scripts configure QEMU with:
- 1 GB RAM, VGA standard display
- RTL8139 network device with host port 8080 forwarded to guest port 80
- USB xHCI controller with keyboard and mouse
- Serial output on stdio (kernel debug logs)
- Persistent second drive (`jskv.img`)

---

## Architecture at a Glance

```
┌─────────────────────────────────────────────────────┐
│                   Main Event Loop                    │
│  keyboard → mouse → USB → network → timers → render │
├─────────────────────────────────────────────────────┤
│  Process Manager          │  JS Runtime (FFI)        │
│  - spawn / kill / IPC     │  - QuickJS-NG sandboxes  │
│  - focus & foreground     │  - os.* API bindings     │
│  - crash isolation        │  - timer polling         │
├───────────────────────────┼──────────────────────────┤
│  Framebuffer / Graphics   │  Networking              │
│  - double buffering       │  - RTL8139 driver (DMA)  │
│  - bitmap font rendering  │  - smoltcp TCP/IP stack  │
│  - kernel overlays        │  - embedded-tls (HTTPS)  │
├───────────────────────────┼──────────────────────────┤
│  Storage (JSKV)           │  Hardware                │
│  - ATA PIO driver         │  - GDT / IDT / PIC      │
│  - 128-key MFT on disk    │  - PS/2 + USB HID       │
│  - key-value API          │  - ACPI power mgmt      │
└───────────────────────────┴──────────────────────────┘
```

The kernel is strictly single-threaded and event-driven. The main loop polls all hardware sources, runs pending JS events for each process, draws kernel overlays (crash toasts, cursor), and swaps framebuffers — one full cycle per frame. The CPU halts only when no network I/O is in flight.

For a deep dive, see [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## JavaScript API

Every JS sandbox has access to the `os` global namespace, which exposes the full kernel surface:

```javascript
// Spawn a new app
const pid = os.spawn("snake.jsos");

// Fetch a URL (returns a Promise)
const html = await os.fetch("https://example.com");

// Create a window and draw to it
const win = os.window.create(100, 100, 400, 300);
os.window.drawRect(win, 0, 0, 400, 300, 20, 20, 30);
os.window.drawString(win, "Hello from JSOS!", 10, 30, 255, 255, 255);
os.window.flush(win);

// Persistent storage
os.store.set("greeting", "hello world");
os.store.get("greeting"); // "hello world"

// System info
JSON.parse(os.sysinfo());  // { cpu_vendor, heap_used_mb, ... }
JSON.parse(os.rtc());      // { h, m, s, day, month, year }
```

Browser compatibility globals (`fetch`, `setTimeout`, `setInterval`, `atob`, `btoa`, `console.*`, `performance.now`, `navigator`) are injected automatically.

Full reference: [`docs/API.md`](docs/API.md)

---

## Writing Your Own App

Create a `.jsos` file, save it to the store, and launch it:

```javascript
// In the shell:
// > edit myapp.jsos

const win = os.window.create(200, 100, 300, 200);

globalThis.on_key = function(charCode) {
    os.window.drawRect(win, 0, 0, 300, 200, 10, 10, 30);
    os.window.drawString(win, "Key: " + charCode, 10, 30, 255, 255, 255);
    os.window.flush(win);
};

globalThis.on_ipc = function(msg) {
    const ev = JSON.parse(msg);
    if (ev.type === "RENDER") {
        os.window.drawRect(win, 0, 0, 300, 200, 10, 10, 30);
        os.window.drawString(win, "My First JSOS App", 10, 30, 0, 200, 255);
        os.window.flush(win);
    }
};
```

Alternatively, you can author `.jsos` files on your host machine and embed them into the kernel by adding an entry to `BUILTIN_BINS` in `src/main.rs`.

Apps that register a window or timer stay alive as long-running processes. Apps that finish without either are automatically cleaned up.

---

## Booting on Real Hardware

JSOS can boot on physical x86_64 machines via USB. The build produces both a raw BIOS disk image and a UEFI `.efi` binary.

- **BIOS**: Flash `boot-bios.img` to a USB drive with `dd` or Rufus
- **UEFI**: Copy `BOOTX64.EFI` to `/EFI/BOOT/` on a FAT32 USB stick

Networking requires an RTL8139 NIC. Keyboard and mouse work via PS/2 or USB legacy emulation.

**Note:** Persistent storage (`jskv.img`) on USB-booted real hardware is not yet supported.

See [`docs/REAL_HARDWARE.md`](docs/REAL_HARDWARE.md) for detailed instructions.

---

## Project Structure

```
src/
├── main.rs            # Boot sequence, event loop, built-in binary seeding
├── js_runtime.rs      # QuickJS FFI bindings, os.* API surface
├── process.rs         # Process lifecycle, IPC, sandbox isolation
├── framebuffer.rs     # Double-buffered rendering, font rasterization
├── graphics.rs        # Drawing primitives (lines, circles, text)
├── net/               # smoltcp TCP/IP stack, RTL8139 driver, TLS
├── storage.rs         # JSKV persistent key-value store (ATA)
├── jsos/              # Built-in JavaScript applications
├── js/                # JS polyfills and runtime helpers
├── interrupts.rs      # IDT, PIC, keyboard/mouse IRQ handlers
├── xhci.rs            # USB xHCI host controller driver
└── ...                # Memory, GDT, PCI, power management, etc.

quickjs/               # Patched QuickJS-NG C source (no_std, freestanding)
docs/                  # Architecture guide, API reference, hardware guide
```

---

## Design Philosophy

1. **JavaScript is first-class.** The kernel exists to serve JS applications. Most kernel capabilities are reachable from `os.*`.
2. **Crash isolation over crash prevention.** A bad JS program gets killed and a toast pops up. The kernel keeps running.
3. **No userspace rings.** Everything runs in Ring 0 with logical sandboxing. This is a research OS, not a production server (yet).
4. **Event-driven, not threaded.** One core, one loop, cooperative multitasking. Simple to reason about, easy to debug.
5. **Persistence by default.** The shell history, user scripts, and data all survive reboots on the JSKV disk.

---

## Dependencies

JSOS depends on a carefully curated set of `no_std`-compatible Rust crates:

| Crate | Role |
|---|---|
| `bootloader` | x86_64 bootloader (BIOS + UEFI) |
| `smoltcp` | Userspace TCP/IP networking stack |
| `embedded-tls` | TLS 1.3 for HTTPS (patched local fork) |
| `u8g2-fonts` | Bitmap font rendering |
| `zune-png` / `zune-jpeg` / `tinybmp` | Image decoding |
| `x86_64` | CPU control registers, paging (local fork) |
| `linked_list_allocator` | Kernel heap allocator |
| `spin` | Spinlock-based mutexes for interrupt-safe globals |
| `rand_chacha` + `getrandom` | CSPRNG backed by RDRAND hardware |

QuickJS-NG is compiled from C source using `clang` via the `cc` build crate.

---

## Current Status

The OS is functional and self-hosting for its JS environment. Completed milestones include:

- ✅ Graphical compositor with draggable, closable windows
- ✅ Stable HTTPS fetch with redirect following and TLS resumption
- ✅ Persistent storage surviving reboots
- ✅ Multi-process isolation with crash recovery
- ✅ Image decoding and pixel buffer rendering
- ✅ USB xHCI keyboard/mouse support
- ✅ Unicode and emoji rendering
- ✅ Built-in text editor and shell history

Known limitations:
- Networking is functional but not yet robust for all hosts and endpoints.
- No audio support.
- No preemptive scheduling.
- No package manager.
- No WebAssembly runtime.

See [`docs/impl-plan.md`](docs/impl-plan.md) for the full roadmap.

---

## Contributing

JSOS is a personal research project, but contributions and ideas are welcome. If you're interested in bare-metal Rust, OS development, or JavaScript runtime internals, feel free to open an issue or PR.

**Important constraints to be aware of:**
- The kernel is `#![no_std]` — no standard library, no POSIX
- All JS↔Rust communication crosses an `unsafe` FFI boundary
- The `x86_64` and `embedded-tls` crates are local forks with project-specific patches
- QuickJS compilation requires `clang` in your PATH

---

## License

JSOS is released under the [MIT License](LICENSE).

Created by **Loni Karmon** — [loni.123.102@gmail.com](mailto:loni.123.102@gmail.com)

Developed with assistance from Gemini and Claude AI.