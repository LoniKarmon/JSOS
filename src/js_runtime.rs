// src/js_runtime.rs — QuickJS-NG FFI-based JavaScript runtime for JSOS
//
// This module provides the QuickJsSandbox which wraps the QuickJS C engine
// via FFI, registering all native `os.*` APIs that winman.jsos, shell.jsos,
// and node.jsos require.

use alloc::string::{String, ToString};
use alloc::format;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::boxed::Box;
use spin::Mutex;
use lazy_static::lazy_static;
use core::sync::atomic::{AtomicU64, Ordering};
use core::ffi::{c_char, c_int, c_void};

// ======== QuickJS C FFI Bindings ========

// Opaque types from QuickJS
#[repr(C)]
pub struct JSRuntime { _opaque: [u8; 0] }

#[repr(C)]
pub struct JSContext { _opaque: [u8; 0] }

// JSValue is a 128-bit tagged union in QuickJS-NG (NaN-boxing on 64-bit)
// Layout: [union (u64), tag (i64)]
#[repr(C)]
#[derive(Clone, Copy)]
pub struct JSValue {
    u: u64,
    tag: i64,
}

// JSValue tags — only keep the ones actually referenced in Rust code
const JS_TAG_INT: i64 = 0;
const JS_TAG_NULL: i64 = 2;
const JS_TAG_UNDEFINED: i64 = 3;
const JS_TAG_EXCEPTION: i64 = 6;

// JS_EVAL flags
const JS_EVAL_TYPE_GLOBAL: c_int = 0;

// QuickJS error message emitted when JS_SetInterruptHandler callback returns non-zero
pub const JS_INTERRUPT_MSG: &str = "interrupted";

type JSCFunction = unsafe extern "C" fn(ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue) -> JSValue;

extern "C" {
    fn JS_NewRuntime() -> *mut JSRuntime;
    fn JS_FreeRuntime(rt: *mut JSRuntime);
    fn JS_NewContext(rt: *mut JSRuntime) -> *mut JSContext;
    fn JS_FreeContext(ctx: *mut JSContext);

    fn JS_Eval(ctx: *mut JSContext, input: *const c_char, input_len: usize, filename: *const c_char, eval_flags: c_int) -> JSValue;
    fn JS_FreeValue(ctx: *mut JSContext, v: JSValue);

    fn JS_GetGlobalObject(ctx: *mut JSContext) -> JSValue;
    fn JS_NewObject(ctx: *mut JSContext) -> JSValue;
    fn JS_NewCFunction2(ctx: *mut JSContext, func: JSCFunction, name: *const c_char, length: c_int, cproto: c_int, magic: c_int) -> JSValue;
    fn JS_SetPropertyStr(ctx: *mut JSContext, this_obj: JSValue, prop: *const c_char, val: JSValue) -> c_int;
    fn JS_GetPropertyStr(ctx: *mut JSContext, this_obj: JSValue, prop: *const c_char) -> JSValue;

    fn JS_NewStringLen(ctx: *mut JSContext, str: *const c_char, len: usize) -> JSValue;

    fn JS_ToCStringLen2(ctx: *mut JSContext, len: *mut usize, val: JSValue, cesu8: c_int) -> *const c_char;
    fn JS_FreeCString(ctx: *mut JSContext, ptr: *const c_char);
    fn JS_ToInt32(ctx: *mut JSContext, pres: *mut i32, val: JSValue) -> c_int;
    fn JS_ToFloat64(ctx: *mut JSContext, pres: *mut f64, val: JSValue) -> c_int;

    fn JS_GetException(ctx: *mut JSContext) -> JSValue;

    fn JS_ExecutePendingJob(rt: *mut JSRuntime, pctx: *mut *mut JSContext) -> c_int;

    fn JS_NewArrayBuffer(
        ctx: *mut JSContext,
        buf: *mut u8,
        len: usize,
        free_func: Option<unsafe extern "C" fn(*mut JSRuntime, *mut c_void, *mut c_void)>,
        opaque: *mut c_void,
        is_shared: bool,
    ) -> JSValue;

    fn JS_NewArrayBufferCopy(ctx: *mut JSContext, buf: *const u8, len: usize) -> JSValue;
    fn JS_GetArrayBuffer(ctx: *mut JSContext, psize: *mut usize, obj: JSValue) -> *mut u8;

    fn JS_SetInterruptHandler(rt: *mut JSRuntime, cb: Option<unsafe extern "C" fn(*mut JSRuntime, *mut c_void) -> c_int>, opaque: *mut c_void);
}

// ======== Helpers ========

fn js_undefined() -> JSValue {
    JSValue { u: 0, tag: JS_TAG_UNDEFINED }
}

fn js_null() -> JSValue {
    JSValue { u: 0, tag: JS_TAG_NULL }
}

fn js_int(v: i32) -> JSValue {
    JSValue { u: v as u64, tag: JS_TAG_INT }
}

fn js_float(v: f64) -> JSValue {
    // JS_TAG_FLOAT64 = 8 in QuickJS-NG (non-NaN-boxing 64-bit layout)
    JSValue { u: v.to_bits(), tag: 8 }
}

fn js_is_exception(v: JSValue) -> bool {
    v.tag == JS_TAG_EXCEPTION
}

unsafe fn js_to_rust_string(ctx: *mut JSContext, val: JSValue) -> String {
    let mut len: usize = 0;
    let cstr = JS_ToCStringLen2(ctx, &mut len, val, 0);
    if cstr.is_null() {
        return String::from("(null)");
    }
    let bytes = core::slice::from_raw_parts(cstr as *const u8, len);
    let s = String::from_utf8_lossy(bytes).into_owned();
    JS_FreeCString(ctx, cstr);
    s
}

unsafe fn js_val_to_i32(ctx: *mut JSContext, val: JSValue) -> i32 {
    let mut result: i32 = 0;
    JS_ToInt32(ctx, &mut result, val);
    result
}

unsafe fn js_val_to_f64(ctx: *mut JSContext, val: JSValue) -> f64 {
    let mut result: f64 = 0.0;
    JS_ToFloat64(ctx, &mut result, val);
    result
}

unsafe fn js_str(ctx: *mut JSContext, s: &str) -> JSValue {
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len())
}

unsafe fn read_str_prop(ctx: *mut JSContext, obj: JSValue, key: &str) -> String {
    let ckey = js_cstring(key);
    let val = JS_GetPropertyStr(ctx, obj, ckey.as_ptr() as *const c_char);
    let s = js_to_rust_string(ctx, val);
    JS_FreeValue(ctx, val);
    s
}

unsafe fn read_f64_prop(ctx: *mut JSContext, obj: JSValue, key: &str) -> f64 {
    let ckey = js_cstring(key);
    let val = JS_GetPropertyStr(ctx, obj, ckey.as_ptr() as *const c_char);
    let f = js_val_to_f64(ctx, val);
    JS_FreeValue(ctx, val);
    f
}

/// Read `this._id` from a canvas context JS object to get the win_id.
unsafe fn read_ctx_id(ctx: *mut JSContext, this_val: JSValue) -> u32 {
    let ckey = js_cstring("_id");
    let val = JS_GetPropertyStr(ctx, this_val, ckey.as_ptr() as *const c_char);
    let id = js_val_to_i32(ctx, val) as u32;
    JS_FreeValue(ctx, val);
    id
}

unsafe fn js_cstring(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

unsafe fn set_func(ctx: *mut JSContext, obj: JSValue, name: &str, func: JSCFunction, argc: c_int) {
    let cname = js_cstring(name);
    let f = JS_NewCFunction2(ctx, func, cname.as_ptr() as *const c_char, argc, 0, 0); 
    JS_SetPropertyStr(ctx, obj, cname.as_ptr() as *const c_char, f);
}

unsafe fn set_prop_obj(ctx: *mut JSContext, obj: JSValue, name: &str, val: JSValue) {
    let cname = js_cstring(name);
    JS_SetPropertyStr(ctx, obj, cname.as_ptr() as *const c_char, val);
}

/// Minimal flat JSON string-array parser. Does not handle escapes or nested values.
/// Returns None for empty or malformed input.
fn parse_json_string_array(json: &str) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let json = json.trim();
    if !json.starts_with('[') || !json.ends_with(']') {
        return None;
    }
    let inner = &json[1..json.len() - 1];
    if inner.trim().is_empty() {
        return None;
    }
    let mut result = alloc::vec::Vec::new();
    let mut remaining = inner;
    loop {
        remaining = remaining.trim();
        if remaining.is_empty() {
            break;
        }
        if remaining.starts_with('"') {
            let start = 1;
            if let Some(end) = remaining[start..].find('"') {
                result.push(alloc::string::String::from(&remaining[start..start + end]));
                remaining = &remaining[start + end + 1..];
                remaining = remaining.trim_start_matches(',');
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if result.is_empty() { None } else { Some(result) }
}

// ======== Timer Infrastructure ========

struct TimerEntry {
    pid: u32,
    timer_id: String,
    fire_at_tick: u64,
}

lazy_static! {
    static ref TIMER_QUEUE: Mutex<Vec<TimerEntry>> = Mutex::new(Vec::new());
    static ref KV_STORE: Mutex<BTreeMap<String, String>> = {
        let mut m = BTreeMap::new();
        m.insert("events".to_string(), include_str!("js/events.js").to_string());
        m.insert("buffer".to_string(), include_str!("js/buffer.js").to_string());
        // Phase 4: HTML/CSS browser engine modules
        m.insert("dom".to_string(),    include_str!("js/dom.js").to_string());
        m.insert("css".to_string(),    include_str!("js/css.js").to_string());
        m.insert("layout".to_string(), include_str!("js/layout.js").to_string());
        m.insert("paint".to_string(),  include_str!("js/paint.js").to_string());
        m.insert("test_features.js".to_string(), include_str!("js/test_features.js").to_string());
        Mutex::new(m)
    };
    static ref BINS: Mutex<BTreeMap<String, String>> = {
        let mut m = BTreeMap::new();
        m.insert("shell.jsos".to_string(), include_str!("jsos/shell.jsos").to_string());
        m.insert("node.jsos".to_string(), include_str!("jsos/node.jsos").to_string());
        m.insert("snake.jsos".to_string(), include_str!("jsos/snake.jsos").to_string());
        m.insert("winman.jsos".to_string(), include_str!("jsos/winman.jsos").to_string());
        m.insert("demo_browser.jsos".to_string(), include_str!("jsos/demo_browser.jsos").to_string());
        m.insert("webremote.jsos".to_string(), include_str!("jsos/webremote.jsos").to_string());
        m.insert("sysman.jsos".to_string(), include_str!("jsos/sysman.jsos").to_string());
        m.insert("fontdemo.jsos".to_string(), include_str!("jsos/fontdemo.jsos").to_string());
        m.insert("drawtest.jsos".to_string(), include_str!("jsos/drawtest.jsos").to_string());
        m.insert("calculator.jsos".to_string(), include_str!("jsos/calculator.jsos").to_string());
        m.insert("imageview.jsos".to_string(), include_str!("jsos/imageview.jsos").to_string());
        m.insert("canvastest.jsos".to_string(), include_str!("jsos/canvastest.jsos").to_string());
        Mutex::new(m)
    };
    static ref CLIPBOARD: Mutex<String> = Mutex::new(String::new());
    // Compile-time embedded binary blobs — fallback for systems without a JSKV disk.
    pub static ref BINARY_BINS: Mutex<BTreeMap<String, &'static [u8]>> = {
        let mut m = BTreeMap::new();
        m.insert("gallery1.png".to_string(), include_bytes!("gallery1.png") as &'static [u8]);
        m.insert("gallery2.bmp".to_string(), include_bytes!("gallery2.bmp") as &'static [u8]);
        m.insert("gallery3.jpg".to_string(), include_bytes!("gallery3.jpg") as &'static [u8]);
        Mutex::new(m)
    };
}

// Window manager global state
pub struct WindowBuffer {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>, // ARGB
    pub owner_pid: u32,   // Which process owns this window
    pub z_index: u32,     // Compositor z-order; higher = drawn later (on top)
    pub pixel_buffer_active: bool, // true once getPixelBuffer() has been called
}

lazy_static! {
    pub static ref WINDOW_BUFFERS: Mutex<BTreeMap<u32, WindowBuffer>> = Mutex::new(BTreeMap::new());
    pub static ref CANVAS_CONTEXTS: Mutex<BTreeMap<u32, crate::canvas::CanvasContext>> =
        Mutex::new(BTreeMap::new());
    static ref NEXT_WINDOW_ID: AtomicU64 = AtomicU64::new(1);
    static ref NEXT_Z_INDEX: spin::Mutex<u32> = spin::Mutex::new(1);
}

// Global cursor position — updated by JS via os.window.setCursor(x, y)
use core::sync::atomic::{AtomicUsize, AtomicBool};
static CURSOR_X: AtomicUsize = AtomicUsize::new(400);
static CURSOR_Y: AtomicUsize = AtomicUsize::new(300);
static CURSOR_VISIBLE: AtomicBool = AtomicBool::new(true);

// Tracks where the cursor was last drawn so we can restore those pixels.
// usize::MAX means "not yet drawn" — skip the restore step on first call.
static CURSOR_LAST_X: AtomicUsize = AtomicUsize::new(usize::MAX);
static CURSOR_LAST_Y: AtomicUsize = AtomicUsize::new(usize::MAX);

const CURSOR_PIXELS: &[(usize, usize, u8, u8, u8)] = &[
    (0,0,255,255,255),(1,0,255,255,255),(0,1,255,255,255),(1,1,50,50,50),(2,1,255,255,255),
    (0,2,255,255,255),(1,2,50,50,50),(2,2,50,50,50),(3,2,255,255,255),
    (0,3,255,255,255),(1,3,50,50,50),(2,3,50,50,50),(3,3,50,50,50),(4,3,255,255,255),
    (0,4,255,255,255),(1,4,50,50,50),(2,4,50,50,50),(3,4,50,50,50),(4,4,50,50,50),(5,4,255,255,255),
    (0,5,255,255,255),(1,5,50,50,50),(2,5,50,50,50),(3,5,50,50,50),(4,5,50,50,50),(5,5,50,50,50),(6,5,255,255,255),
    (0,6,255,255,255),(1,6,50,50,50),(2,6,50,50,50),(3,6,50,50,50),(4,6,50,50,50),(5,6,50,50,50),(6,6,50,50,50),(7,6,255,255,255),
    (0,7,255,255,255),(1,7,50,50,50),(2,7,50,50,50),(3,7,50,50,50),(4,7,50,50,50),(5,7,50,50,50),(6,7,50,50,50),(7,7,50,50,50),(8,7,255,255,255),
    (0,8,255,255,255),(1,8,50,50,50),(2,8,50,50,50),(3,8,50,50,50),(4,8,50,50,50),(5,8,50,50,50),(6,8,255,255,255),(7,8,255,255,255),(8,8,255,255,255),(9,8,255,255,255),
    (0,9,255,255,255),(1,9,50,50,50),(2,9,50,50,50),(3,9,255,255,255),(4,9,50,50,50),(5,9,50,50,50),(6,9,255,255,255),
    (0,10,255,255,255),(1,10,50,50,50),(2,10,255,255,255),(4,10,255,255,255),(5,10,50,50,50),(6,10,50,50,50),(7,10,255,255,255),
    (0,11,255,255,255),(1,11,255,255,255),(4,11,255,255,255),(5,11,50,50,50),(6,11,50,50,50),(7,11,255,255,255),
    (0,12,255,255,255),(5,12,255,255,255),(6,12,50,50,50),(7,12,50,50,50),(8,12,255,255,255),
    (5,13,255,255,255),(6,13,50,50,50),(7,13,50,50,50),(8,13,255,255,255),
    (6,14,255,255,255),(7,14,255,255,255),
];

// Per-pixel save buffer — stores the back-buffer bytes under the cursor.
// One (r,g,b) triple per entry in CURSOR_PIXELS.
lazy_static! {
    static ref CURSOR_SAVE: spin::Mutex<alloc::vec::Vec<(u8,u8,u8)>> =
        spin::Mutex::new(alloc::vec![(0u8,0u8,0u8); CURSOR_PIXELS.len()]);
}

/// Call this after os.clear() so the save buffer is not restored onto a
/// freshly rendered frame — the old pixels are now stale.
pub fn invalidate_cursor_save() {
    CURSOR_LAST_X.store(usize::MAX, Ordering::Relaxed);
    CURSOR_LAST_Y.store(usize::MAX, Ordering::Relaxed);
}

/// Draw the cursor on top of the back buffer, with save/restore so it can
/// move without leaving a trail.  All buffer access is done inside a single
/// framebuffer lock so there is no window where a partial cursor appears.
pub fn draw_cursor_overlay() {
    if !CURSOR_VISIBLE.load(Ordering::Relaxed) { return; }

    // Read position from the hardware mouse atomics — no JS round-trip needed.
    let cx = crate::mouse::MOUSE_X.load(Ordering::Relaxed) as usize;
    let cy = crate::mouse::MOUSE_Y.load(Ordering::Relaxed) as usize;
    let lx = CURSOR_LAST_X.load(Ordering::Relaxed);
    let ly = CURSOR_LAST_Y.load(Ordering::Relaxed);

    // Do NOT skip when the cursor is stationary: winman redraws every frame
    // and can overwrite the pixels under the cursor.
    // Always save + redraw so the cursor stays on top.

    x86_64::instructions::interrupts::without_interrupts(|| {
        if let Some(w) = crate::framebuffer::FRAMEBUFFER_WRITER.lock().as_mut() {
            let stride = w.info.stride;
            let bpp    = w.info.bytes_per_pixel;
            let fw     = w.info.horizontal_resolution;
            let fh     = w.info.vertical_resolution;
            let buf    = &mut w.back_buffer;

            let mut save = CURSOR_SAVE.lock();

            // Step 1 — restore pixels at previous cursor position.
            if lx != usize::MAX {
                for (i, &(dx, dy, _, _, _)) in CURSOR_PIXELS.iter().enumerate() {
                    let px = lx + dx; let py = ly + dy;
                    if px < fw && py < fh {
                        let off = (py * stride + px) * bpp;
                        let (r, g, b) = save[i];
                        buf[off]     = r;
                        buf[off + 1] = g;
                        buf[off + 2] = b;
                    }
                }
            }

            // Step 2 — save pixels at new cursor position, then draw cursor.
            for (i, &(dx, dy, cr, cg, cb)) in CURSOR_PIXELS.iter().enumerate() {
                let px = cx + dx; let py = cy + dy;
                if px < fw && py < fh {
                    let off = (py * stride + px) * bpp;
                    save[i] = (buf[off], buf[off + 1], buf[off + 2]);
                    buf[off]     = cr;
                    buf[off + 1] = cg;
                    buf[off + 2] = cb;
                } else {
                    save[i] = (0, 0, 0);
                }
            }
        }
    });

    CURSOR_LAST_X.store(cx, Ordering::Relaxed);
    CURSOR_LAST_Y.store(cy, Ordering::Relaxed);
}

// ======== Crash Notification Overlay ========

struct CrashNotification {
    label: String,
    expire_tick: u64,
    is_crash: bool,
}

lazy_static! {
    static ref CRASH_NOTIFICATIONS: Mutex<Vec<CrashNotification>> = Mutex::new(Vec::new());
}

/// Push a crash notification that will be drawn as a kernel overlay for ~5 seconds.
pub fn push_notification(name: &str, error: &str) {
    let expire_tick = crate::interrupts::TICKS.load(Ordering::Relaxed) + 100; // ~5.5s at 18Hz
    let raw = alloc::format!("{}: {}", name, error);
    let label = if raw.len() > 50 { raw[..50].to_string() } else { raw };
    let mut notifs = CRASH_NOTIFICATIONS.lock();
    notifs.push(CrashNotification { label, expire_tick, is_crash: true });
    if notifs.len() > 4 { notifs.remove(0); }
}

/// Push a user-initiated info toast (blue) for ~3 seconds.
pub fn push_toast(message: &str) {
    let expire_tick = crate::interrupts::TICKS.load(Ordering::Relaxed) + 54; // ~3s at 18Hz
    let label = if message.len() > 50 { message[..50].to_string() } else { message.to_string() };
    let mut notifs = CRASH_NOTIFICATIONS.lock();
    notifs.push(CrashNotification { label, expire_tick, is_crash: false });
    if notifs.len() > 4 { notifs.remove(0); }
}

/// Draw active crash notifications directly on the back buffer, on top of everything.
/// Called from the main loop after all process rendering, before swap_buffers().
pub fn draw_notification_overlay() {
    let current_tick = crate::interrupts::TICKS.load(Ordering::Relaxed);
    // Prune expired entries
    {
        let mut notifs = CRASH_NOTIFICATIONS.lock();
        notifs.retain(|n| current_tick < n.expire_tick);
        if notifs.is_empty() { return; }
    }

    let notifs = CRASH_NOTIFICATIONS.lock();
    let visible: Vec<&CrashNotification> = notifs.iter()
        .filter(|n| current_tick < n.expire_tick)
        .collect();
    if visible.is_empty() { return; }

    let (screen_w, _) = crate::framebuffer::get_resolution();
    let notif_w: usize = 370;
    let notif_h: usize = 22;
    let notif_x = screen_w.saturating_sub(notif_w + 6);

    for (i, notif) in visible.iter().enumerate() {
        let ny = 6 + i * (notif_h + 4);
        if notif.is_crash {
            // Drop shadow
            crate::framebuffer::fill_rect(notif_x + 2, ny + 2, notif_w, notif_h, 20, 5, 5);
            // Background + fill
            crate::framebuffer::fill_rect(notif_x, ny, notif_w, notif_h, 30, 10, 10);
            crate::framebuffer::fill_rect(notif_x + 1, ny + 1, notif_w - 2, notif_h - 2, 80, 22, 22);
            // Left accent bar
            crate::framebuffer::fill_rect(notif_x + 1, ny + 1, 3, notif_h - 2, 220, 55, 55);
            crate::framebuffer::set_foreground_color(255, 210, 210);
        } else {
            // Drop shadow
            crate::framebuffer::fill_rect(notif_x + 2, ny + 2, notif_w, notif_h, 5, 15, 40);
            // Background + fill
            crate::framebuffer::fill_rect(notif_x, ny, notif_w, notif_h, 15, 35, 80);
            crate::framebuffer::fill_rect(notif_x + 1, ny + 1, notif_w - 2, notif_h - 2, 25, 58, 135);
            // Left accent bar
            crate::framebuffer::fill_rect(notif_x + 1, ny + 1, 3, notif_h - 2, 75, 155, 255);
            crate::framebuffer::set_foreground_color(200, 225, 255);
        }
        // Text indented past accent bar
        crate::framebuffer::draw_string(&notif.label, notif_x + 8, ny + notif_h - 5);
    }
}

pub fn poll_timers() {
    let current_tick = crate::interrupts::TICKS.load(Ordering::Relaxed);

    let mut fired: Vec<(u32, String)> = Vec::new();

    {
        let mut queue = TIMER_QUEUE.lock();
        let mut i = 0;
        while i < queue.len() {
            if current_tick >= queue[i].fire_at_tick {
                let entry = queue.remove(i);
                fired.push((entry.pid, entry.timer_id));
            } else {
                i += 1;
            }
        }
    }

    for (pid, timer_id) in fired {
        let info = {
            let list = crate::process::PROCESS_LIST.lock();
            list.get(&pid).map(|p| (p.sandbox.clone(), p.name.clone()))
        };
        if let Some((sandbox_arc, name)) = info {
            let mut sandbox = sandbox_arc.lock();
            let script = format!(
                "if (typeof globalThis.__fireTimer === 'function') {{ globalThis.__fireTimer('{}'); }}",
                timer_id
            );
            sandbox.start_timeslice();
            match sandbox.eval(&script) {
                Ok(_) => {}
                Err(ref e) if e.contains(JS_INTERRUPT_MSG) => {
                    // preempted mid-timer callback — not a crash
                    crate::serial_println!("[sched] preempted timer for pid={}", pid);
                }
                Err(e) => {
                    drop(sandbox);
                    crate::process::crash_process(pid, &name, &e);
                }
            }
        }
    }
}

/// Returns true if the given PID owns at least one window.
pub fn process_has_windows(pid: u32) -> bool {
    WINDOW_BUFFERS.lock().values().any(|w| w.owner_pid == pid)
}

/// Returns true if the given PID has at least one pending timer.
pub fn process_has_timers(pid: u32) -> bool {
    TIMER_QUEUE.lock().iter().any(|t| t.pid == pid)
}

/// Cleans up all resources (windows, timers) owned by a specific PID.
pub fn cleanup_process_resources(pid: u32) {
    // 1. Remove windows and collect their IDs for canvas cleanup
    let removed_win_ids: Vec<u32> = {
        let mut buffers = WINDOW_BUFFERS.lock();
        let keys_to_remove: Vec<u32> = buffers.iter()
            .filter(|(_, win)| win.owner_pid == pid)
            .map(|(id, _)| *id)
            .collect();
        for id in &keys_to_remove {
            buffers.remove(id);
        }
        keys_to_remove
    };

    // 2. Remove timers
    {
        let mut queue = TIMER_QUEUE.lock();
        queue.retain(|entry| entry.pid != pid);
    }

    // 3. Remove canvas contexts for this process's windows
    {
        let mut ctxs = CANVAS_CONTEXTS.lock();
        for id in removed_win_ids {
            ctxs.remove(&id);
        }
    }
}

// ======== QuickJS Sandbox ========

struct TimesliceState {
    start_tick: AtomicU64,
    budget_ticks: AtomicU64,
}

unsafe extern "C" fn js_timeslice_interrupt_handler(
    _rt: *mut JSRuntime,
    opaque: *mut c_void,
) -> c_int {
    let state = &*(opaque as *const TimesliceState);
    let elapsed = crate::interrupts::TICKS
        .load(Ordering::Relaxed)
        .saturating_sub(state.start_tick.load(Ordering::Relaxed));
    if elapsed > state.budget_ticks.load(Ordering::Relaxed) { 1 } else { 0 }
}

pub struct QuickJsSandbox {
    rt: *mut JSRuntime,
    ctx: *mut JSContext,
    timeslice: Box<TimesliceState>,
}

unsafe impl Send for QuickJsSandbox {}
unsafe impl Sync for QuickJsSandbox {}

impl QuickJsSandbox {
    pub fn new() -> Result<Self, &'static str> {
        unsafe {
            let rt = JS_NewRuntime();
            if rt.is_null() { return Err("Failed to create JS runtime"); }
            let ctx = JS_NewContext(rt);
            if ctx.is_null() {
                JS_FreeRuntime(rt);
                return Err("Failed to create JS context");
            }

            let timeslice = Box::new(TimesliceState {
                start_tick: AtomicU64::new(crate::interrupts::TICKS.load(Ordering::Relaxed)),
                budget_ticks: AtomicU64::new(u64::MAX), // disabled during init; enabled after spawn
            });
            let opaque = &*timeslice as *const TimesliceState as *mut c_void;
            JS_SetInterruptHandler(rt, Some(js_timeslice_interrupt_handler), opaque);

            // Register os.* namespace
            register_os_namespace(ctx);
            register_console(ctx);

            // Inject require and process polyfills
            let polyfill_src = "
                globalThis._requireCache = {};
                globalThis.require = function(moduleName) {
                    if (globalThis._requireCache[moduleName]) {
                        return globalThis._requireCache[moduleName].exports;
                    }
                    const source = os.store.get(moduleName);
                    if (source === undefined || source === null) {
                        throw new Error(\"Cannot find module '\" + moduleName + \"'\");
                    }
                    const module = { exports: {} };
                    globalThis._requireCache[moduleName] = module;
                    const wrapper = eval(\"(function(exports, require, module, __filename, __dirname) { \" + source + \"\\n})\");
                    wrapper(module.exports, globalThis.require, module, moduleName, \"/\");
                    return module.exports;
                };
                globalThis.process = {
                    env: {},
                    get pid() { return globalThis.__PID; },
                    uptime: function() { return os.uptime(); },
                    stdout: { write: function(msg) { os.log(msg); } },
                    stderr: { write: function(msg) { os.log(\"ERR: \" + msg); } }
                };

                // Network Fetch Promise Polyfill
                globalThis.__fetchHandlers = {};
                globalThis.__onFetchResponse = function(url, status, text) {
                    const resolve = globalThis.__fetchHandlers[url];
                    if (resolve) {
                        resolve(new Response(status, text));
                        delete globalThis.__fetchHandlers[url];
                    }
                };

                function Response(status, body) {
                    this.status = status != null ? status : 200;
                    this.ok = this.status >= 200 && this.status < 400;
                    this.headers = {};
                    this._body = body || '';
                }
                Response.prototype.text = function() { return Promise.resolve(this._body); };
                Response.prototype.json = function() {
                    try { return Promise.resolve(JSON.parse(this._body)); }
                    catch(e) { return Promise.reject(new SyntaxError('Invalid JSON: ' + e.message)); }
                };
                Response.prototype.arrayBuffer = function() {
                    const enc = new TextEncoder();
                    return Promise.resolve(enc.encode(this._body).buffer);
                };
                globalThis.Response = Response;

                os.fetch = function(url, options) {
                    options = options || {};
                    const method = options.method || 'GET';
                    const body = options.body || '';
                    const headers = options.headers || {};
                    const headersJson = JSON.stringify(headers);
                    const alpnJson = options.alpn ? JSON.stringify(options.alpn) : '[]';
                    return new Promise(function(resolve, reject) {
                        globalThis.__fetchHandlers[url] = resolve;
                        os.fetchNative(url, method, body, headersJson, alpnJson);
                    });
                };

                // Timer Polyfills
                globalThis.__timers = {};
                globalThis.__timerCounter = 0;
                globalThis.__fireTimer = function(id) {
                    const timer = globalThis.__timers[id];
                    if (!timer) return;
                    if (timer.interval) {
                        // Re-schedule before execution to maintain cadence
                        os._setTimeout(String(globalThis.__PID), id, String(timer.ms));
                    }
                    try { timer.func(); } catch(e) { os.log(\"Timer Error: \" + e); }
                    if (!timer.interval) {
                        delete globalThis.__timers[id];
                    }
                };
                globalThis.setTimeout = function(fn, ms) {
                    const id = \"t\" + (++globalThis.__timerCounter);
                    globalThis.__timers[id] = { func: fn, interval: false, ms: ms };
                    os._setTimeout(String(globalThis.__PID), id, String(ms));
                    return id;
                };
                globalThis.setInterval = function(fn, ms) {
                    const id = \"t\" + (++globalThis.__timerCounter);
                    globalThis.__timers[id] = { func: fn, interval: true, ms: ms };
                    os._setTimeout(String(globalThis.__PID), id, String(ms));
                    return id;
                };
                globalThis.clearTimeout = globalThis.clearInterval = function(id) {
                    delete globalThis.__timers[id];
                };
                
                // Compatibility Alias
                globalThis.window = globalThis;
            ";
            let c_poly = js_cstring(polyfill_src);
            let fname = js_cstring("<polyfill>");
            let val = JS_Eval(ctx, c_poly.as_ptr() as *const c_char, c_poly.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
            if js_is_exception(val) {
                let ex = JS_GetException(ctx);
                let msg = js_to_rust_string(ctx, ex);
                crate::serial_println!("[QuickJS] Polyfill Exception: {}", msg);
                JS_FreeValue(ctx, ex);
            }
            JS_FreeValue(ctx, val);

            {
                let src = include_str!("js/globals_compat.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_compat>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_compat error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }
            {
                let src = include_str!("js/globals_websocket.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_websocket>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_websocket error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }
            {
                let src = include_str!("js/globals_storage.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_storage>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_storage error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }
            // globals_encoding: TextEncoder, TextDecoder, crypto
            {
                let src = include_str!("js/globals_encoding.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_encoding>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_encoding error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }
            // globals_date: Date polyfill backed by os.rtc() and os.uptime()
            {
                let src = include_str!("js/globals_date.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_date>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_date error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }
            // globals_console_url: console extensions, URL, URLSearchParams
            {
                let src = include_str!("js/globals_console_url.js");
                let c_src = js_cstring(src);
                let fname = js_cstring("<globals_console_url>");
                let v = JS_Eval(ctx, c_src.as_ptr() as *const c_char, c_src.len() - 1, fname.as_ptr() as *const c_char, JS_EVAL_TYPE_GLOBAL);
                if js_is_exception(v) {
                    let ex = JS_GetException(ctx);
                    crate::serial_println!("[QuickJS] globals_console_url error: {}", js_to_rust_string(ctx, ex));
                    JS_FreeValue(ctx, ex);
                }
                JS_FreeValue(ctx, v);
            }

            Ok(Self { rt, ctx, timeslice })
        }
    }

    pub fn start_timeslice(&mut self) {
        self.timeslice.start_tick.store(
            crate::interrupts::TICKS.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }

    /// Enable preemption after initialization is complete. Called once after the
    /// initial app eval succeeds. Sets the real budget and resets the slice start.
    pub fn enable_preemption(&mut self) {
        self.timeslice.budget_ticks.store(3, Ordering::Relaxed);
        self.start_timeslice();
    }

    pub fn eval(&mut self, script: &str) -> Result<String, String> {
        unsafe {
            let filename = js_cstring("<eval>");
            let c_script = js_cstring(script);

            let val = JS_Eval(
                self.ctx,
                c_script.as_ptr() as *const c_char,
                script.len(),
                filename.as_ptr() as *const c_char,
                JS_EVAL_TYPE_GLOBAL,
            );

            if js_is_exception(val) {
                let exc = JS_GetException(self.ctx);
                let msg = js_to_rust_string(self.ctx, exc);
                JS_FreeValue(self.ctx, exc);
                if !msg.contains(JS_INTERRUPT_MSG) {
                    crate::serial_println!("JS Error: {}", msg);
                }
                return Err(msg);
            }

            let result = js_to_rust_string(self.ctx, val);
            JS_FreeValue(self.ctx, val);
            Ok(result)
        }
    }

    pub fn execute_pending_jobs(&mut self) -> Result<(), String> {
        unsafe {
            let mut pctx: *mut JSContext = core::ptr::null_mut();
            loop {
                let ret = JS_ExecutePendingJob(self.rt, &mut pctx);
                if ret == 0 { break; }
                if ret < 0 {
                    let ctx = if pctx.is_null() { self.ctx } else { pctx };
                    let exc = JS_GetException(ctx);
                    let msg = js_to_rust_string(ctx, exc);
                    JS_FreeValue(ctx, exc);
                    if msg.contains(JS_INTERRUPT_MSG) {
                        // Preempted — not a crash. Resume next frame.
                        return Ok(());
                    }
                    crate::serial_println!("JS Async Error: {}", msg);
                    return Err(msg);
                }
            }
            Ok(())
        }
    }

    pub fn run_gc(&mut self) {
        // QuickJS handles GC automatically via reference counting
    }
}

impl Drop for QuickJsSandbox {
    fn drop(&mut self) {
        unsafe {
            JS_FreeContext(self.ctx);
            JS_FreeRuntime(self.rt);
        }
    }
}

// ======== Register os.* namespace ========


unsafe extern "C" fn js_os_mouse_scroll(
    _ctx: *mut JSContext,
    _this: JSValue,
    _argc: c_int,
    _argv: *const JSValue,
) -> JSValue {
    // Return accumulated scroll delta and atomically reset to 0.
    // Positive = wheel rolled down (scroll content up = show older lines).
    // Negative = wheel rolled up  (scroll content down = show newer lines).
    let delta = crate::mouse::MOUSE_SCROLL.swap(0, Ordering::Relaxed);
    js_int(delta)
}

unsafe fn register_os_namespace(ctx: *mut JSContext) {
    let global = JS_GetGlobalObject(ctx);

    // Build os.graphics sub-object
    let graphics = JS_NewObject(ctx);
    set_func(ctx, graphics, "fillRect", js_os_graphics_fill_rect, 7);
    set_func(ctx, graphics, "drawString", js_os_graphics_draw_string, 7);
    set_func(ctx, graphics, "clear", js_os_graphics_clear, 0);
    set_func(ctx, graphics, "screenshot", js_os_graphics_screenshot, 0);

    // Build os.net sub-object
    let net = JS_NewObject(ctx);
    set_func(ctx, net, "listen", js_os_net_listen, 1);
    set_func(ctx, net, "config", js_os_net_config, 0);
    set_func(ctx, net, "serveStatic", js_os_net_serve_static, 1);

    // Build os.store sub-object
    let store = JS_NewObject(ctx);
    set_func(ctx, store, "set", js_os_store_set, 2);
    set_func(ctx, store, "get", js_os_store_get, 1);
    set_func(ctx, store, "getBytes", js_os_store_get_bytes, 1);
    set_func(ctx, store, "setBytes", js_os_store_set_bytes, 2);
    set_func(ctx, store, "list", js_os_store_list, 0);
    set_func(ctx, store, "delete", js_os_store_delete, 1);

    // Build os.window sub-object
    let window = JS_NewObject(ctx);
    set_func(ctx, window, "create", js_os_window_create, 5);
    set_func(ctx, window, "drawRect", js_os_window_draw_rect, 8);
    set_func(ctx, window, "drawString", js_os_window_draw_string, 8);
    set_func(ctx, window, "drawStringUnicode", js_os_window_draw_string_unicode, 7);
    set_func(ctx, window, "drawLine", js_os_window_draw_line, 8);
    set_func(ctx, window, "drawCircle", js_os_window_draw_circle, 7);
    set_func(ctx, window, "fillCircle", js_os_window_fill_circle, 7);
    set_func(ctx, window, "flush", js_os_window_flush, 1);
    set_func(ctx, window, "getPixelBuffer", js_os_window_get_pixel_buffer, 1);
    set_func(ctx, window, "setCursor", js_os_window_set_cursor, 2);
    set_func(ctx, window, "list", js_os_window_list, 0);
    set_func(ctx, window, "move", js_os_window_move, 3);
    set_func(ctx, window, "setZIndex", js_os_window_set_z_index, 2);
    set_func(ctx, window, "getContext", js_os_window_get_context, 1);

    // Build os.mouse sub-object
    let mouse = JS_NewObject(ctx);
    set_func(ctx, mouse, "scroll", js_os_mouse_scroll, 0);

    // Build os.clipboard sub-object
    let clipboard = JS_NewObject(ctx);
    set_func(ctx, clipboard, "write", js_os_clipboard_write, 1);
    set_func(ctx, clipboard, "read", js_os_clipboard_read, 0);

    // Build the main `os` object
    let os = JS_NewObject(ctx);
    set_prop_obj(ctx, os, "graphics", graphics);
    set_prop_obj(ctx, os, "store", store);
    set_prop_obj(ctx, os, "window", window);
    set_prop_obj(ctx, os, "net", net);
    set_prop_obj(ctx, os, "mouse", mouse);
    set_prop_obj(ctx, os, "clipboard", clipboard);

    // Build os.base64 sub-object
    let base64 = JS_NewObject(ctx);
    set_func(ctx, base64, "encode", js_os_base64_encode, 1);
    set_func(ctx, base64, "decode", js_os_base64_decode, 1);
    set_func(ctx, base64, "decodeBytes", js_os_base64_decode_bytes, 1);
    set_prop_obj(ctx, os, "base64", base64);

    // Build os.image sub-object
    let image = JS_NewObject(ctx);
    set_func(ctx, image, "decode", js_os_image_decode, 1);
    set_prop_obj(ctx, os, "image", image);

    // Build os.websocket sub-object
    let websocket = JS_NewObject(ctx);
    set_func(ctx, websocket, "connect", js_os_websocket_connect, 1);
    set_func(ctx, websocket, "send", js_os_websocket_send, 2);
    set_func(ctx, websocket, "recv", js_os_websocket_recv, 1);
    set_func(ctx, websocket, "close", js_os_websocket_close, 1);
    set_func(ctx, websocket, "state", js_os_websocket_state, 1);
    set_prop_obj(ctx, os, "websocket", websocket);

    set_func(ctx, os, "log", js_os_log, 1);
    set_func(ctx, os, "spawn", js_os_spawn, 1);
    set_func(ctx, os, "processes", js_os_ps, 0);
    set_func(ctx, os, "sendIpc", js_os_send_ipc, 2);
    set_func(ctx, os, "awaken", js_os_awaken, 1);
    set_func(ctx, os, "_setTimeout", js_os_set_timeout, 3);
    set_func(ctx, os, "uptime", js_os_uptime, 0);
    set_func(ctx, os, "serialLog", js_os_serial_log, 0);
    set_func(ctx, os, "clear", js_os_clear, 0);
    set_func(ctx, os, "reboot", js_os_reboot, 0);
    set_func(ctx, os, "shutdown", js_os_shutdown, 0);
    set_func(ctx, os, "notify", js_os_notify, 1);
    set_func(ctx, os, "fetchNative", js_os_fetch, 1);
    set_func(ctx, os, "exit", js_os_exit, 0);
    set_func(ctx, os, "exec", js_os_exec, 1);
    set_func(ctx, os, "listBin", js_os_list_bin, 0);
    set_func(ctx, os, "sysinfo", js_os_sysinfo, 0);
    set_func(ctx, os, "rtc", js_os_rtc, 0);
    set_func(ctx, os, "screen", js_os_screen, 0);
    set_func(ctx, os, "randomBytes", js_os_random_bytes, 1);

    // Font size constants: pass as the optional last arg to drawString
    set_prop_obj(ctx, os, "FONT_SMALL", js_int(0));
    set_prop_obj(ctx, os, "FONT_LARGE", js_int(1));

    set_prop_obj(ctx, global, "os", os);
    JS_FreeValue(ctx, global);
}

unsafe fn register_console(ctx: *mut JSContext) {
    let global = JS_GetGlobalObject(ctx);
    let console = JS_NewObject(ctx);
    set_func(ctx, console, "log", js_console_log, 1);
    set_func(ctx, console, "warn", js_console_log, 1);
    set_func(ctx, console, "error", js_console_log, 1);
    set_func(ctx, console, "info", js_console_log, 1);
    set_prop_obj(ctx, global, "console", console);
    JS_FreeValue(ctx, global);
}

// ======== Native function implementations ========

unsafe extern "C" fn js_console_log(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    let mut parts: Vec<String> = Vec::new();
    for i in 0..argc {
        parts.push(js_to_rust_string(ctx, *argv.offset(i as isize)));
    }
    crate::serial_println!("{}", parts.join(" "));
    js_undefined()
}

unsafe extern "C" fn js_os_log(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc >= 1 {
        let msg = js_to_rust_string(ctx, *argv.offset(0));
        crate::serial_println!("[os.log] {}", msg);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_image_decode(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_null(); }

    let mut buf_size: usize = 0;
    let buf_ptr = JS_GetArrayBuffer(ctx, &mut buf_size, *argv.offset(0));
    if buf_ptr.is_null() { return js_null(); }

    let bytes = core::slice::from_raw_parts(buf_ptr, buf_size);
    let (width, height, pixels) = match crate::image::decode(bytes) {
        Some(v) => v,
        None => return js_null(),
    };

    // Transfer pixel Vec ownership to QuickJS via a zero-copy ArrayBuffer.
    // The free callback reconstructs the Vec from (ptr, opaque=count) and drops it.
    unsafe extern "C" fn free_image_pixels(_rt: *mut JSRuntime, opaque: *mut c_void, ptr: *mut c_void) {
        let count = opaque as usize;
        drop(Vec::from_raw_parts(ptr as *mut u32, count, count));
    }

    let pixel_count = pixels.len();
    let byte_len = pixel_count * 4;
    let raw_ptr = pixels.as_ptr() as *mut u8;
    core::mem::forget(pixels);

    let data_ab = JS_NewArrayBuffer(
        ctx,
        raw_ptr,
        byte_len,
        Some(free_image_pixels),
        pixel_count as *mut c_void,
        false,
    );

    let result = JS_NewObject(ctx);
    set_prop_obj(ctx, result, "width", js_int(width as i32));
    set_prop_obj(ctx, result, "height", js_int(height as i32));
    set_prop_obj(ctx, result, "data", data_ab);
    result
}

unsafe extern "C" fn js_os_spawn(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_int(0); }
    let name = js_to_rust_string(ctx, *argv.offset(0));

    // Read from persistent JSKV storage — built-ins and user scripts both live here.
    let source = crate::storage::read_object(&name)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .or_else(|| BINS.lock().get(&name).cloned()); // fallback: in-memory (shouldn't be needed after seeding)

    if let Some(code) = source {
        crate::serial_println!("[os.spawn] Spawning: {}", name);
        let pid = crate::process::spawn_process(&name, &code);
        js_int(pid as i32)
    } else {
        crate::serial_println!("[os.spawn] Unknown Binary {}", name);
        js_int(0)
    }
}

unsafe extern "C" fn js_os_send_ipc(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let pid = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let msg = js_to_rust_string(ctx, *argv.offset(1));
    let list = crate::process::PROCESS_LIST.lock();
    if let Some(process) = list.get(&pid) {
        process.ipc_queue.lock().push(msg);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_awaken(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    let pid = if argc >= 1 { js_val_to_i32(ctx, *argv.offset(0)) as u32 } else { 1 };
    crate::process::ACTIVE_FOREGROUND_PID.store(pid, Ordering::SeqCst);
    js_undefined()
}

unsafe extern "C" fn js_os_set_timeout(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 3 { return js_undefined(); }
    let pid_str = js_to_rust_string(ctx, *argv.offset(0));
    let timer_id = js_to_rust_string(ctx, *argv.offset(1));
    let ms_str = js_to_rust_string(ctx, *argv.offset(2));

    let pid: u32 = pid_str.parse().unwrap_or(0);
    let ms: u64 = ms_str.parse().unwrap_or(500);
    let ticks_delay = ms / 10;
    let current_tick = crate::interrupts::TICKS.load(Ordering::Relaxed);

    TIMER_QUEUE.lock().push(TimerEntry {
        pid,
        timer_id,
        fire_at_tick: current_tick + ticks_delay,
    });

    js_undefined()
}

unsafe extern "C" fn js_os_uptime(_ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let ticks = crate::interrupts::TICKS.load(Ordering::Relaxed);
    let seconds = ticks / crate::interrupts::TICKS_PER_SEC.load(Ordering::Relaxed).max(1);
    js_int(seconds as i32)
}

unsafe extern "C" fn js_os_serial_log(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let s = crate::serial::serial_log_snapshot();
    let cs = js_cstring(&s);
    JS_NewStringLen(ctx, cs.as_ptr() as *const c_char, cs.len() - 1)
}

unsafe extern "C" fn js_os_clear(_ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    crate::framebuffer::clear_screen();
    // Invalidate the cursor save buffer — the back buffer has been fully redrawn
    // so the saved pixels from under the old cursor position are now stale.
    crate::js_runtime::invalidate_cursor_save();
    js_undefined()
}

#[allow(unreachable_code)]
unsafe extern "C" fn js_os_reboot(_ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    crate::power::reboot();
    js_undefined()
}

#[allow(unreachable_code)]
unsafe extern "C" fn js_os_shutdown(_ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    crate::power::shutdown();
    js_undefined()
}

unsafe extern "C" fn js_os_notify(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let msg = js_to_rust_string(ctx, *argv.offset(0));
    push_toast(&msg);
    js_undefined()
}

unsafe extern "C" fn js_os_fetch(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc >= 1 {
        let url = js_to_rust_string(ctx, *argv.offset(0));
        let method = if argc >= 2 { js_to_rust_string(ctx, *argv.offset(1)) } else { "GET".into() };
        let body = if argc >= 3 { js_to_rust_string(ctx, *argv.offset(2)) } else { "".into() };
        let headers_json = if argc >= 4 { js_to_rust_string(ctx, *argv.offset(3)) } else { "{}".into() };

        let alpn_protocols = if argc >= 5 {
            let alpn_json = js_to_rust_string(ctx, *argv.offset(4));
            parse_json_string_array(&alpn_json)
        } else {
            None
        };

        crate::serial_println!("[os.fetch] Fetch requested: {} {} (body len: {}, headers: {})", method, url, body.len(), headers_json);

        // Get the caller's PID from globalThis.__PID
        let global = JS_GetGlobalObject(ctx);
        let pid_prop = js_cstring("__PID");
        let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
        let pid = js_val_to_i32(ctx, pid_val) as u32;
        JS_FreeValue(ctx, pid_val);
        JS_FreeValue(ctx, global);

        crate::net::start_fetch(pid, &url, &method, &body, &headers_json, alpn_protocols);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_net_listen(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let port = js_val_to_i32(ctx, *argv.offset(0)) as u16;
    let global = JS_GetGlobalObject(ctx);
    let pid_prop = js_cstring("__PID");
    let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
    let pid = js_val_to_i32(ctx, pid_val) as u32;
    JS_FreeValue(ctx, pid_val);
    JS_FreeValue(ctx, global);
    crate::net::start_listen(pid, port);
    js_undefined()
}

unsafe extern "C" fn js_os_net_config(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let s = js_cstring(&crate::net::get_net_info());
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_net_serve_static(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc >= 1 {
        let html = js_to_rust_string(ctx, *argv.offset(0));
        crate::net::set_http_response(html);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_graphics_screenshot(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let data = crate::framebuffer::get_screenshot_bmp_small();
    let hex = data.iter().map(|b| alloc::format!("{:02x}", b)).collect::<alloc::string::String>();
    let s = js_cstring(&hex);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_exit(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    let pid = if argc >= 1 {
        js_val_to_i32(ctx, *argv.offset(0)) as u32
    } else {
        // Fallback: search for __PID in global scope
        let global = JS_GetGlobalObject(ctx);
        let pid_prop = js_cstring("__PID");
        let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
        let p = js_val_to_i32(ctx, pid_val) as u32;
        JS_FreeValue(ctx, pid_val);
        JS_FreeValue(ctx, global);
        p
    };

    if pid > 0 {
        crate::process::kill_process_and_cleanup(pid);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_exec(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    let code = KV_STORE.lock().get(&key).cloned();
    if let Some(code) = code {
        crate::serial_println!("[os.exec] Executing stored code: {}", key);
        crate::process::spawn_process(&key, &code);
    } else {
        crate::serial_println!("[os.exec] Key not found: {}", key);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_list_bin(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    // List all .jsos keys from persistent storage (built-ins + user scripts).
    let mut keys: Vec<String> = crate::storage::list_objects()
        .into_iter()
        .filter(|k| k.ends_with(".jsos"))
        .collect();

    // Safety net: include any embedded BINS not yet seeded into storage.
    for key in BINS.lock().keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }

    keys.sort();
    let json = format!("[{}]", keys.iter().map(|k| format!("\"{}\"", k)).collect::<Vec<String>>().join(","));
    let s = js_cstring(&json);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_ps(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let list = crate::process::PROCESS_LIST.lock();
    let mut procs = Vec::new();
    for (pid, proc) in list.iter() {
        procs.push(format!("{{\"pid\":{},\"name\":\"{}\"}}", pid, proc.name));
    }
    let json = format!("[{}]", procs.join(","));
    let s = js_cstring(&json);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_graphics_fill_rect(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 7 { return js_undefined(); }
    let x = js_val_to_i32(ctx, *argv.offset(0)) as usize;
    let y = js_val_to_i32(ctx, *argv.offset(1)) as usize;
    let w = js_val_to_i32(ctx, *argv.offset(2)) as usize;
    let h = js_val_to_i32(ctx, *argv.offset(3)) as usize;
    let r = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    crate::graphics::Graphics::fill_rect(x, y, w, h, r, g, b);
    js_undefined()
}

unsafe extern "C" fn js_os_graphics_draw_string(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 6 { return js_undefined(); }
    let text = js_to_rust_string(ctx, *argv.offset(0));
    let x = js_val_to_i32(ctx, *argv.offset(1)) as usize;
    let y = js_val_to_i32(ctx, *argv.offset(2)) as usize;
    let r = js_val_to_i32(ctx, *argv.offset(3)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let large = argc >= 7 && js_val_to_i32(ctx, *argv.offset(6)) != 0;
    crate::framebuffer::set_foreground_color(r, g, b);
    crate::framebuffer::draw_string_sized(&text, x, y, large);
    js_undefined()
}

unsafe extern "C" fn js_os_graphics_clear(_ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    crate::framebuffer::clear_screen();
    js_undefined()
}

unsafe extern "C" fn js_os_store_set(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    let val = js_to_rust_string(ctx, *argv.offset(1));
    KV_STORE.lock().insert(key.clone(), val.clone());
    crate::storage::write_object(&key, val.as_bytes());
    js_undefined()
}

unsafe extern "C" fn js_os_store_get(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    
    if let Some(data) = crate::storage::read_object(&key) {
        let val_str = alloc::string::String::from_utf8_lossy(&data).to_string();
        let s = js_cstring(&val_str);
        return JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1);
    }
    
    match KV_STORE.lock().get(&key) {
        Some(val) => {
            let s = js_cstring(val);
            JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
        }
        None => js_undefined(),
    }
}

unsafe extern "C" fn js_os_store_set_bytes(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    let mut buf_size: usize = 0;
    let buf_ptr = JS_GetArrayBuffer(ctx, &mut buf_size, *argv.offset(1));
    if buf_ptr.is_null() { return js_undefined(); }
    let bytes = core::slice::from_raw_parts(buf_ptr, buf_size);
    crate::storage::write_object(&key, bytes);
    js_undefined()
}

unsafe extern "C" fn js_os_store_get_bytes(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    
    if let Some(data) = crate::storage::read_object(&key) {
        let ab = JS_NewArrayBufferCopy(ctx, data.as_ptr() as *const u8, data.len());
        return ab;
    }

    // Fall back to compile-time embedded binary blobs (images etc. on systems without JSKV disk).
    if let Some(data) = BINARY_BINS.lock().get(&key) {
        return JS_NewArrayBufferCopy(ctx, data.as_ptr() as *const u8, data.len());
    }

    match KV_STORE.lock().get(&key) {
        Some(val) => {
            let s = js_cstring(val);
            JS_NewArrayBufferCopy(ctx, s.as_ptr() as *const u8, s.len() - 1)
        }
        None => js_undefined(),
    }
}

unsafe extern "C" fn js_os_store_list(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let mut keys: Vec<String> = KV_STORE.lock().keys().cloned().collect();
    let persistent_keys = crate::storage::list_objects();
    for pk in persistent_keys {
        if !keys.contains(&pk) {
            keys.push(pk);
        }
    }
    for bk in BINARY_BINS.lock().keys() {
        if !keys.contains(bk) {
            keys.push(bk.clone());
        }
    }
    
    let json = format!("[{}]", keys.iter().map(|k| format!("\"{}\"", k)).collect::<Vec<String>>().join(","));
    let s = js_cstring(&json);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_store_delete(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let key = js_to_rust_string(ctx, *argv.offset(0));
    KV_STORE.lock().remove(&key);
    crate::storage::delete_object(&key);
    js_undefined()
}

unsafe extern "C" fn js_os_clipboard_write(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc >= 1 {
        let text = js_to_rust_string(ctx, *argv.offset(0));
        *CLIPBOARD.lock() = text;
    }
    js_undefined()
}

unsafe extern "C" fn js_os_clipboard_read(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let text = CLIPBOARD.lock().clone();
    let s = js_cstring(&text);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_websocket_connect(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_int(-1); }
    let url = js_to_rust_string(ctx, *argv.offset(0));
    
    let global = JS_GetGlobalObject(ctx);
    let pid_prop = js_cstring("__PID");
    let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
    let pid = js_val_to_i32(ctx, pid_val) as u32;
    JS_FreeValue(ctx, pid_val);
    JS_FreeValue(ctx, global);

    let handle = crate::net::start_websocket(pid, &url, None);
    js_int(handle)
}

unsafe extern "C" fn js_os_base64_encode(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let s = js_to_rust_string(ctx, *argv.offset(0));
    let encoded = crate::net::base64_encode(s.as_bytes());
    let cs = js_cstring(&encoded);
    JS_NewStringLen(ctx, cs.as_ptr() as *const c_char, cs.len() - 1)
}

unsafe extern "C" fn js_os_base64_decode(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let s = js_to_rust_string(ctx, *argv.offset(0));
    let decoded = crate::net::base64_decode(&s);
    // Return as a string for simplicity in the shell, though Vec<u8> might be more general.
    // Given the prompt "commandline base64", usually people expect string input/output.
    let decoded_str = String::from_utf8_lossy(&decoded).to_string();
    let cs = js_cstring(&decoded_str);
    JS_NewStringLen(ctx, cs.as_ptr() as *const c_char, cs.len() - 1)
}

unsafe extern "C" fn js_os_base64_decode_bytes(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let s = js_to_rust_string(ctx, *argv.offset(0));
    let decoded = crate::net::base64_decode(&s);
    let ab = JS_NewArrayBufferCopy(ctx, decoded.as_ptr() as *const u8, decoded.len());
    ab
}

unsafe extern "C" fn js_os_websocket_send(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let handle = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let text = js_to_rust_string(ctx, *argv.offset(1));
    
    let mut jobs = crate::net::WEBSOCKET_JOBS.lock();
    if let Some(job) = jobs.get_mut(&handle) {
        job.tx_queue.push(text);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_websocket_recv(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let handle = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    
    let mut jobs = crate::net::WEBSOCKET_JOBS.lock();
    if let Some(job) = jobs.get_mut(&handle) {
        if !job.rx_queue.is_empty() {
            let msg = job.rx_queue.remove(0);
            let s = js_cstring(&msg);
            return JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1);
        }
    }
    js_undefined() // null in JS
}

unsafe extern "C" fn js_os_websocket_close(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let handle = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    
    let mut jobs = crate::net::WEBSOCKET_JOBS.lock();
    if let Some(job) = jobs.get_mut(&handle) {
        job.closing = true;
        job.closed = true;
    }
    js_undefined()
}

unsafe extern "C" fn js_os_websocket_state(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let handle = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    
    let jobs = crate::net::WEBSOCKET_JOBS.lock();
    let state_str = if let Some(job) = jobs.get(&handle) {
        if job.closed {
            "closed"
        } else if job.state == 8 {
            "open"
        } else {
            "connecting"
        }
    } else {
        "closed"
    };
    
    let s = js_cstring(state_str);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_window_create(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 4 { return js_int(0); }
    let x = js_val_to_i32(ctx, *argv.offset(0)) as usize;
    let y = js_val_to_i32(ctx, *argv.offset(1)) as usize;
    let width = js_val_to_i32(ctx, *argv.offset(2)) as usize;
    let height = js_val_to_i32(ctx, *argv.offset(3)) as usize;

    // Get the caller's PID from globalThis.__PID
    let global = JS_GetGlobalObject(ctx);
    let pid_prop = js_cstring("__PID");
    let pid_val = JS_GetPropertyStr(ctx, global, pid_prop.as_ptr() as *const c_char);
    let owner_pid = js_val_to_i32(ctx, pid_val) as u32;
    JS_FreeValue(ctx, pid_val);
    JS_FreeValue(ctx, global);
    
    let win_id = NEXT_WINDOW_ID.fetch_add(1, Ordering::SeqCst) as u32;
    let z_index = { let mut z = NEXT_Z_INDEX.lock(); let v = *z; *z += 1; v };
    crate::serial_println!("[os.window] create(x={}, y={}, w={}, h={}) for PID {} -> ID {}", x, y, width, height, owner_pid, win_id);
    let buf = WindowBuffer {
        x, y, width, height,
        pixels: alloc::vec![0; width * height],
        owner_pid,
        z_index,
        pixel_buffer_active: false,
    };
    
    WINDOW_BUFFERS.lock().insert(win_id, buf);
    js_int(win_id as i32)
}

unsafe extern "C" fn js_os_window_draw_rect(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 8 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let rel_x = js_val_to_i32(ctx, *argv.offset(1)) as isize;
    let rel_y = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let w = js_val_to_i32(ctx, *argv.offset(3)) as usize;
    let h = js_val_to_i32(ctx, *argv.offset(4)) as usize;
    let r = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(7)).clamp(0, 255) as u8;

    // Draw directly to the framebuffer back-buffer using the fast row-optimized fill_rect
    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        let abs_x = (win.x as isize + rel_x).max(0) as usize;
        let abs_y = (win.y as isize + rel_y).max(0) as usize;
        crate::framebuffer::fill_rect(abs_x, abs_y, w, h, r, g, b);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_draw_string(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 7 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let text = js_to_rust_string(ctx, *argv.offset(1));
    let rel_x = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let rel_y = js_val_to_i32(ctx, *argv.offset(3)) as isize;
    let r = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    
    let large = argc >= 8 && js_val_to_i32(ctx, *argv.offset(7)) != 0;
    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        let abs_x = (win.x as isize + rel_x).max(0) as usize;
        let abs_y = (win.y as isize + rel_y).max(0) as usize;
        crate::framebuffer::set_foreground_color(r, g, b);
        crate::framebuffer::draw_string_sized(&text, abs_x, abs_y, large);
    }
    js_undefined()
}

/// os.window.drawStringUnicode(winId, text, relX, relY, r, g, b)
/// Renders text with per-character font fallback: multilingual → symbols → emoticons → placeholder box.
/// SMP emoji (U+10000+) render as U+25A1 (□). BMP symbols (★☀✔♫) render as proper glyphs.
unsafe extern "C" fn js_os_window_draw_string_unicode(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 7 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let text = js_to_rust_string(ctx, *argv.offset(1));
    let rel_x = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let rel_y = js_val_to_i32(ctx, *argv.offset(3)) as isize;
    let r = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;

    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        let abs_x = (win.x as isize + rel_x).max(0) as usize;
        let abs_y = (win.y as isize + rel_y).max(0) as usize;
        crate::framebuffer::set_foreground_color(r, g, b);
        crate::framebuffer::draw_string_unicode(&text, abs_x, abs_y);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_draw_line(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 8 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let x0 = js_val_to_i32(ctx, *argv.offset(1)) as isize;
    let y0 = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let x1 = js_val_to_i32(ctx, *argv.offset(3)) as isize;
    let y1 = js_val_to_i32(ctx, *argv.offset(4)) as isize;
    let r = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(7)).clamp(0, 255) as u8;
    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        let ox = win.x as isize;
        let oy = win.y as isize;
        crate::framebuffer::draw_line(ox + x0, oy + y0, ox + x1, oy + y1, r, g, b);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_draw_circle(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 7 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let cx = js_val_to_i32(ctx, *argv.offset(1)) as isize;
    let cy = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let radius = js_val_to_i32(ctx, *argv.offset(3)).max(0) as isize;
    let r = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        crate::framebuffer::draw_circle(win.x as isize + cx, win.y as isize + cy, radius, r, g, b);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_fill_circle(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 7 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let cx = js_val_to_i32(ctx, *argv.offset(1)) as isize;
    let cy = js_val_to_i32(ctx, *argv.offset(2)) as isize;
    let radius = js_val_to_i32(ctx, *argv.offset(3)).max(0) as isize;
    let r = js_val_to_i32(ctx, *argv.offset(4)).clamp(0, 255) as u8;
    let g = js_val_to_i32(ctx, *argv.offset(5)).clamp(0, 255) as u8;
    let b = js_val_to_i32(ctx, *argv.offset(6)).clamp(0, 255) as u8;
    if let Some(win) = WINDOW_BUFFERS.lock().get(&win_id) {
        crate::framebuffer::fill_circle(win.x as isize + cx, win.y as isize + cy, radius, r, g, b);
    }
    js_undefined()
}

/// No-op free callback: the Vec<u32> in WindowBuffer owns the pixel memory.
/// QuickJS calls this when the ArrayBuffer is GC'd; we leave the Vec intact.
unsafe extern "C" fn pixel_buffer_free(_rt: *mut JSRuntime, _opaque: *mut c_void, _ptr: *mut c_void) {}


unsafe extern "C" fn js_os_window_get_pixel_buffer(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;

    let (ptr, byte_len) = {
        let mut buffers = WINDOW_BUFFERS.lock();
        match buffers.get_mut(&win_id) {
            Some(win) => {
                win.pixel_buffer_active = true;
                (win.pixels.as_mut_ptr() as *mut u8, win.pixels.len() * 4)
            }
            None => return js_undefined(),
        }
        // Lock released here; pointer into Vec remains valid as long as the
        // Vec is not moved/dropped (it isn't — fixed size, lives in WINDOW_BUFFERS).
    };

    // Return the raw ArrayBuffer backed by the window's pixel Vec.
    // JS callers wrap it themselves: new Uint32Array(os.window.getPixelBuffer(winId))
    // This avoids calling JS_NewTypedArray from FFI, which hangs inside QuickJS.
    JS_NewArrayBuffer(ctx, ptr, byte_len, Some(pixel_buffer_free), core::ptr::null_mut(), false)
}

unsafe extern "C" fn js_os_window_flush(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;

    // Collect position/size and clone the pixel data before releasing the lock,
    // since blit_window_pixels holds FRAMEBUFFER_WRITER which may also be locked.
    let blit_args = {
        let buffers = WINDOW_BUFFERS.lock();
        buffers.get(&win_id)
            .filter(|win| win.pixel_buffer_active)
            .map(|win| (win.x, win.y, win.width, win.height, win.pixels.clone()))
    };

    if let Some((x, y, w, h, pixels)) = blit_args {
        crate::framebuffer::blit_window_pixels(x, y, w, h, &pixels);
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_set_z_index(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let z      = js_val_to_i32(ctx, *argv.offset(1)) as u32;
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        win.z_index = z;
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_get_context(
    ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;

    // Create CanvasContext for this window if not yet created
    {
        let mut ctxs = CANVAS_CONTEXTS.lock();
        if !ctxs.contains_key(&win_id) {
            ctxs.insert(win_id, crate::canvas::CanvasContext::new(win_id));
        }
    }
    // Mark pixel buffer active so flush() will blit
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        win.pixel_buffer_active = true;
    }

    // Build the JS context object
    let obj = JS_NewObject(ctx);
    set_prop_obj(ctx, obj, "_id",         js_int(win_id as i32));
    set_prop_obj(ctx, obj, "fillStyle",   js_str(ctx, "#000000"));
    set_prop_obj(ctx, obj, "strokeStyle", js_str(ctx, "#000000"));
    set_prop_obj(ctx, obj, "lineWidth",   js_float(1.0));
    set_prop_obj(ctx, obj, "font",        js_str(ctx, "16px monospace"));

    set_func(ctx, obj, "fillRect",           js_canvas_fill_rect,           4);
    set_func(ctx, obj, "strokeRect",         js_canvas_stroke_rect,         4);
    set_func(ctx, obj, "clearRect",          js_canvas_clear_rect,          4);
    set_func(ctx, obj, "beginPath",          js_canvas_begin_path,          0);
    set_func(ctx, obj, "moveTo",             js_canvas_move_to,             2);
    set_func(ctx, obj, "lineTo",             js_canvas_line_to,             2);
    set_func(ctx, obj, "arc",                js_canvas_arc,                 5);
    set_func(ctx, obj, "bezierCurveTo",      js_canvas_bezier_curve_to,     6);
    set_func(ctx, obj, "quadraticCurveTo",   js_canvas_quadratic_curve_to,  4);
    set_func(ctx, obj, "rect",               js_canvas_rect_path,           4);
    set_func(ctx, obj, "closePath",          js_canvas_close_path,          0);
    set_func(ctx, obj, "fill",               js_canvas_fill,                0);
    set_func(ctx, obj, "stroke",             js_canvas_stroke,              0);
    set_func(ctx, obj, "fillText",           js_canvas_fill_text,           3);
    set_func(ctx, obj, "strokeText",         js_canvas_stroke_text,         3);
    set_func(ctx, obj, "drawImage",          js_canvas_draw_image,          3);
    set_func(ctx, obj, "getImageData",       js_canvas_get_image_data,      4);
    set_func(ctx, obj, "putImageData",       js_canvas_put_image_data,      3);
    set_func(ctx, obj, "save",               js_canvas_save,                0);
    set_func(ctx, obj, "restore",            js_canvas_restore,             0);
    set_func(ctx, obj, "translate",          js_canvas_translate,           2);
    set_func(ctx, obj, "rotate",             js_canvas_rotate,              1);
    set_func(ctx, obj, "scale",              js_canvas_scale,               2);
    set_func(ctx, obj, "setTransform",       js_canvas_set_transform,       6);
    set_func(ctx, obj, "resetTransform",     js_canvas_reset_transform,     0);
    set_func(ctx, obj, "flush",              js_canvas_flush,               0);
    obj
}

unsafe extern "C" fn js_canvas_fill_rect(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    let w = js_val_to_f64(ctx, *argv.offset(2));
    let h = js_val_to_f64(ctx, *argv.offset(3));
    let style = read_str_prop(ctx, this_val, "fillStyle");
    let color = crate::canvas::parse_css_color(&style);
    let transform = { CANVAS_CONTEXTS.lock().get(&win_id).map(|c| c.transform) };
    if let Some(t) = transform {
        if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
            crate::canvas::fill_rect_buf(&mut win.pixels, win.width, win.height, x, y, w, h, color, &t);
        }
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_stroke_rect(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    let w = js_val_to_f64(ctx, *argv.offset(2));
    let h = js_val_to_f64(ctx, *argv.offset(3));
    let style = read_str_prop(ctx, this_val, "strokeStyle");
    let lw    = read_f64_prop(ctx, this_val, "lineWidth");
    let color = crate::canvas::parse_css_color(&style);
    let transform = { CANVAS_CONTEXTS.lock().get(&win_id).map(|c| c.transform) };
    if let Some(t) = transform {
        if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
            crate::canvas::stroke_rect_buf(&mut win.pixels, win.width, win.height, x, y, w, h, color, lw, &t);
        }
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_clear_rect(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    let w = js_val_to_f64(ctx, *argv.offset(2));
    let h = js_val_to_f64(ctx, *argv.offset(3));
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        crate::canvas::clear_rect_buf(&mut win.pixels, win.width, win.height, x, y, w, h);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_begin_path(
    _ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(_ctx, this_val);
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.clear();
        c.current_pos = (0.0, 0.0);
        c.subpath_start = (0.0, 0.0);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_move_to(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::MoveTo(x, y));
        c.current_pos = (x, y);
        c.subpath_start = (x, y);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_line_to(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::LineTo(x, y));
        c.current_pos = (x, y);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_arc(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 5 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let cx    = js_val_to_f64(ctx, *argv.offset(0));
    let cy    = js_val_to_f64(ctx, *argv.offset(1));
    let r     = js_val_to_f64(ctx, *argv.offset(2));
    let start = js_val_to_f64(ctx, *argv.offset(3));
    let end   = js_val_to_f64(ctx, *argv.offset(4));
    let ccw   = argc >= 6 && js_val_to_i32(ctx, *argv.offset(5)) != 0;
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::Arc { cx, cy, r, start, end, ccw });
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_bezier_curve_to(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 6 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let cp1x = js_val_to_f64(ctx, *argv.offset(0));
    let cp1y = js_val_to_f64(ctx, *argv.offset(1));
    let cp2x = js_val_to_f64(ctx, *argv.offset(2));
    let cp2y = js_val_to_f64(ctx, *argv.offset(3));
    let x    = js_val_to_f64(ctx, *argv.offset(4));
    let y    = js_val_to_f64(ctx, *argv.offset(5));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::BezierCurveTo { cp1x, cp1y, cp2x, cp2y, x, y });
        c.current_pos = (x, y);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_quadratic_curve_to(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let cpx = js_val_to_f64(ctx, *argv.offset(0));
    let cpy = js_val_to_f64(ctx, *argv.offset(1));
    let x   = js_val_to_f64(ctx, *argv.offset(2));
    let y   = js_val_to_f64(ctx, *argv.offset(3));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::QuadraticCurveTo { cpx, cpy, x, y });
        c.current_pos = (x, y);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_rect_path(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x = js_val_to_f64(ctx, *argv.offset(0));
    let y = js_val_to_f64(ctx, *argv.offset(1));
    let w = js_val_to_f64(ctx, *argv.offset(2));
    let h = js_val_to_f64(ctx, *argv.offset(3));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::MoveTo(x, y));
        c.path.push(crate::canvas::PathCmd::Rect(x, y, w, h));
        c.current_pos = (x, y);
        c.subpath_start = (x, y);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_close_path(
    _ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(_ctx, this_val);
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.path.push(crate::canvas::PathCmd::ClosePath);
        c.current_pos = c.subpath_start;
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_fill(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    let style = read_str_prop(ctx, this_val, "fillStyle");
    let color = crate::canvas::parse_css_color(&style);
    let (path, transform) = {
        let ctxs = CANVAS_CONTEXTS.lock();
        match ctxs.get(&win_id) {
            Some(c) => (c.path.clone(), c.transform),
            None => return js_undefined(),
        }
    };
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        crate::canvas::fill_path(&mut win.pixels, win.width, win.height, &path, &transform, color);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_stroke(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    let style = read_str_prop(ctx, this_val, "strokeStyle");
    let lw    = read_f64_prop(ctx, this_val, "lineWidth");
    let color = crate::canvas::parse_css_color(&style);
    let (path, transform) = {
        let ctxs = CANVAS_CONTEXTS.lock();
        match ctxs.get(&win_id) {
            Some(c) => (c.path.clone(), c.transform),
            None => return js_undefined(),
        }
    };
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        crate::canvas::stroke_path(&mut win.pixels, win.width, win.height, &path, &transform, color, lw);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_fill_text(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 3 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let text  = js_to_rust_string(ctx, *argv.offset(0));
    let x     = js_val_to_f64(ctx, *argv.offset(1)) as i32;
    let y     = js_val_to_f64(ctx, *argv.offset(2)) as i32;
    let style = read_str_prop(ctx, this_val, "fillStyle");
    let font  = read_str_prop(ctx, this_val, "font");
    let (r, g, b) = crate::canvas::parse_css_color(&style);
    // Parse font size: "16px monospace" → extract number before "px"
    let large = font.split("px").next()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map(|size| size >= 16.0)
        .unwrap_or(false);
    let transform = { CANVAS_CONTEXTS.lock().get(&win_id).map(|c| c.transform) };
    if let Some(t) = transform {
        let (tx, ty) = crate::canvas::transform_point(&t, x as f64, y as f64);
        if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
            crate::framebuffer::draw_string_to_buffer(
                &mut win.pixels, win.width, win.height,
                &text, tx as i32, ty as i32, r, g, b, large,
            );
        }
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_stroke_text(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    // strokeText is the same as fillText for JSOS (no separate outline rendering)
    js_canvas_fill_text(ctx, this_val, argc, argv)
}

/// drawImage(img, dx, dy)
/// drawImage(img, dx, dy, dw, dh)
/// drawImage(img, sx, sy, sw, sh, dx, dy, dw, dh)
/// `img` = { width: number, height: number, data: ArrayBuffer }
unsafe extern "C" fn js_canvas_draw_image(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 3 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let img = *argv.offset(0);

    // Read img.width, img.height, img.data
    let cw = js_cstring("width");  let img_w_val = JS_GetPropertyStr(ctx, img, cw.as_ptr() as *const c_char);
    let ch = js_cstring("height"); let img_h_val = JS_GetPropertyStr(ctx, img, ch.as_ptr() as *const c_char);
    let cd = js_cstring("data");   let data_val  = JS_GetPropertyStr(ctx, img, cd.as_ptr() as *const c_char);
    let img_w = js_val_to_i32(ctx, img_w_val) as usize;
    let img_h = js_val_to_i32(ctx, img_h_val) as usize;
    JS_FreeValue(ctx, img_w_val);
    JS_FreeValue(ctx, img_h_val);

    let mut data_size: usize = 0;
    let data_ptr = JS_GetArrayBuffer(ctx, &mut data_size, data_val);
    JS_FreeValue(ctx, data_val);
    if data_ptr.is_null() || img_w == 0 || img_h == 0 { return js_undefined(); }
    let src = core::slice::from_raw_parts(data_ptr as *const u32, img_w * img_h);

    let (sx, sy, sw, sh, dx, dy, dw, dh) = if argc >= 9 {
        (
            js_val_to_f64(ctx, *argv.offset(1)) as usize,
            js_val_to_f64(ctx, *argv.offset(2)) as usize,
            js_val_to_f64(ctx, *argv.offset(3)) as usize,
            js_val_to_f64(ctx, *argv.offset(4)) as usize,
            js_val_to_f64(ctx, *argv.offset(5)) as i32,
            js_val_to_f64(ctx, *argv.offset(6)) as i32,
            js_val_to_f64(ctx, *argv.offset(7)) as usize,
            js_val_to_f64(ctx, *argv.offset(8)) as usize,
        )
    } else if argc >= 5 {
        (0, 0, img_w, img_h,
            js_val_to_f64(ctx, *argv.offset(1)) as i32,
            js_val_to_f64(ctx, *argv.offset(2)) as i32,
            js_val_to_f64(ctx, *argv.offset(3)) as usize,
            js_val_to_f64(ctx, *argv.offset(4)) as usize,
        )
    } else {
        (0, 0, img_w, img_h,
            js_val_to_f64(ctx, *argv.offset(1)) as i32,
            js_val_to_f64(ctx, *argv.offset(2)) as i32,
            img_w, img_h,
        )
    };

    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        crate::canvas::blit_image(
            &mut win.pixels, win.width, win.height,
            src, img_w, img_h,
            sx, sy, sw, sh,
            dx, dy, dw, dh,
        );
    }
    js_undefined()
}

/// getImageData(x, y, w, h) → { width, height, data: ArrayBuffer (RGBA u8×4) }
unsafe extern "C" fn js_canvas_get_image_data(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 4 { return js_null(); }
    let win_id = read_ctx_id(ctx, this_val);
    let x  = js_val_to_f64(ctx, *argv.offset(0)) as i32;
    let y  = js_val_to_f64(ctx, *argv.offset(1)) as i32;
    let rw = js_val_to_f64(ctx, *argv.offset(2)) as usize;
    let rh = js_val_to_f64(ctx, *argv.offset(3)) as usize;
    if rw == 0 || rh == 0 { return js_null(); }

    let rgba_data: Vec<u8> = {
        let buffers = WINDOW_BUFFERS.lock();
        match buffers.get(&win_id) {
            None => return js_null(),
            Some(win) => {
                let mut out = Vec::with_capacity(rw * rh * 4);
                for row in 0..rh {
                    let src_y_raw = y + row as i32;
                    for col in 0..rw {
                        let src_x_raw = x + col as i32;
                        let px = if src_x_raw >= 0 && src_y_raw >= 0
                            && (src_x_raw as usize) < win.width
                            && (src_y_raw as usize) < win.height {
                            win.pixels[src_y_raw as usize * win.width + src_x_raw as usize]
                        } else { 0 };
                        out.push(((px >> 16) & 0xFF) as u8); // R
                        out.push(((px >>  8) & 0xFF) as u8); // G
                        out.push(( px        & 0xFF) as u8); // B
                        out.push(255u8);                     // A
                    }
                }
                out
            }
        }
    };

    let byte_len = rgba_data.len();
    let raw = rgba_data.as_ptr() as *mut u8;
    core::mem::forget(rgba_data);

    unsafe extern "C" fn free_rgba(_rt: *mut JSRuntime, opaque: *mut c_void, ptr: *mut c_void) {
        let count = opaque as usize;
        drop(Vec::from_raw_parts(ptr as *mut u8, count, count));
    }

    let data_ab = JS_NewArrayBuffer(ctx, raw, byte_len, Some(free_rgba), byte_len as *mut c_void, false);
    let result = JS_NewObject(ctx);
    set_prop_obj(ctx, result, "width",  js_int(rw as i32));
    set_prop_obj(ctx, result, "height", js_int(rh as i32));
    set_prop_obj(ctx, result, "data",   data_ab);
    result
}

/// putImageData(imageData, dx, dy)  — imageData = { width, height, data: ArrayBuffer (RGBA u8×4) }
unsafe extern "C" fn js_canvas_put_image_data(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 3 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let img_data = *argv.offset(0);
    let dx = js_val_to_f64(ctx, *argv.offset(1)) as i32;
    let dy = js_val_to_f64(ctx, *argv.offset(2)) as i32;

    let cw = js_cstring("width");  let w_val = JS_GetPropertyStr(ctx, img_data, cw.as_ptr() as *const c_char);
    let ch = js_cstring("height"); let h_val = JS_GetPropertyStr(ctx, img_data, ch.as_ptr() as *const c_char);
    let cd = js_cstring("data");   let d_val = JS_GetPropertyStr(ctx, img_data, cd.as_ptr() as *const c_char);
    let iw = js_val_to_i32(ctx, w_val) as usize;
    let ih = js_val_to_i32(ctx, h_val) as usize;
    JS_FreeValue(ctx, w_val);
    JS_FreeValue(ctx, h_val);

    let mut data_size: usize = 0;
    let data_ptr = JS_GetArrayBuffer(ctx, &mut data_size, d_val);
    JS_FreeValue(ctx, d_val);
    if data_ptr.is_null() { return js_undefined(); }
    let rgba = core::slice::from_raw_parts(data_ptr as *const u8, data_size);

    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        for row in 0..ih {
            let dst_y = dy + row as i32;
            if dst_y < 0 || dst_y >= win.height as i32 { continue; }
            for col in 0..iw {
                let dst_x = dx + col as i32;
                if dst_x < 0 || dst_x >= win.width as i32 { continue; }
                let i = (row * iw + col) * 4;
                if i + 3 >= rgba.len() { break; }
                let r = rgba[i] as u32;
                let g = rgba[i+1] as u32;
                let b = rgba[i+2] as u32;
                win.pixels[dst_y as usize * win.width + dst_x as usize] =
                    (r << 16) | (g << 8) | b;
            }
        }
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_save(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    let fill_style   = read_str_prop(ctx, this_val, "fillStyle");
    let stroke_style = read_str_prop(ctx, this_val, "strokeStyle");
    let line_width   = read_f64_prop(ctx, this_val, "lineWidth");
    let font         = read_str_prop(ctx, this_val, "font");
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.state_stack.push(crate::canvas::CanvasState {
            fill_style, stroke_style, line_width, font, transform: c.transform,
        });
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_restore(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    let state = {
        let mut ctxs = CANVAS_CONTEXTS.lock();
        ctxs.get_mut(&win_id).and_then(|c| {
            let s = c.state_stack.pop();
            if let Some(ref st) = s { c.transform = st.transform; }
            s
        })
    };
    if let Some(s) = state {
        set_prop_obj(ctx, this_val, "fillStyle",   js_str(ctx, &s.fill_style));
        set_prop_obj(ctx, this_val, "strokeStyle", js_str(ctx, &s.stroke_style));
        set_prop_obj(ctx, this_val, "lineWidth",   js_float(s.line_width));
        set_prop_obj(ctx, this_val, "font",        js_str(ctx, &s.font));
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_translate(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let tx = js_val_to_f64(ctx, *argv.offset(0));
    let ty = js_val_to_f64(ctx, *argv.offset(1));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        let t = [1.0, 0.0, 0.0, 1.0, tx, ty];
        c.transform = crate::canvas::multiply_transform(&t, &c.transform);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_rotate(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 1 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let angle = js_val_to_f64(ctx, *argv.offset(0));
    let cos_a = libm::cos(angle);
    let sin_a = libm::sin(angle);
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        let rot = [cos_a, sin_a, -sin_a, cos_a, 0.0, 0.0];
        c.transform = crate::canvas::multiply_transform(&rot, &c.transform);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_scale(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let sx = js_val_to_f64(ctx, *argv.offset(0));
    let sy = js_val_to_f64(ctx, *argv.offset(1));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        let s = [sx, 0.0, 0.0, sy, 0.0, 0.0];
        c.transform = crate::canvas::multiply_transform(&s, &c.transform);
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_set_transform(
    ctx: *mut JSContext, this_val: JSValue, argc: c_int, argv: *const JSValue,
) -> JSValue {
    if argc < 6 { return js_undefined(); }
    let win_id = read_ctx_id(ctx, this_val);
    let a = js_val_to_f64(ctx, *argv.offset(0));
    let b = js_val_to_f64(ctx, *argv.offset(1));
    let c_ = js_val_to_f64(ctx, *argv.offset(2));
    let d = js_val_to_f64(ctx, *argv.offset(3));
    let e = js_val_to_f64(ctx, *argv.offset(4));
    let f = js_val_to_f64(ctx, *argv.offset(5));
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.transform = [a, b, c_, d, e, f];
    }
    js_undefined()
}

unsafe extern "C" fn js_canvas_reset_transform(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    if let Some(c) = CANVAS_CONTEXTS.lock().get_mut(&win_id) {
        c.transform = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    }
    js_undefined()
}

/// ctx.flush() — blit the window pixel buffer to the framebuffer.
unsafe extern "C" fn js_canvas_flush(
    ctx: *mut JSContext, this_val: JSValue, _argc: c_int, _argv: *const JSValue,
) -> JSValue {
    let win_id = read_ctx_id(ctx, this_val);
    let blit_args = {
        let buffers = WINDOW_BUFFERS.lock();
        buffers.get(&win_id)
            .filter(|win| win.pixel_buffer_active)
            .map(|win| (win.x, win.y, win.width, win.height, win.pixels.clone()))
    };
    if let Some((x, y, w, h, pixels)) = blit_args {
        crate::framebuffer::blit_window_pixels(x, y, w, h, &pixels);
    }
    js_undefined()
}

/// Subtract occluder `sub` from every rect in `list`, returning only the
/// visible fragments.  Each input rect may produce up to 4 output fragments.
fn subtract_rect(
    list: &[(usize, usize, usize, usize)],
    sub: (usize, usize, usize, usize),
) -> alloc::vec::Vec<(usize, usize, usize, usize)> {
    let (sx, sy, sw, sh) = sub;
    let mut out = alloc::vec::Vec::new();
    for &(rx, ry, rw, rh) in list {
        // No overlap — keep intact.
        if rx + rw <= sx || sx + sw <= rx || ry + rh <= sy || sy + sh <= ry {
            out.push((rx, ry, rw, rh));
            continue;
        }
        // Top strip (above sub)
        if ry < sy {
            out.push((rx, ry, rw, sy - ry));
        }
        // Bottom strip (below sub)
        let r_bot = ry + rh;
        let s_bot = sy + sh;
        if r_bot > s_bot {
            out.push((rx, s_bot, rw, r_bot - s_bot));
        }
        // Middle band — left and right strips
        let band_top = ry.max(sy);
        let band_bot = r_bot.min(s_bot);
        if band_bot > band_top {
            if rx < sx {
                out.push((rx, band_top, sx - rx, band_bot - band_top));
            }
            let r_right = rx + rw;
            let s_right = sx + sw;
            if r_right > s_right {
                out.push((s_right, band_top, r_right - s_right, band_bot - band_top));
            }
        }
    }
    out
}

/// Draw one window's decoration at its absolute screen position.
/// The caller is responsible for setting (and clearing) the scissor rect.
fn draw_decoration(
    owner_pid: u32,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    is_focused: bool,
    name_map: &alloc::collections::BTreeMap<u32, alloc::string::String>,
) {
    const TITLE_H: usize = 22;
    let ty = y.saturating_sub(TITLE_H);

    // Shadow strip above title bar
    if ty >= 2 {
        crate::framebuffer::fill_rect(x, ty.saturating_sub(2), width, 2, 4, 5, 12);
    }

    // Title bar background
    let (br, bg, bb) = if is_focused { (18, 28, 60) } else { (14, 18, 36) };
    crate::framebuffer::fill_rect(x, ty, width, TITLE_H, br, bg, bb);

    // Top accent line
    if is_focused {
        crate::framebuffer::fill_rect(x, ty, width, 2, 50, 90, 200);
    } else {
        crate::framebuffer::fill_rect(x, ty, width, 1, 30, 45, 100);
    }

    // Bottom divider
    crate::framebuffer::fill_rect(x, ty + TITLE_H - 1, width, 1, 4, 5, 12);

    // macOS-style control dots
    let dot_cy = ty + 9;
    crate::framebuffer::fill_circle(x as isize + 12, dot_cy as isize, 6, 180, 55, 55);
    crate::framebuffer::fill_circle(x as isize + 12, dot_cy as isize, 4, 200, 60, 60);

    // Title text
    let default_name = alloc::format!("pid {}", owner_pid);
    let name = name_map.get(&owner_pid).unwrap_or(&default_name);
    let max_chars = (width.saturating_sub(70)) / 8;
    let title: alloc::string::String = name.chars().take(max_chars).collect();
    let (tr, tg, tb) = if is_focused { (180, 200, 255) } else { (100, 120, 165) };
    crate::framebuffer::set_foreground_color(tr, tg, tb);
    crate::framebuffer::draw_string(&title, x + 56, ty + 15);

    // Side + bottom drop shadows
    crate::framebuffer::fill_rect(x.saturating_sub(2), ty, 2, TITLE_H + height, 4, 5, 12);
    crate::framebuffer::fill_rect(x + width, ty, 2, TITLE_H + height, 4, 5, 12);
    crate::framebuffer::fill_rect(x.saturating_sub(2), ty + TITLE_H + height, width + 4, 3, 4, 5, 12);
}

/// Draw title bar decorations for every non-winman window, in z-index order,
/// with correct occlusion: each title bar is scissor-clipped to only the
/// pixels not covered by higher-z windows.
///
/// Called from the main loop AFTER poll_processes() so all app frames are
/// committed before decorations are painted on top.
pub fn draw_all_decorations() {
    let focus_pid = crate::process::ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst);

    // Build (owner_pid, x, y, width, height, z) sorted back→front.
    let mut windows: alloc::vec::Vec<(u32, usize, usize, usize, usize, u32)> = {
        let buffers = WINDOW_BUFFERS.lock();
        buffers.values()
            .filter(|w| w.owner_pid != 1)
            .map(|w| (w.owner_pid, w.x, w.y, w.width, w.height, w.z_index))
            .collect()
    };
    windows.sort_by_key(|&(_, _, _, _, _, z)| z);

    // Build pid→name map once.
    let name_map: alloc::collections::BTreeMap<u32, alloc::string::String> = {
        let list = crate::process::PROCESS_LIST.lock();
        list.iter().map(|(&pid, p)| {
            (pid, p.name.trim_end_matches(".jsos").to_string())
        }).collect()
    };

    const TITLE_H: usize = 22;

    for (idx, &(owner_pid, x, y, width, height, _z)) in windows.iter().enumerate() {
        let ty = y.saturating_sub(TITLE_H);
        let is_focused = owner_pid == focus_pid;

        // Start with the full title bar as the visible region.
        let mut visible: alloc::vec::Vec<(usize, usize, usize, usize)> =
            alloc::vec![(x, ty, width, TITLE_H)];

        // Subtract every higher-z window's full bounding box (body + its title bar).
        for &(_, vx, vy, vw, vh, _) in &windows[idx + 1..] {
            let v_top = vy.saturating_sub(TITLE_H);
            visible = subtract_rect(&visible, (vx, v_top, vw, vh + TITLE_H));
            if visible.is_empty() { break; }
        }

        if visible.is_empty() { continue; } // fully occluded — skip entirely

        // Draw the decoration once per visible sub-rect with the scissor active.
        for (vx, vy, vw, vh) in visible {
            crate::framebuffer::set_clip(vx, vy, vw, vh);
            draw_decoration(owner_pid, x, y, width, height, is_focused, &name_map);
        }
    }

    // Always clear the scissor after we're done.
    crate::framebuffer::unset_clip();
}

unsafe extern "C" fn js_os_window_move(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 3 { return js_undefined(); }
    let win_id = js_val_to_i32(ctx, *argv.offset(0)) as u32;
    let new_x = js_val_to_i32(ctx, *argv.offset(1)).max(0) as usize;
    let new_y = js_val_to_i32(ctx, *argv.offset(2)).max(0) as usize;
    if let Some(win) = WINDOW_BUFFERS.lock().get_mut(&win_id) {
        win.x = new_x;
        win.y = new_y;
    }
    js_undefined()
}

unsafe extern "C" fn js_os_window_set_cursor(ctx: *mut JSContext, _this: JSValue, argc: c_int, argv: *const JSValue) -> JSValue {
    if argc < 2 { return js_undefined(); }
    let x = js_val_to_i32(ctx, *argv.offset(0)).max(0) as usize;
    let y = js_val_to_i32(ctx, *argv.offset(1)).max(0) as usize;
    CURSOR_X.store(x, Ordering::Relaxed);
    CURSOR_Y.store(y, Ordering::Relaxed);
    js_undefined()
}

unsafe extern "C" fn js_os_window_list(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let buffers = WINDOW_BUFFERS.lock();
    let mut entries = alloc::vec::Vec::new();
    for (id, win) in buffers.iter() {
        entries.push(format!(
            "{{\"id\":{}, \"x\":{}, \"y\":{}, \"width\":{}, \"height\":{}, \"owner_pid\":{}}}", 
            id, win.x, win.y, win.width, win.height, win.owner_pid
        ));
    }
    let json = format!("[{}]", entries.join(","));
    let s = js_cstring(&json);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_sysinfo(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let info = crate::sysinfo::get_sysinfo();
    let s = js_cstring(&info);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_rtc(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let rtc = crate::sysinfo::get_rtc();
    let s = js_cstring(&rtc);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_screen(ctx: *mut JSContext, _this: JSValue, _argc: c_int, _argv: *const JSValue) -> JSValue {
    let (w, h) = crate::framebuffer::get_resolution();
    let json = format!("{{\"width\":{},\"height\":{}}}", w, h);
    let s = js_cstring(&json);
    JS_NewStringLen(ctx, s.as_ptr() as *const c_char, s.len() - 1)
}

unsafe extern "C" fn js_os_random_bytes(
    ctx: *mut JSContext,
    _this: JSValue,
    argc: c_int,
    argv: *const JSValue,
) -> JSValue {
    use core::arch::x86_64::_rdrand64_step;

    if argc < 1 {
        return js_undefined();
    }
    let n = (js_val_to_i32(ctx, *argv) as usize).min(65536);
    let mut buf = alloc::vec![0u8; n];
    let mut i = 0;
    while i < n {
        let mut val: u64 = 0;
        if _rdrand64_step(&mut val) == 1 {
            let bytes = val.to_le_bytes();
            let take = (n - i).min(8);
            buf[i..i + take].copy_from_slice(&bytes[..take]);
            i += take;
        }
    }
    JS_NewArrayBufferCopy(ctx, buf.as_ptr(), n)
}

// ======== Rust-side exports for C stubs ========

/// Called from C freestanding.c to print to serial
#[no_mangle]
pub extern "C" fn rust_serial_print(s: *const c_char, len: usize) {
    unsafe {
        if !s.is_null() {
            let bytes = core::slice::from_raw_parts(s as *const u8, len);
            if let Ok(text) = core::str::from_utf8(bytes) {
                crate::serial_print!("{}", text);
            }
        }
    }
}

/// Called from C freestanding.c for malloc
#[no_mangle]
pub extern "C" fn rust_alloc(size: usize, align: usize) -> *mut c_void {
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(size, align);
        alloc::alloc::alloc(layout) as *mut c_void
    }
}

/// Called from C freestanding.c for free
#[no_mangle]
pub extern "C" fn rust_dealloc(ptr: *mut c_void, size: usize, align: usize) {
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(size, align);
        alloc::alloc::dealloc(ptr as *mut u8, layout);
    }
}

/// Called from C freestanding.c for realloc
#[no_mangle]
pub extern "C" fn rust_realloc(ptr: *mut c_void, old_size: usize, new_size: usize, align: usize) -> *mut c_void {
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(old_size, align);
        alloc::alloc::realloc(ptr as *mut u8, layout, new_size) as *mut c_void
    }
}

/// Called from C freestanding.c for system tick count
#[no_mangle]
pub extern "C" fn rust_get_ticks() -> u64 {
    crate::interrupts::TICKS.load(Ordering::Relaxed)
}

// ---- Math forwarding from C to Rust libm ----

#[no_mangle] pub extern "C" fn rust_floor(x: f64) -> f64 { libm::floor(x) }
#[no_mangle] pub extern "C" fn rust_ceil(x: f64) -> f64 { libm::ceil(x) }
#[no_mangle] pub extern "C" fn rust_sqrt(x: f64) -> f64 { libm::sqrt(x) }
#[no_mangle] pub extern "C" fn rust_fabs(x: f64) -> f64 { libm::fabs(x) }
#[no_mangle] pub extern "C" fn rust_fmod(x: f64, y: f64) -> f64 { libm::fmod(x, y) }
#[no_mangle] pub extern "C" fn rust_pow(x: f64, y: f64) -> f64 { libm::pow(x, y) }
#[no_mangle] pub extern "C" fn rust_log(x: f64) -> f64 { libm::log(x) }
#[no_mangle] pub extern "C" fn rust_log2(x: f64) -> f64 { libm::log2(x) }
#[no_mangle] pub extern "C" fn rust_log10(x: f64) -> f64 { libm::log10(x) }
#[no_mangle] pub extern "C" fn rust_exp(x: f64) -> f64 { libm::exp(x) }
#[no_mangle] pub extern "C" fn rust_expm1(x: f64) -> f64 { libm::expm1(x) }
#[no_mangle] pub extern "C" fn rust_log1p(x: f64) -> f64 { libm::log1p(x) }
#[no_mangle] pub extern "C" fn rust_sin(x: f64) -> f64 { libm::sin(x) }
#[no_mangle] pub extern "C" fn rust_cos(x: f64) -> f64 { libm::cos(x) }
#[no_mangle] pub extern "C" fn rust_tan(x: f64) -> f64 { libm::tan(x) }
#[no_mangle] pub extern "C" fn rust_asin(x: f64) -> f64 { libm::asin(x) }
#[no_mangle] pub extern "C" fn rust_acos(x: f64) -> f64 { libm::acos(x) }
#[no_mangle] pub extern "C" fn rust_atan(x: f64) -> f64 { libm::atan(x) }
#[no_mangle] pub extern "C" fn rust_atan2(y: f64, x: f64) -> f64 { libm::atan2(y, x) }
#[no_mangle] pub extern "C" fn rust_sinh(x: f64) -> f64 { libm::sinh(x) }
#[no_mangle] pub extern "C" fn rust_cosh(x: f64) -> f64 { libm::cosh(x) }
#[no_mangle] pub extern "C" fn rust_tanh(x: f64) -> f64 { libm::tanh(x) }
#[no_mangle] pub extern "C" fn rust_asinh(x: f64) -> f64 { libm::asinh(x) }
#[no_mangle] pub extern "C" fn rust_acosh(x: f64) -> f64 { libm::acosh(x) }
#[no_mangle] pub extern "C" fn rust_atanh(x: f64) -> f64 { libm::atanh(x) }
#[no_mangle] pub extern "C" fn rust_round(x: f64) -> f64 { libm::round(x) }
#[no_mangle] pub extern "C" fn rust_trunc(x: f64) -> f64 { libm::trunc(x) }
#[no_mangle] pub extern "C" fn rust_floorf(x: f32) -> f32 { libm::floorf(x) }
#[no_mangle] pub extern "C" fn rust_ceilf(x: f32) -> f32 { libm::ceilf(x) }
#[no_mangle] pub extern "C" fn rust_sqrtf(x: f32) -> f32 { libm::sqrtf(x) }
#[no_mangle] pub extern "C" fn rust_fabsf(x: f32) -> f32 { libm::fabsf(x) }
#[no_mangle] pub extern "C" fn rust_modf(x: f64, iptr: *mut f64) -> f64 {
    let (f, i) = libm::modf(x);
    unsafe { if !iptr.is_null() { *iptr = i; } }
    f
}