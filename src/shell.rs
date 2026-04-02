// src/shell.rs
// Thin keyboard router bridging hardware interrupts directly to Javascript Usermode.

pub fn handle_key(c: char) {
    if c as u8 == 0 {
        return;
    }
    crate::process::send_key(c);
}

pub fn init() {}
