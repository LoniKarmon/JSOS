use core::sync::atomic::{AtomicUsize, AtomicU8, AtomicBool, Ordering};
use pc_keyboard::{DecodedKey, HandleControl, Keyboard, ScancodeSet1, layouts, KeyCode, KeyState};
use spin::Mutex;
use lazy_static::lazy_static;

const QUEUE_SIZE: usize = 256;

static KBD_QUEUE: [AtomicU8; QUEUE_SIZE] = {
    const INIT: AtomicU8 = AtomicU8::new(0);
    [INIT; QUEUE_SIZE]
};

static KBD_HEAD: AtomicUsize = AtomicUsize::new(0);
static KBD_TAIL: AtomicUsize = AtomicUsize::new(0);

// --- Layout Tracking State ---
pub static ALT_PRESSED: AtomicBool = AtomicBool::new(false);
pub static SHIFT_PRESSED: AtomicBool = AtomicBool::new(false);
pub static CTRL_PRESSED: AtomicBool = AtomicBool::new(false);
pub static HEBREW_LAYOUT: AtomicBool = AtomicBool::new(false);

lazy_static! {
    static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
        Mutex::new(Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore
        ));
}

/// Called ONLY by the hardware interrupt handler
pub fn push_scancode(scancode: u8) -> Result<(), ()> {
    let tail = KBD_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % QUEUE_SIZE;
    
    if next_tail == KBD_HEAD.load(Ordering::Acquire) {
        return Err(());
    }
    
    KBD_QUEUE[tail].store(scancode, Ordering::Release);
    KBD_TAIL.store(next_tail, Ordering::Release);
    Ok(())
}

fn pop_scancode() -> Option<u8> {
    let head = KBD_HEAD.load(Ordering::Relaxed);
    
    if head == KBD_TAIL.load(Ordering::Acquire) {
        return None;
    }
    
    let scancode = KBD_QUEUE[head].load(Ordering::Acquire);
    KBD_HEAD.store((head + 1) % QUEUE_SIZE, Ordering::Release);
    
    Some(scancode)
}

/// Called safely in your main kernel loop
pub fn process_keyboard_queue() {
    let mut keyboard_locked = None;

    while let Some(scancode) = pop_scancode() {
        // Only lock the keyboard state machine if we actually have bytes
        if keyboard_locked.is_none() {
            keyboard_locked = Some(KEYBOARD.lock());
        }

        if let Some(keyboard) = keyboard_locked.as_mut() {
            if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
                // Track layout combinations
                match key_event.code {
                    KeyCode::LAlt => ALT_PRESSED.store(key_event.state == KeyState::Down, Ordering::Relaxed),
                    KeyCode::LControl | KeyCode::RControl => CTRL_PRESSED.store(key_event.state == KeyState::Down, Ordering::Relaxed),
                    KeyCode::LShift => {
                        let down = key_event.state == KeyState::Down;
                        SHIFT_PRESSED.store(down, Ordering::Relaxed);
                        
                        // Toggle layout on Shift+Alt
                        if down && ALT_PRESSED.load(Ordering::Relaxed) {
                            let layout = HEBREW_LAYOUT.load(Ordering::Relaxed);
                            HEBREW_LAYOUT.store(!layout, Ordering::Relaxed);
                        }
                    },
                    _ => {}
                }

                if let Some(key) = keyboard.process_keyevent(key_event) {
                    match key {
                        DecodedKey::Unicode(mut character) => {
                            // Apply Hebrew Layout
                            if HEBREW_LAYOUT.load(Ordering::Relaxed) {
                                character = match character {
                                    'q' | 'Q' => '/', 'w' | 'W' => '\'', 'e' | 'E' => 'ק',
                                    'r' | 'R' => 'ר', 't' | 'T' => 'א', 'y' | 'Y' => 'ט',
                                    'u' | 'U' => 'ו', 'i' | 'I' => 'ן', 'o' | 'O' => 'ם',
                                    'p' | 'P' => 'פ', '[' => ']', ']' => '[',
                                    'a' | 'A' => 'ש', 's' | 'S' => 'ד', 'd' | 'D' => 'ג',
                                    'f' | 'F' => 'כ', 'g' | 'G' => 'ע', 'h' | 'H' => 'י',
                                    'j' | 'J' => 'ח', 'k' | 'K' => 'ל', 'l' | 'L' => 'ך',
                                    ';' => 'ף', '\'' => ',', 'z' | 'Z' => 'ז',
                                    'x' | 'X' => 'ס', 'c' | 'C' => 'ב', 'v' | 'V' => 'ה',
                                    'b' | 'B' => 'נ', 'n' | 'N' => 'מ', 'm' | 'M' => 'צ',
                                    ',' => 'ת', '.' => 'ץ', '/' => '.',
                                    _ => character,
                                };
                            }

                            if CTRL_PRESSED.load(Ordering::Relaxed) {
                                character = match character {
                                    // Ctrl+C — interrupt
                                    'c' | 'C' | 'ב' => '\x03',
                                    // Ctrl+V — paste (placeholder)
                                    'v' | 'V' | 'ה' => '\x16',
                                    // Ctrl+A — beginning of line
                                    'a' | 'A' | 'ש' => '\x01',
                                    // Ctrl+E — end of line
                                    'e' | 'E' | 'ק' => '\x05',
                                    // Ctrl+K — kill to end of line
                                    'k' | 'K' | 'ל' => '\x0B',
                                    // Ctrl+U — kill entire line
                                    'u' | 'U' | 'ו' => '\x15',
                                    // Ctrl+W — delete word backward
                                    'w' | 'W' | '\'' => '\x17',
                                    // Ctrl+S — save (in editor)
                                    's' | 'S' | 'ד' => '\x13',
                                    // Ctrl+Q — quit (in editor)
                                    'q' | 'Q' | '/' => '\x11',
                                    _ => character,
                                };
                            }

                            // Send the decoded char to the shell!
                            crate::shell::handle_key(character);
                        },
                        DecodedKey::RawKey(key) => {

                            let ctrl = CTRL_PRESSED.load(Ordering::Relaxed);
                            let code: Option<char> = match (key, ctrl) {
                                // Arrow keys
                                (KeyCode::ArrowUp,    _)     => Some('\x10'), // DLE  — history prev
                                (KeyCode::ArrowDown,  _)     => Some('\x0E'), // SO   — history next
                                (KeyCode::ArrowLeft,  false) => Some('\x02'), // STX  — cursor left
                                (KeyCode::ArrowLeft,  true)  => Some('\x1D'), // GS   — word left
                                (KeyCode::ArrowRight, false) => Some('\x06'), // ACK  — cursor right
                                (KeyCode::ArrowRight, true)  => Some('\x1E'), // RS   — word right
                                // Navigation
                                (KeyCode::Home,       _)     => Some('\x01'), // SOH  — beginning (= Ctrl+A)
                                (KeyCode::End,        _)     => Some('\x05'), // ENQ  — end       (= Ctrl+E)
                                (KeyCode::Delete,     _)     => Some('\x7F'), // DEL  — forward delete
                                // Scrolling
                                (KeyCode::PageUp,     _)     => Some('\x1B'), // ESC  — scroll up
                                (KeyCode::PageDown,   _)     => Some('\x1C'), // FS   — scroll down
                                _ => None,
                            };
                            if let Some(c) = code {
                                crate::shell::handle_key(c);
                            }
                        }
                    }
                }
            }
        }
    }
}