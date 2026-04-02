use bootloader::boot_info::{FrameBufferInfo, PixelFormat};
use core::fmt;
use embedded_graphics::{
    pixelcolor::{Rgb888, RgbColor},
    prelude::*,
};
use lazy_static::lazy_static;
use spin::Mutex;
use u8g2_fonts::{fonts, U8g2TextStyle};

lazy_static! {
    pub static ref FRAMEBUFFER_WRITER: Mutex<Option<FramebufferWriter>> = Mutex::new(None);
}

pub struct FramebufferWriter {
    pub front_buffer: &'static mut [u8],
    pub back_buffer: alloc::vec::Vec<u8>,
    pub bg_buffer: alloc::vec::Vec<u8>,
    pub info: FrameBufferInfo,
    pub x_pos: usize,
    pub y_pos: usize,
    pub foreground_color: (u8, u8, u8),
    pub background_color: (u8, u8, u8),
    /// Active scissor rect `(x, y, w, h)`.  When `Some`, all drawing ops are
    /// clipped to this region.  `None` = no clipping (default).
    pub clip: Option<(usize, usize, usize, usize)>,
}

pub const FONT_HEIGHT_SMALL: usize = 16;
pub const FONT_HEIGHT_LARGE: usize = 20;
const FONT_HEIGHT: usize = FONT_HEIGHT_SMALL;
const LINE_SPACING: usize = 2;

/// Return the pixel width a character occupies in our bitmap font grid.
/// - ASCII (U+0000–U+007F): 8 px
/// - BMP non-ASCII (U+0080–U+FFFF): 16 px
/// - SMP / surrogate (U+10000+): 0 px  (rendered as a BMP replacement box instead)
pub fn char_display_width(c: char) -> usize {
    let cp = c as u32;
    if cp < 0x80 {
        8
    } else if cp <= 0xFFFF {
        16
    } else {
        // SMP – we can't render these; caller should substitute U+25A1 (□)
        0
    }
}

impl FramebufferWriter {
    pub fn new(front_buffer: &'static mut [u8], info: FrameBufferInfo) -> Self {
        let byte_len = info.byte_len;
        let mut back_buffer = alloc::vec::Vec::with_capacity(byte_len);
        back_buffer.resize(byte_len, 0);
        let bg_buffer = back_buffer.clone();

        let mut writer = Self {
            front_buffer,
            back_buffer,
            bg_buffer,
            info,
            x_pos: 0,
            y_pos: 0,
            foreground_color: (220, 220, 220),
            background_color: (0, 0, 0),
            clip: None,
        };
        writer.clear_screen();
        writer
    }

    pub fn swap_buffers(&mut self) {
        self.front_buffer.copy_from_slice(&self.back_buffer);
    }

    pub fn draw_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if let Some((cx, cy, cw, ch)) = self.clip {
            if x < cx || x >= cx + cw || y < cy || y >= cy + ch { return; }
        }
        if x >= self.info.horizontal_resolution || y >= self.info.vertical_resolution {
            return;
        }

        let byte_offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        let color = match self.info.pixel_format {
            PixelFormat::RGB => [r, g, b, 0],
            PixelFormat::BGR => [b, g, r, 0],
            PixelFormat::U8 => [if r > 128 { 255 } else { 0 }, 0, 0, 0],
            other => panic!("Unsupported pixel format: {:?}", other),
        };

        for (i, byte) in color.iter().enumerate().take(self.info.bytes_per_pixel) {
            self.back_buffer[byte_offset + i] = *byte;
        }
    }

    pub fn fill_rect(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        r: u8,
        g: u8,
        b: u8,
    ) {
        // Intersect with active scissor rect before the screen-bounds clamp.
        let (x, y, width, height) = if let Some((cx, cy, cw, ch)) = self.clip {
            let nx = x.max(cx);
            let ny = y.max(cy);
            let nw = (x + width).min(cx + cw).saturating_sub(nx);
            let nh = (y + height).min(cy + ch).saturating_sub(ny);
            (nx, ny, nw, nh)
        } else {
            (x, y, width, height)
        };
        let start_y = y.min(self.info.vertical_resolution);
        let end_y = (y + height).min(self.info.vertical_resolution);
        let start_x = x.min(self.info.horizontal_resolution);
        let end_x = (x + width).min(self.info.horizontal_resolution);

        if start_x >= end_x || start_y >= end_y { return; }

        let color = match self.info.pixel_format {
            PixelFormat::RGB => [r, g, b, 0],
            PixelFormat::BGR => [b, g, r, 0],
            PixelFormat::U8 => [if r > 128 { 255 } else { 0 }, 0, 0, 0],
            other => panic!("Unsupported pixel format: {:?}", other),
        };
        let bpp = self.info.bytes_per_pixel;
        let stride = self.info.stride;
        let row_byte_len = (end_x - start_x) * bpp;

        // Write the first row pixel-by-pixel (no allocation).
        let first_row_offset = (start_y * stride + start_x) * bpp;
        for i in 0..(end_x - start_x) {
            let o = first_row_offset + i * bpp;
            self.back_buffer[o..o + bpp].copy_from_slice(&color[..bpp]);
        }

        // Copy the first row into every subsequent row using copy_within.
        // This is a single memmove per row — much faster than rebuilding each row
        // from scratch, and avoids any heap allocation.
        for row in (start_y + 1)..end_y {
            let dst = (row * stride + start_x) * bpp;
            self.back_buffer.copy_within(first_row_offset..first_row_offset + row_byte_len, dst);
        }

    }

    pub fn draw_line(
        &mut self,
        mut x0: isize,
        mut y0: isize,
        x1: isize,
        y1: isize,
        r: u8,
        g: u8,
        b: u8,
    ) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            if x0 >= 0 && y0 >= 0 {
                self.draw_pixel(x0 as usize, y0 as usize, r, g, b);
            }
            if x0 == x1 && y0 == y1 { break; }
            let e2 = 2 * err;
            if e2 >= dy { err += dy; x0 += sx; }
            if e2 <= dx { err += dx; y0 += sy; }
        }
    }

    pub fn draw_circle(&mut self, cx: isize, cy: isize, radius: isize, r: u8, g: u8, b: u8) {
        let mut x = radius;
        let mut y = 0;
        let mut err = 0;

        while x >= y {
            let points = [
                (cx + x, cy + y), (cx + y, cy + x),
                (cx - y, cy + x), (cx - x, cy + y),
                (cx - x, cy - y), (cx - y, cy - x),
                (cx + y, cy - x), (cx + x, cy - y),
            ];
            for (px, py) in points {
                if px >= 0 && py >= 0 {
                    self.draw_pixel(px as usize, py as usize, r, g, b);
                }
            }
            if err <= 0 { y += 1; err += 2 * y + 1; }
            else        { x -= 1; err -= 2 * x + 1; }
        }
    }

    pub fn fill_circle(&mut self, cx: isize, cy: isize, radius: isize, r: u8, g: u8, b: u8) {
        let mut x = radius;
        let mut y = 0isize;
        let mut err = 0isize;
        let h = self.info.vertical_resolution as isize;

        while x >= y {
            for (x0, x1, row) in [
                (cx - x, cx + x, cy + y),
                (cx - x, cx + x, cy - y),
                (cx - y, cx + y, cy + x),
                (cx - y, cx + y, cy - x),
            ] {
                if row < 0 || row >= h { continue; }
                let xs = x0.max(0) as usize;
                let xe = (x1 + 1).min(self.info.horizontal_resolution as isize).max(0) as usize;
                if xs < xe {
                    self.fill_rect(xs, row as usize, xe - xs, 1, r, g, b);
                }
            }
            if err <= 0 { y += 1; err += 2 * y + 1; }
            else        { x -= 1; err -= 2 * x + 1; }
        }
    }

    pub fn clear_screen(&mut self) {
        // Restore from the saved background buffer — one memcpy instead of
        // 600 row-buffer iterations. Falls back to fill_rect before save_bg
        // has been called (i.e. during early boot).
        if self.bg_buffer.iter().any(|&b| b != 0) {
            self.back_buffer.copy_from_slice(&self.bg_buffer);
        } else {
            let (r, g, b) = self.background_color;
            self.fill_rect(
                0, 0,
                self.info.horizontal_resolution,
                self.info.vertical_resolution,
                r, g, b,
            );
        }
        self.x_pos = 0;
        self.y_pos = 0;
    }

    pub fn save_bg(&mut self) {
        self.bg_buffer.copy_from_slice(&self.back_buffer);
    }

    pub fn clear_text(&mut self) {
        self.back_buffer.copy_from_slice(&self.bg_buffer);
        self.x_pos = 0;
        self.y_pos = 0;
    }

    pub fn write_char(&mut self, c: char) {
        match c {
            '\n' => self.newline(),
            '\r' => self.x_pos = 0,
            '\x08' => {
                let back_width = 8;
                if self.x_pos >= back_width {
                    self.x_pos -= back_width;
                } else if self.y_pos >= FONT_HEIGHT + LINE_SPACING {
                    self.y_pos -= FONT_HEIGHT + LINE_SPACING;
                    self.x_pos = self.info.horizontal_resolution - back_width;
                }
                let (r, g, b) = (20, 30, 40);
                self.fill_rect(self.x_pos, self.y_pos, 16, FONT_HEIGHT, r, g, b);
            }
            c => {
                // Substitute U+25A1 (□) for SMP codepoints we cannot render.
                let (render_c, char_width) = if c as u32 > 0xFFFF {
                    ('\u{25A1}', 16)
                } else {
                    (c, char_display_width(c))
                };

                if self.x_pos + char_width > self.info.horizontal_resolution {
                    self.newline();
                }

                let fg = Rgb888::new(
                    self.foreground_color.0,
                    self.foreground_color.1,
                    self.foreground_color.2,
                );

                let mut s = alloc::string::String::new();
                s.push(render_c);

                use embedded_graphics::text::Text;

                // Font fallback chain: multilingual → symbols → emoticons → skip
                let cp = render_c as u32;
                if cp < 0x80 || (cp >= 0x0080 && cp <= 0x05FF) || (cp >= 0x0600 && cp <= 0x06FF) {
                    // Latin, Latin-Extended, Hebrew, Arabic — primary font covers these
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_hebrew, fg);
                    let _ = Text::new(&s, Point::new(self.x_pos as i32, (self.y_pos + FONT_HEIGHT - 2) as i32), font).draw(self);
                } else if (cp >= 0x2000 && cp <= 0x27FF) || (cp >= 0x2B00 && cp <= 0x2BFF) || (cp >= 0xFB00 && cp <= 0xFB4F) {
                    // General Punctuation, Arrows, Dingbats — symbols font
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_symbols, fg);
                    let _ = Text::new(&s, Point::new(self.x_pos as i32, (self.y_pos + FONT_HEIGHT - 2) as i32), font).draw(self);
                } else if (cp >= 0x2600 && cp <= 0x26FF) || (cp >= 0x2700 && cp <= 0x27BF) {
                    // Misc Symbols, Dingbats — emoticons font
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_emoticons, fg);
                    let _ = Text::new(&s, Point::new(self.x_pos as i32, (self.y_pos + FONT_HEIGHT - 2) as i32), font).draw(self);
                } else {
                    // Remaining BMP coverage — try primary then fall through
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_hebrew, fg);
                    let _ = Text::new(&s, Point::new(self.x_pos as i32, (self.y_pos + FONT_HEIGHT - 2) as i32), font).draw(self);
                }

                self.x_pos += char_width;
            }
        }
    }

    fn newline(&mut self) {
        self.x_pos = 0;
        self.y_pos += FONT_HEIGHT + LINE_SPACING;
        if self.y_pos + FONT_HEIGHT > self.info.vertical_resolution {
            self.clear_text();
        }
    }

    pub fn write_string(&mut self, s: &str) {
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c.is_whitespace() && c != '\n' {
                self.write_char(c);
            } else if c == '\n' {
                self.write_char(c);
            } else {
                let remaining = &s[i..];
                let word = remaining.split_whitespace().next().unwrap_or(remaining);
                let word_len = word.chars().count();
                let word_width: usize = word.chars()
                    .map(|ch| if ch as u32 > 0xFFFF { 16 } else { char_display_width(ch) })
                    .sum();

                if self.x_pos + word_width > self.info.horizontal_resolution
                    && word_width <= self.info.horizontal_resolution
                {
                    self.newline();
                }

                self.write_char(c);
                for _ in 1..word_len {
                    if let Some((_, next_c)) = chars.next() {
                        self.write_char(next_c);
                    }
                }
            }
        }
    }
}

// ── Module-level public API ───────────────────────────────────────────────

pub fn swap_buffers() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.swap_buffers();
        }
    });
}

impl fmt::Write for FramebufferWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

impl DrawTarget for FramebufferWriter {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels.into_iter() {
            if coord.x >= 0 && coord.y >= 0 {
                self.draw_pixel(coord.x as usize, coord.y as usize, color.r(), color.g(), color.b());
            }
        }
        Ok(())
    }
}

impl OriginDimensions for FramebufferWriter {
    fn size(&self) -> Size {
        Size::new(
            self.info.horizontal_resolution as u32,
            self.info.vertical_resolution as u32,
        )
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::framebuffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.write_fmt(args).unwrap();
        }
    });
}

pub fn set_foreground_color(r: u8, g: u8, b: u8) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.foreground_color = (r, g, b);
        }
    });
}

pub fn draw_line(x0: isize, y0: isize, x1: isize, y1: isize, r: u8, g: u8, b: u8) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.draw_line(x0, y0, x1, y1, r, g, b);
        }
    });
}

pub fn draw_circle(cx: isize, cy: isize, radius: isize, r: u8, g: u8, b: u8) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.draw_circle(cx, cy, radius, r, g, b);
        }
    });
}

pub fn fill_circle(cx: isize, cy: isize, radius: isize, r: u8, g: u8, b: u8) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.fill_circle(cx, cy, radius, r, g, b);
        }
    });
}

pub fn fill_rect(x: usize, y: usize, width: usize, height: usize, r: u8, g: u8, b: u8) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.fill_rect(x, y, width, height, r, g, b);
        }
    });
}

/// Blit a window's ARGB pixel buffer (0x00RRGGBB per element) to the back-buffer
/// at the given screen position, clipping to the screen bounds.
pub fn blit_window_pixels(win_x: usize, win_y: usize, win_w: usize, win_h: usize, pixels: &[u32]) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            let screen_w = writer.info.horizontal_resolution;
            let screen_h = writer.info.vertical_resolution;
            let bpp      = writer.info.bytes_per_pixel;
            let stride   = writer.info.stride;

            for row in 0..win_h {
                let screen_y = win_y + row;
                if screen_y >= screen_h { break; }
                for col in 0..win_w {
                    let screen_x = win_x + col;
                    if screen_x >= screen_w { continue; }

                    let px = pixels[row * win_w + col];
                    let r = ((px >> 16) & 0xFF) as u8;
                    let g = ((px >>  8) & 0xFF) as u8;
                    let b = ( px        & 0xFF) as u8;

                    let color = match writer.info.pixel_format {
                        PixelFormat::RGB => [r, g, b, 0],
                        PixelFormat::BGR => [b, g, r, 0],
                        PixelFormat::U8  => [if r > 128 { 255 } else { 0 }, 0, 0, 0],
                        _ => [r, g, b, 0],
                    };

                    let byte_offset = (screen_y * stride + screen_x) * bpp;
                    for (i, byte) in color.iter().enumerate().take(bpp) {
                        writer.back_buffer[byte_offset + i] = *byte;
                    }
                }
            }
        }
    });
}

pub fn init(framebuffer: &'static mut [u8], info: FrameBufferInfo) {
    let writer = FramebufferWriter::new(framebuffer, info);
    *FRAMEBUFFER_WRITER.lock() = Some(writer);
}

pub fn clear_screen() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.clear_screen();
        }
    });
}

pub fn clear_text() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.clear_text();
        }
    });
}

pub fn save_bg() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.save_bg();
        }
    });
}

/// Set a scissor/clip rect.  All subsequent drawing calls are restricted to
/// pixels within `(x, y, w, h)` until `unset_clip()` is called.
pub fn set_clip(x: usize, y: usize, w: usize, h: usize) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.clip = Some((x, y, w, h));
        }
    });
}

/// Remove the active scissor rect, allowing drawing to the full framebuffer.
pub fn unset_clip() {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            writer.clip = None;
        }
    });
}

pub fn get_resolution() -> (usize, usize) {
    use x86_64::instructions::interrupts;
    let mut res = (800, 600);
    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_ref() {
            res = (writer.info.horizontal_resolution, writer.info.vertical_resolution);
        }
    });
    res
}

pub fn draw_string(text: &str, x: usize, y: usize) {
    draw_string_sized(text, x, y, false);
}

/// Draw text at (x, y) using either the small (8×16) or large (10×20) bitmap font.
/// `large = false` → `u8g2_font_unifont_t_hebrew` (default, 8 px wide / 16 px tall)
/// `large = true`  → `u8g2_font_10x20_tf`         (10 px wide / 20 px tall)
pub fn draw_string_sized(text: &str, x: usize, y: usize, large: bool) {
    use embedded_graphics::text::Text;
    use u8g2_fonts::{fonts, U8g2TextStyle};
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            let fg = Rgb888::new(
                writer.foreground_color.0,
                writer.foreground_color.1,
                writer.foreground_color.2,
            );
            if large {
                let font = U8g2TextStyle::new(fonts::u8g2_font_10x20_tf, fg);
                let _ = Text::new(text, Point::new(x as i32, y as i32), font).draw(writer);
            } else {
                let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_hebrew, fg);
                let _ = Text::new(text, Point::new(x as i32, y as i32), font).draw(writer);
            }
        }
    });
}

/// Draw text at (x, y) with automatic per-character font fallback.
///
/// Character coverage:
/// - ASCII + Latin Extended + Hebrew + Arabic: `u8g2_font_unifont_t_hebrew`
/// - General Punctuation, Arrows, Letterlike Symbols: `u8g2_font_unifont_t_symbols`
/// - Misc Symbols (☀★♫) + Dingbats (✔✖): `u8g2_font_unifont_t_emoticons`
/// - SMP codepoints (U+10000+, e.g. 🌍): rendered as □ (U+25A1)
pub fn draw_string_unicode(text: &str, x: usize, y: usize) {
    use embedded_graphics::text::Text;
    use u8g2_fonts::{fonts, U8g2TextStyle};
    use x86_64::instructions::interrupts;

    interrupts::without_interrupts(|| {
        if let Some(writer) = FRAMEBUFFER_WRITER.lock().as_mut() {
            let fg = Rgb888::new(
                writer.foreground_color.0,
                writer.foreground_color.1,
                writer.foreground_color.2,
            );

            let mut cursor_x = x as i32;
            let cursor_y = y as i32;

            for c in text.chars() {
                let (render_c, glyph_w) = if c as u32 > 0xFFFF {
                    ('\u{25A1}', 16i32)
                } else {
                    (c, char_display_width(c) as i32)
                };

                let mut s = alloc::string::String::new();
                s.push(render_c);

                let cp = render_c as u32;
                let pos = Point::new(cursor_x, cursor_y);

                if (cp >= 0x2600 && cp <= 0x26FF) || (cp >= 0x2700 && cp <= 0x27BF) {
                    // Misc Symbols + Dingbats
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_emoticons, fg);
                    let _ = Text::new(&s, pos, font).draw(writer);
                } else if (cp >= 0x2000 && cp <= 0x25FF) || (cp >= 0x2B00 && cp <= 0x2BFF) {
                    // General Punctuation, Arrows, Box Drawing, Letterlike
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_symbols, fg);
                    let _ = Text::new(&s, pos, font).draw(writer);
                } else {
                    // Everything else: primary multilingual font
                    let font = U8g2TextStyle::new(fonts::u8g2_font_unifont_t_hebrew, fg);
                    let _ = Text::new(&s, pos, font).draw(writer);
                }

                cursor_x += glyph_w;
            }
        }
    });
}

pub fn get_screenshot_bmp_small() -> alloc::vec::Vec<u8> {
    let writer_lock = FRAMEBUFFER_WRITER.lock();
    if let Some(ref writer) = *writer_lock {
        let w = writer.info.horizontal_resolution;
        let h = writer.info.vertical_resolution;
        let step = 4;
        let target_w = w / step;
        let target_h = h / step;
        
        let row_bytes = target_w * 3;
        let padding = (4 - (row_bytes % 4)) % 4;
        let padded_row_size = row_bytes + padding;
        let pixel_data_size = padded_row_size * target_h;
        let file_size = 54 + pixel_data_size;
        
        let mut bmp = alloc::vec::Vec::with_capacity(file_size);
        
        // 14-byte BMP Header
        bmp.push(b'B'); bmp.push(b'M');
        bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
        bmp.extend_from_slice(&[0, 0, 0, 0]); // Reserved
        bmp.extend_from_slice(&54u32.to_le_bytes()); // Data offset
        
        // 40-byte DIB Header (BITMAPINFOHEADER)
        bmp.extend_from_slice(&40u32.to_le_bytes()); // Header size
        bmp.extend_from_slice(&(target_w as u32).to_le_bytes()); // Width
        bmp.extend_from_slice(&(-(target_h as i32)).to_le_bytes()); // Height (negative for top-down)
        bmp.extend_from_slice(&1u16.to_le_bytes()); // Color planes
        bmp.extend_from_slice(&24u16.to_le_bytes()); // Bits per pixel
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Compression (BI_RGB)
        bmp.extend_from_slice(&(pixel_data_size as u32).to_le_bytes()); // Image size
        bmp.extend_from_slice(&0u32.to_le_bytes()); // X pixels per meter
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Y pixels per meter
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Colors used
        bmp.extend_from_slice(&0u32.to_le_bytes()); // Important colors
        
        let stride = writer.info.stride;
        let bpp = writer.info.bytes_per_pixel;
        
        for y in (0..target_h).map(|y| y * step) {
            for x in (0..target_w).map(|x| x * step) {
                let offset = (y * stride + x) * bpp;
                if offset + 2 < writer.back_buffer.len() {
                    let r = match writer.info.pixel_format {
                        PixelFormat::RGB => writer.back_buffer[offset],
                        PixelFormat::BGR => writer.back_buffer[offset + 2],
                        _ => 128,
                    };
                    let g = writer.back_buffer[offset + 1];
                    let b = match writer.info.pixel_format {
                        PixelFormat::RGB => writer.back_buffer[offset + 2],
                        PixelFormat::BGR => writer.back_buffer[offset],
                        _ => 128,
                    };
                    // BMP requires BGR order
                    bmp.push(b);
                    bmp.push(g);
                    bmp.push(r);
                } else {
                    bmp.push(0); bmp.push(0); bmp.push(0);
                }
            }
            // Add row padding
            for _ in 0..padding {
                bmp.push(0);
            }
        }
        bmp
    } else {
        alloc::vec::Vec::new()
    }
}