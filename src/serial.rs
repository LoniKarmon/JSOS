/// 32 KB circular buffer that shadows every serial_print! call.
/// Sized to hold ~3000 typical 80-char lines — enough for full boot trace.
const LOG_CAP: usize = 32 * 1024;

struct LogBuf {
    data: [u8; LOG_CAP],
    write: usize, // next write position (mod LOG_CAP)
    total: usize, // total bytes ever written
}

impl LogBuf {
    const fn new() -> Self {
        Self { data: [0u8; LOG_CAP], write: 0, total: 0 }
    }

    fn push(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.data[self.write] = b;
            self.write = (self.write + 1) % LOG_CAP;
            self.total += 1;
        }
    }

    /// Return the entire log (oldest → newest) as a heap String.
    /// Only callable after the allocator is up.
    pub fn snapshot(&self) -> alloc::string::String {
        let filled = self.total.min(LOG_CAP);
        let start = if self.total >= LOG_CAP { self.write } else { 0 };
        let mut out = alloc::string::String::with_capacity(filled);
        for i in 0..filled {
            let b = self.data[(start + i) % LOG_CAP];
            // Replace non-printable bytes (except \n/\t) with '?'
            let c = if b == b'\n' || b == b'\t' || (b >= 0x20 && b < 0x7F) { b } else { b'?' };
            out.push(c as char);
        }
        out
    }
}

static LOG: spin::Mutex<LogBuf> = spin::Mutex::new(LogBuf::new());

/// Return a snapshot of all serial output since boot.
pub fn serial_log_snapshot() -> alloc::string::String {
    LOG.lock().snapshot()
}

pub unsafe fn primitive_serial_print(s: &str) {
    let port = 0x3F8;
    for &b in s.as_bytes() {
        while (x86_64::instructions::port::Port::<u8>::new(port + 5).read() & 0x20) == 0 {}
        x86_64::instructions::port::Port::<u8>::new(port).write(b);
    }
}

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    struct PrimitiveWriter;
    impl core::fmt::Write for PrimitiveWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let port = 0x3F8;
            for &b in s.as_bytes() {
                unsafe {
                    // Just write, don't wait.
                    x86_64::instructions::port::Port::<u8>::new(port).write(b);
                }
            }
            Ok(())
        }
    }

    interrupts::without_interrupts(|| {
        // Write to COM1
        let mut writer = PrimitiveWriter;
        writer.write_fmt(args).expect("Printing to serial failed");
        // Mirror to in-memory ring buffer for os.serialLog()
        if let Some(mut log) = LOG.try_lock() {
            // Re-format into the buffer
            struct BufWriter<'a>(&'a mut LogBuf);
            impl<'a> core::fmt::Write for BufWriter<'a> {
                fn write_str(&mut self, s: &str) -> core::fmt::Result {
                    self.0.push(s);
                    Ok(())
                }
            }
            let _ = core::fmt::write(&mut BufWriter(&mut log), args);
        }
    });
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*));
    };
}

/// Prints to the host through the serial interface, appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}



