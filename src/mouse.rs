use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;
use core::sync::atomic::{AtomicI32, AtomicU8, AtomicUsize, Ordering};

pub static MOUSE_X:      AtomicI32 = AtomicI32::new(400);
pub static MOUSE_Y:      AtomicI32 = AtomicI32::new(300);
pub static MOUSE_BTN:    AtomicU8  = AtomicU8::new(0);
// Scroll delta from the IntelliMouse 4th byte.
// Positive = scroll down (toward user), negative = scroll up.
// Cleared to 0 by winman after it reads the value each frame.
pub static MOUSE_SCROLL: AtomicI32 = AtomicI32::new(0);

const MOUSE_CMD_PORT:  u16 = 0x64;
const MOUSE_DATA_PORT: u16 = 0x60;

// ── Top half: lock-free hardware byte queue ───────────────────────────────

const MOUSE_QUEUE_SIZE: usize = 256;
static MOUSE_QUEUE: [AtomicU8; MOUSE_QUEUE_SIZE] = {
    const INIT: AtomicU8 = AtomicU8::new(0);
    [INIT; MOUSE_QUEUE_SIZE]
};
static MOUSE_HEAD: AtomicUsize = AtomicUsize::new(0);
static MOUSE_TAIL: AtomicUsize = AtomicUsize::new(0);

/// Called ONLY by the hardware interrupt handler.
pub fn push_mouse_byte(byte: u8) {
    let tail = MOUSE_TAIL.load(Ordering::Relaxed);
    let next_tail = (tail + 1) % MOUSE_QUEUE_SIZE;
    if next_tail != MOUSE_HEAD.load(Ordering::Acquire) {
        MOUSE_QUEUE[tail].store(byte, Ordering::Release);
        MOUSE_TAIL.store(next_tail, Ordering::Release);
    }
}

fn pop_mouse_byte() -> Option<u8> {
    let head = MOUSE_HEAD.load(Ordering::Relaxed);
    if head == MOUSE_TAIL.load(Ordering::Acquire) {
        return None;
    }
    let byte = MOUSE_QUEUE[head].load(Ordering::Acquire);
    MOUSE_HEAD.store((head + 1) % MOUSE_QUEUE_SIZE, Ordering::Release);
    Some(byte)
}

/// Called ONLY by the main kernel loop.
pub fn process_mouse_queue() {
    let mut mouse_locked = None;
    while let Some(byte) = pop_mouse_byte() {
        if mouse_locked.is_none() {
            mouse_locked = Some(MOUSE.lock());
        }
        if let Some(mouse) = mouse_locked.as_mut() {
            mouse.process_packet(byte);
        }
    }
}

// ── Bottom half: state machine + hardware init ────────────────────────────

lazy_static! {
    pub static ref MOUSE: Mutex<Mouse> = Mutex::new(Mouse::new());
}

pub struct Mouse {
    state:           u8,
    packet:          [u8; 4], // 4 bytes for IntelliMouse, 3 for standard
    intellimouse:    bool,     // true once we've confirmed scroll wheel support
}

impl Mouse {
    pub fn new() -> Self {
        Mouse { state: 0, packet: [0; 4], intellimouse: false }
    }

    pub fn init(&mut self) {
        unsafe {
            let mut cmd_port  = Port::<u8>::new(MOUSE_CMD_PORT);
            let mut data_port = Port::<u8>::new(MOUSE_DATA_PORT);

            // 1. Enable auxiliary mouse device
            cmd_port.write(0xA8u8);

            // 2. Read and patch the Compaq status byte
            cmd_port.write(0x20u8);
            Self::wait_for_read();
            let mut status: u8 = data_port.read();
            status |= 0b0000_0010; // enable IRQ12
            status &= 0b1101_1111; // clear Disable Mouse Clock
            cmd_port.write(0x60u8);
            Self::wait_for_write();
            data_port.write(status);

            // 3. Default settings
            Self::write_to_mouse(0xF6);
            Self::read_from_mouse();

            // 4. IntelliMouse activation sequence:
            //    Set sample rate 200, 100, 80 in sequence — the mouse responds
            //    by switching to 4-byte packets with scroll wheel data in byte 4.
            for &rate in &[200u8, 100, 80] {
                Self::write_to_mouse(0xF3); // Set Sample Rate
                Self::read_from_mouse();    // ACK
                Self::write_to_mouse(rate);
                Self::read_from_mouse();    // ACK
            }

            // 5. Read device ID — 0x03 means IntelliMouse (scroll wheel) active
            Self::write_to_mouse(0xF2); // Get Device ID
            Self::read_from_mouse();    // ACK
            let device_id = Self::read_from_mouse();
            self.intellimouse = device_id == 0x03;
            crate::serial_println!(
                "[Mouse] Device ID: 0x{:02X} — IntelliMouse: {}",
                device_id, self.intellimouse
            );

            // 6. Enable data reporting
            Self::write_to_mouse(0xF4);
            Self::read_from_mouse();

            // 7. Flush any leftover ACK bytes
            while Self::can_read() {
                let _ = data_port.read();
            }
        }
    }

    pub fn process_packet(&mut self, payload: u8) {
        let packet_len: u8 = if self.intellimouse { 4 } else { 3 };

        match self.state {
            0 => {
                if payload == 0xFA { return; } // stray ACK
                if payload & 0x08 != 0 {       // sync bit always set in byte 0
                    self.packet[0] = payload;
                    self.state = 1;
                }
            }
            1 => { self.packet[1] = payload; self.state = 2; }
            2 => {
                self.packet[2] = payload;
                if packet_len == 3 {
                    self.state = 0;
                    self.parse_and_dispatch();
                } else {
                    self.state = 3;
                }
            }
            3 => {
                self.packet[3] = payload;
                self.state = 0;
                self.parse_and_dispatch();
            }
            _ => self.state = 0,
        }
    }

    fn parse_and_dispatch(&mut self) {
        let flags = self.packet[0];

        // Discard overflowed packets
        if flags & 0xC0 != 0 { return; }

        // 9-bit signed X/Y deltas
        let x_movement = self.packet[1] as i32 - if flags & 0x10 != 0 { 256 } else { 0 };
        let y_movement = self.packet[2] as i32 - if flags & 0x20 != 0 { 256 } else { 0 };

        let btn_state = flags & 0x07; // bits 0-2: left, right, middle
        MOUSE_BTN.store(btn_state, Ordering::Relaxed);

        let (max_w, max_h) = crate::framebuffer::get_resolution();
        let new_x = (MOUSE_X.load(Ordering::Relaxed) + x_movement).clamp(0, max_w as i32 - 1);
        let new_y = (MOUSE_Y.load(Ordering::Relaxed) - y_movement).clamp(0, max_h as i32 - 1);
        MOUSE_X.store(new_x, Ordering::Relaxed);
        MOUSE_Y.store(new_y, Ordering::Relaxed);

        // Scroll wheel — byte 4, bits 0-3, signed 4-bit two's complement
        if self.intellimouse {
            let raw = (self.packet[3] & 0x0F) as i32;
            let delta = if raw >= 8 { raw - 16 } else { raw }; // sign-extend 4-bit
            if delta != 0 {
                // Accumulate scroll — winman reads and resets this each frame
                MOUSE_SCROLL.fetch_add(delta, Ordering::Relaxed);
            }
        }
    }

    unsafe fn can_read() -> bool {
        (Port::<u8>::new(MOUSE_CMD_PORT).read() & 0x01) != 0
    }

    unsafe fn wait_for_read() {
        let mut cmd_port = Port::<u8>::new(MOUSE_CMD_PORT);
        for _ in 0..100_000 {
            if cmd_port.read() & 0x01 != 0 { return; }
            Self::io_wait();
        }
    }

    unsafe fn wait_for_write() {
        let mut cmd_port = Port::<u8>::new(MOUSE_CMD_PORT);
        for _ in 0..100_000 {
            if cmd_port.read() & 0x02 == 0 { return; }
            Self::io_wait();
        }
    }

    unsafe fn io_wait() {
        Port::<u8>::new(0x80).write(0u8);
    }

    unsafe fn write_to_mouse(data: u8) {
        let mut cmd_port  = Port::<u8>::new(MOUSE_CMD_PORT);
        let mut data_port = Port::<u8>::new(MOUSE_DATA_PORT);
        Self::wait_for_write();
        cmd_port.write(0xD4u8);
        Self::wait_for_write();
        data_port.write(data);
    }

    unsafe fn read_from_mouse() -> u8 {
        Self::wait_for_read();
        Port::<u8>::new(MOUSE_DATA_PORT).read()
    }
}