use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use spin::Mutex;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, AtomicI32, AtomicU8, Ordering};
use crate::js_runtime::QuickJsSandbox;
use lazy_static::lazy_static;

pub const MAX_EVENTS: usize = 128;

pub struct EventQueue<T: Copy> {
    buffer: [T; MAX_EVENTS],
    head: usize,
    tail: usize,
}

impl<T: Copy> EventQueue<T> {
    pub const fn new(default: T) -> Self {
        Self { buffer: [default; MAX_EVENTS], head: 0, tail: 0 }
    }
    pub fn push(&mut self, item: T) {
        let next = (self.head + 1) % MAX_EVENTS;
        if next != self.tail {
            self.buffer[self.head] = item;
            self.head = next;
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        if self.head == self.tail {
            None
        } else {
            let item = self.buffer[self.tail];
            self.tail = (self.tail + 1) % MAX_EVENTS;
            Some(item)
        }
    }
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }
}

lazy_static! {
    pub static ref KEY_EVENTS: Mutex<EventQueue<char>> = Mutex::new(EventQueue::new('\0'));
}

pub struct Process {
    pub pid: u32,
    pub name: String,
    pub sandbox: Arc<Mutex<QuickJsSandbox>>,
    pub ipc_queue: Arc<Mutex<alloc::vec::Vec<String>>>,
    pub dead: bool,
}

pub static PROCESS_LIST: Mutex<BTreeMap<u32, Process>> = Mutex::new(BTreeMap::new());
pub static NEXT_PID: AtomicU32 = AtomicU32::new(1);
pub static ACTIVE_FOREGROUND_PID: AtomicU32 = AtomicU32::new(1);

// ── Mouse delta tracking — replaces the former `static mut` UB ───────────
// 255 / -1 are sentinel "uninitialised" values so the first real mouse event
// always fires, regardless of where the cursor starts.
static LAST_MOUSE_X: AtomicI32 = AtomicI32::new(-1);
static LAST_MOUSE_Y: AtomicI32 = AtomicI32::new(-1);
static LAST_MOUSE_BTN: AtomicU8 = AtomicU8::new(255);

// ── IPC helpers ───────────────────────────────────────────────────────────

/// Encode a string as a JSON string literal (including surrounding quotes).
/// This is used to safely pass IPC messages into JS without template-literal
/// injection — previously `${...}` inside a message could execute arbitrary JS.
fn json_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Escape ASCII control characters
                let hex = alloc::format!("\\u{:04X}", c as u32);
                out.push_str(&hex);
            }
            c    => out.push(c),
        }
    }
    out.push('"');
    out
}

// ── ES module import transform ────────────────────────────────────────────

/// Rewrite `import { A, B as b } from 'mod'` binding list to destructuring
/// syntax: `A, B: b`.
fn rewrite_bindings(inner: &str) -> String {
    inner.split(',')
        .map(|b| b.trim())
        .filter(|b| !b.is_empty())
        .map(|b| {
            if let Some(pos) = b.find(" as ") {
                alloc::format!("{}: {}", b[..pos].trim(), b[pos + 4..].trim())
            } else {
                b.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Try to rewrite a single `import …` statement (with the leading `import`
/// keyword already stripped) into a CommonJS-compatible expression.
fn rewrite_import_line(after_kw: &str) -> Option<String> {
    // Side-effect: import 'X'  /  import "X"
    let first = after_kw.chars().next()?;
    if first == '\'' || first == '"' {
        let module = after_kw.trim_matches(|c: char| c == '\'' || c == '"');
        return Some(alloc::format!("require('{}');", module));
    }

    // All other forms require `from`
    let from_pos = after_kw.rfind(" from ")?;
    let bindings = after_kw[..from_pos].trim();
    let module = after_kw[from_pos + 6..]
        .trim()
        .trim_matches(|c: char| c == '\'' || c == '"');

    // Namespace: * as X
    if let Some(ns) = bindings.strip_prefix("* as ") {
        return Some(alloc::format!("const {} = require('{}');", ns.trim(), module));
    }

    // Named only: { A, B as b }
    if bindings.starts_with('{') && bindings.ends_with('}') {
        let inner = &bindings[1..bindings.len() - 1];
        return Some(alloc::format!(
            "const {{{}}} = require('{}');",
            rewrite_bindings(inner),
            module
        ));
    }

    // Default + named: X, { A, B }
    if let Some(brace) = bindings.find('{') {
        let default_name = bindings[..bindings.find(',')?].trim();
        let named_inner = &bindings[brace + 1..bindings.rfind('}').unwrap_or(bindings.len())];
        return Some(alloc::format!(
            "const __m = require('{}'); const {} = __m.default !== undefined ? __m.default : __m; const {{{}}} = __m;",
            module,
            default_name,
            rewrite_bindings(named_inner)
        ));
    }

    // Default only: import X from 'mod'
    let name = bindings.trim();
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return Some(alloc::format!("const {} = require('{}');", name, module));
    }

    None
}

/// Transform ES-module `import` statements at the top level of `source` into
/// CommonJS `require()` calls so that `.jsos` apps can use `import` syntax
/// while the kernel continues to run them in `JS_EVAL_TYPE_GLOBAL` mode.
///
/// Only lines whose first non-whitespace token is the keyword `import`
/// (followed by a space, `'`, or `"`) are touched; all other lines are copied
/// verbatim.
fn transform_imports(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 64);
    for line in source.lines() {
        let trimmed = line.trim_start();
        let is_import = trimmed.starts_with("import ")
            || trimmed.starts_with("import'")
            || trimmed.starts_with("import\"");
        if is_import {
            // Strip trailing semicolon/whitespace before parsing
            let stripped = trimmed
                .strip_prefix("import")
                .unwrap()
                .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
                .trim_start();
            if let Some(replacement) = rewrite_import_line(stripped) {
                let indent_len = line.len() - trimmed.len();
                out.push_str(&line[..indent_len]);
                out.push_str(&replacement);
                out.push('\n');
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ── Process lifecycle ─────────────────────────────────────────────────────

/// Spawns a new JavaScript process with the given name and source code.
pub fn spawn_process(name: &str, js_source: &str) -> u32 {
    let pid = NEXT_PID.fetch_add(1, Ordering::SeqCst);

    let mut sandbox = match QuickJsSandbox::new() {
        Ok(s) => s,
        Err(_) => panic!("Failed to initialize JS sandbox"),
    };

    let pid_script = alloc::format!(
        "globalThis.__PID = {}; globalThis.__PROCESS_NAME = '{}';",
        pid, name
    );
    let _ = sandbox.eval(&pid_script);

    let process = Process {
        pid,
        name: name.to_string(),
        sandbox: Arc::new(Mutex::new(sandbox)),
        ipc_queue: Arc::new(Mutex::new(alloc::vec::Vec::new())),
        dead: false,
    };

    PROCESS_LIST.lock().insert(pid, process);

    // Evaluate the app source now that the process is in the list (so
    // crash_process can find it if the top-level script throws).
    let result = {
        let list = PROCESS_LIST.lock();
        list.get(&pid).map(|p| p.sandbox.clone())
    };
    if let Some(sandbox_arc) = result {
        let mut sandbox = sandbox_arc.lock();
        let transformed = transform_imports(js_source);
        if let Err(e) = sandbox.eval(&transformed) {
            drop(sandbox);
            crash_process(pid, name, &e);
            return pid;
        }
        sandbox.enable_preemption();
    }

    // If the script ran to completion without registering any windows or timers,
    // it has no event loop — auto-exit it to avoid ghost processes.
    let has_windows = crate::js_runtime::process_has_windows(pid);
    let has_timers  = crate::js_runtime::process_has_timers(pid);
    if !has_windows && !has_timers {
        kill_process_and_cleanup(pid);
    }

    pid
}

/// Mark a process dead and immediately release its external resources
/// (windows, timers). The sandbox itself is dropped by `reap_dead_processes`
/// from the main loop, outside of any interrupt context.
pub fn kill_process_and_cleanup(pid: u32) {
    // Just mark as dead — actual cleanup happens in reap_dead_processes()
    // to avoid deadlocks when called from inside sandbox eval.
    if let Some(process) = PROCESS_LIST.lock().get_mut(&pid) {
        process.dead = true;
    }
}

/// Alias for `kill_process_and_cleanup`.
pub fn kill_process(pid: u32) {
    kill_process_and_cleanup(pid);
}

/// Kill a process due to an uncaught JS exception and show a kernel-level toast.
pub fn crash_process(pid: u32, name: &str, error: &str) {
    crate::serial_println!("[crash] {} (pid={}) — {}", name, pid, error);
    // Push a kernel-level overlay notification (drawn on top of all windows).
    crate::js_runtime::push_notification(name, error);
    kill_process_and_cleanup(pid);
}

/// Drop all processes marked dead. Called from the main loop.
pub fn reap_dead_processes() {
    let mut list = PROCESS_LIST.lock();

    let dead_pids: Vec<u32> = list.iter()
        .filter(|(_, p)| p.dead)
        .map(|(pid, _)| *pid)
        .collect();

    if dead_pids.is_empty() {
        return;
    }

    // Clean up resources for dead processes (windows, timers, canvas, network).
    // Must happen while PROCESS_LIST is held to prevent races.
    drop(list); // release PROCESS_LIST before taking other locks
    for pid in &dead_pids {
        crate::js_runtime::cleanup_process_resources(*pid);
        crate::net::cleanup_process_network(*pid);
    }
    let mut list = PROCESS_LIST.lock();

    list.retain(|_pid, process| !process.dead);

    // If the foreground process was just reaped, fall back to PID 1 (winman).
    let focus = ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst);
    if !list.contains_key(&focus) {
        ACTIVE_FOREGROUND_PID.store(1, Ordering::SeqCst);
    }
}

// ── Main-loop polling ─────────────────────────────────────────────────────

/// Polls all active processes: drains input queues and runs pending JS jobs.
pub fn poll_processes() {
    // Drain keyboard events collected by the interrupt handler.
    let mut keys = Vec::new();
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = KEY_EVENTS.lock();
        while let Some(k) = q.pop() {
            keys.push(k);
        }
    });

    // Read latest mouse state and only dispatch if it changed.
    let mx     = crate::mouse::MOUSE_X.load(Ordering::Relaxed);
    let my     = crate::mouse::MOUSE_Y.load(Ordering::Relaxed);
    let mflags = crate::mouse::MOUSE_BTN.load(Ordering::Relaxed);

    let mouse_changed =
        LAST_MOUSE_X.load(Ordering::Relaxed)   != mx ||
        LAST_MOUSE_Y.load(Ordering::Relaxed)   != my ||
        LAST_MOUSE_BTN.load(Ordering::Relaxed) != mflags;

    let mut mice = Vec::new();
    if mouse_changed {
        LAST_MOUSE_X.store(mx, Ordering::Relaxed);
        LAST_MOUSE_Y.store(my, Ordering::Relaxed);
        LAST_MOUSE_BTN.store(mflags, Ordering::Relaxed);
        mice.push((mx, my, mflags));
    }

    // Route keyboard events to the foreground process.
    let active_pid = ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst);
    let active_info = {
        let list = PROCESS_LIST.lock();
        list.get(&active_pid).map(|p| (p.sandbox.clone(), p.name.clone()))
    };
    if let Some((arc, name)) = active_info {
        let mut sandbox = arc.lock();
        if active_pid != 1 { sandbox.start_timeslice(); }
        for k in keys {
            let script = alloc::format!(
                "if (typeof globalThis.on_key === 'function') {{ globalThis.on_key({}); }}",
                k as u32
            );
            if let Err(e) = sandbox.eval(&script) {
                if e.contains("interrupted") {
                    crate::serial_println!("[sched] preempted {} (pid={}) on key handler", name, active_pid);
                    break;
                }
                drop(sandbox);
                crash_process(active_pid, &name, &e);
                break;
            }
        }
    }

    // Mouse events always go to winman (PID 1) for global focus/drag management.
    let winman_sandbox = {
        let list = PROCESS_LIST.lock();
        list.get(&1).map(|p| p.sandbox.clone())
    };
    if let Some(arc) = winman_sandbox {
        let mut sandbox = arc.lock();
        for (x, y, flags) in mice {
            let script = alloc::format!(
                "if (typeof globalThis.on_mouse === 'function') {{ globalThis.on_mouse({},{},{}); }}",
                x, y, flags
            );
            if let Err(e) = sandbox.eval(&script) {
                if !e.contains(crate::js_runtime::JS_INTERRUPT_MSG) {
                    crate::serial_println!("[winman] mouse handler error: {}", e);
                }
                break;
            }
        }
    }

    // Collect (pid, name, sandbox, ipc_queue), running processes in z-index
    // order (back-to-front) so higher windows always draw on top. The focused
    // process runs last so its content and title bar are never overwritten.
    let mut sandboxes: Vec<(u32, String, Arc<Mutex<QuickJsSandbox>>, Arc<Mutex<Vec<String>>>)> = Vec::new();
    let mut fg_entry = None;

    {
        let list = PROCESS_LIST.lock();
        let focus = ACTIVE_FOREGROUND_PID.load(Ordering::SeqCst);
        for (pid, p) in list.iter() {
            let entry = (*pid, p.name.clone(), p.sandbox.clone(), p.ipc_queue.clone());
            if *pid == focus {
                fg_entry = Some(entry);
            } else {
                sandboxes.push(entry);
            }
        }
    }

    // Sort background processes by the highest z_index among their windows,
    // ascending (back-to-front), so the topmost background window draws last.
    {
        let buffers = crate::js_runtime::WINDOW_BUFFERS.lock();
        sandboxes.sort_by_key(|(pid, _, _, _)| {
            buffers.values()
                .filter(|w| w.owner_pid == *pid)
                .map(|w| w.z_index)
                .max()
                .unwrap_or(0)
        });
    }

    if let Some(fg) = fg_entry {
        sandboxes.push(fg);
    }

    for (pid, name, sandbox_arc, ipc_queue_arc) in sandboxes {
        let messages: Vec<String> = {
            let mut q = ipc_queue_arc.lock();
            core::mem::take(&mut *q)
        };

        let crash_err: Option<String> = {
            let mut sandbox = sandbox_arc.lock();
            // pid=1 is winman — system process, must never be preempted mid-render
            if pid != 1 { sandbox.start_timeslice(); }
            let mut err = None;

            for msg in messages {
                // JSON-quote the message so that special characters (including
                // `${...}`) in the payload cannot inject code.
                let script = alloc::format!(
                    "if (typeof globalThis.on_ipc === 'function') {{ globalThis.on_ipc({}); }}",
                    json_quote(&msg)
                );
                sandbox.start_timeslice();
                match sandbox.eval(&script) {
                    Ok(_) => {}
                    Err(ref e) if e.contains(crate::js_runtime::JS_INTERRUPT_MSG) => {
                        // preempted mid-IPC dispatch — not a crash
                        crate::serial_println!("[sched] preempted ipc for pid={}", pid);
                        break;
                    }
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }

            if err.is_none() {
                // Give pending jobs a generous budget for async work.
                sandbox.start_timeslice_with_budget(5000);
                if let Err(e) = sandbox.execute_pending_jobs() {
                    err = Some(e);
                } else {
                    sandbox.run_gc();
                }
            }

            err
            // sandbox drops here, before crash_process is called
        };

        if let Some(e) = crash_err {
            if e.contains("interrupted") {
                crate::serial_println!("[sched] preempted {} (pid={})", name, pid);
            } else {
                crash_process(pid, &name, &e);
            }
        }
    }
}

/// Enqueue a keyboard character for the foreground process.
pub fn send_key(c: char) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        KEY_EVENTS.lock().push(c);
    });
}
