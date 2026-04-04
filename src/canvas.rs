// src/canvas.rs
use alloc::vec::Vec;
use alloc::string::String;

#[derive(Clone)]
pub enum PathCmd {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    Arc { cx: f64, cy: f64, r: f64, start: f64, end: f64, ccw: bool },
    BezierCurveTo { cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64 },
    QuadraticCurveTo { cpx: f64, cpy: f64, x: f64, y: f64 },
    Rect(f64, f64, f64, f64),
    ClosePath,
}

pub struct CanvasState {
    pub fill_style: String,
    pub stroke_style: String,
    pub line_width: f64,
    pub font: String,
    pub transform: [f64; 6],
}

pub struct CanvasContext {
    pub win_id: u32,
    pub path: Vec<PathCmd>,
    pub current_pos: (f64, f64),
    pub subpath_start: (f64, f64),
    pub transform: [f64; 6],   // identity = [1,0,0,1,0,0]
    pub state_stack: Vec<CanvasState>,
}

impl CanvasContext {
    pub fn new(win_id: u32) -> Self {
        Self {
            win_id,
            path: Vec::new(),
            current_pos: (0.0, 0.0),
            subpath_start: (0.0, 0.0),
            transform: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            state_stack: Vec::new(),
        }
    }
}

/// Parse a CSS color string to (r, g, b). Falls back to black on error.
/// Handles: #rgb, #rrggbb, rgb(r,g,b), rgba(r,g,b,a), and named colors.
pub fn parse_css_color(s: &str) -> (u8, u8, u8) {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    if let Some(inner) = s.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb_components(inner);
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb_components(inner);
    }
    named_color(s)
}

fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).unwrap_or(0) * 17;
            let g = u8::from_str_radix(&hex[1..2], 16).unwrap_or(0) * 17;
            let b = u8::from_str_radix(&hex[2..3], 16).unwrap_or(0) * 17;
            (r, g, b)
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
            let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
            let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
            (r, g, b)
        }
        _ => (0, 0, 0),
    }
}

fn parse_rgb_components(s: &str) -> (u8, u8, u8) {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() < 3 { return (0, 0, 0); }
    let r = parts[0].trim().parse::<f64>().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    let g = parts[1].trim().parse::<f64>().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    let b = parts[2].trim().parse::<f64>().unwrap_or(0.0).clamp(0.0, 255.0) as u8;
    (r, g, b)
}

fn named_color(name: &str) -> (u8, u8, u8) {
    match name {
        "black"   => (0, 0, 0),
        "white"   => (255, 255, 255),
        "red"     => (255, 0, 0),
        "green"   => (0, 128, 0),
        "blue"    => (0, 0, 255),
        "yellow"  => (255, 255, 0),
        "orange"  => (255, 165, 0),
        "purple"  => (128, 0, 128),
        "pink"    => (255, 192, 203),
        "gray" | "grey"   => (128, 128, 128),
        "cyan"    => (0, 255, 255),
        "magenta" => (255, 0, 255),
        "lime"    => (0, 255, 0),
        "maroon"  => (128, 0, 0),
        "navy"    => (0, 0, 128),
        "teal"    => (0, 128, 128),
        "silver"  => (192, 192, 192),
        "brown"   => (165, 42, 42),
        "transparent" => (0, 0, 0),
        _         => (0, 0, 0),
    }
}

/// Apply 2D affine matrix [a,b,c,d,e,f] to point (x,y).
/// Transform: x' = a*x + c*y + e,  y' = b*x + d*y + f
pub fn transform_point(m: &[f64; 6], x: f64, y: f64) -> (f64, f64) {
    (m[0]*x + m[2]*y + m[4], m[1]*x + m[3]*y + m[5])
}

/// Combine two affine transforms: result = a * b (apply b first, then a).
pub fn multiply_transform(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6] {
    [
        a[0]*b[0] + a[2]*b[1],
        a[1]*b[0] + a[3]*b[1],
        a[0]*b[2] + a[2]*b[3],
        a[1]*b[2] + a[3]*b[3],
        a[0]*b[4] + a[2]*b[5] + a[4],
        a[1]*b[4] + a[3]*b[5] + a[5],
    ]
}

/// Write one pixel to a `0x00RRGGBB` pixel buffer with bounds check.
#[inline]
pub fn write_pixel(buf: &mut [u32], bw: usize, bh: usize, x: i32, y: i32, r: u8, g: u8, b: u8) {
    if x < 0 || y < 0 || x as usize >= bw || y as usize >= bh { return; }
    buf[y as usize * bw + x as usize] = ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
}

pub fn fill_rect_buf(
    buf: &mut [u32], bw: usize, bh: usize,
    x: f64, y: f64, w: f64, h: f64,
    color: (u8, u8, u8), transform: &[f64; 6],
) {
    let corners = [
        transform_point(transform, x, y),
        transform_point(transform, x + w, y),
        transform_point(transform, x + w, y + h),
        transform_point(transform, x, y + h),
    ];
    let segs = [
        (corners[0].0, corners[0].1, corners[1].0, corners[1].1),
        (corners[1].0, corners[1].1, corners[2].0, corners[2].1),
        (corners[2].0, corners[2].1, corners[3].0, corners[3].1),
        (corners[3].0, corners[3].1, corners[0].0, corners[0].1),
    ];
    scanline_fill(buf, bw, bh, &segs, color);
}

pub fn clear_rect_buf(buf: &mut [u32], bw: usize, bh: usize, x: f64, y: f64, w: f64, h: f64) {
    let x0 = (libm::floor(x) as i32).max(0);
    let y0 = (libm::floor(y) as i32).max(0);
    let x1 = (libm::ceil(x + w) as i32).min(bw as i32);
    let y1 = (libm::ceil(y + h) as i32).min(bh as i32);
    for row in y0..y1 {
        for col in x0..x1 {
            buf[row as usize * bw + col as usize] = 0;
        }
    }
}

pub fn stroke_rect_buf(
    buf: &mut [u32], bw: usize, bh: usize,
    x: f64, y: f64, w: f64, h: f64,
    color: (u8, u8, u8), line_width: f64, transform: &[f64; 6],
) {
    let corners = [
        transform_point(transform, x, y),
        transform_point(transform, x + w, y),
        transform_point(transform, x + w, y + h),
        transform_point(transform, x, y + h),
    ];
    draw_line_buf(buf, bw, bh, corners[0].0, corners[0].1, corners[1].0, corners[1].1, color, line_width);
    draw_line_buf(buf, bw, bh, corners[1].0, corners[1].1, corners[2].0, corners[2].1, color, line_width);
    draw_line_buf(buf, bw, bh, corners[2].0, corners[2].1, corners[3].0, corners[3].1, color, line_width);
    draw_line_buf(buf, bw, bh, corners[3].0, corners[3].1, corners[0].0, corners[0].1, color, line_width);
}

/// Bresenham line with thickness: for each point on the 1px line, fills a square of radius `lw`.
pub fn draw_line_buf(
    buf: &mut [u32], bw: usize, bh: usize,
    x0: f64, y0: f64, x1: f64, y1: f64,
    color: (u8, u8, u8), line_width: f64,
) {
    let lw = (libm::ceil(line_width / 2.0) as i32).max(0);
    let mut x0i = libm::round(x0) as i32;
    let mut y0i = libm::round(y0) as i32;
    let x1i = libm::round(x1) as i32;
    let y1i = libm::round(y1) as i32;
    let dx = (x1i - x0i).abs();
    let dy = -(y1i - y0i).abs();
    let sx = if x0i < x1i { 1 } else { -1 };
    let sy = if y0i < y1i { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        for dy2 in -lw..=lw {
            for dx2 in -lw..=lw {
                write_pixel(buf, bw, bh, x0i + dx2, y0i + dy2, color.0, color.1, color.2);
            }
        }
        if x0i == x1i && y0i == y1i { break; }
        let e2 = 2 * err;
        if e2 >= dy { err += dy; x0i += sx; }
        if e2 <= dx { err += dx; y0i += sy; }
    }
}

/// Scanline even-odd fill for a list of line segments (x0,y0,x1,y1).
pub fn scanline_fill(
    buf: &mut [u32], bw: usize, bh: usize,
    segments: &[(f64, f64, f64, f64)],
    color: (u8, u8, u8),
) {
    if segments.is_empty() { return; }
    let mut min_y = f64::MAX;
    let mut max_y = f64::MIN;
    for &(x0, y0, x1, y1) in segments {
        let _ = x0; let _ = x1;
        if y0 < min_y { min_y = y0; }
        if y1 < min_y { min_y = y1; }
        if y0 > max_y { max_y = y0; }
        if y1 > max_y { max_y = y1; }
    }
    let start_y = (libm::ceil(min_y) as i32).max(0);
    let end_y   = (libm::ceil(max_y) as i32).min(bh as i32);

    let mut xs: Vec<f64> = Vec::new();
    for y in start_y..end_y {
        let yf = y as f64 + 0.5;
        xs.clear();
        for &(x0, y0, x1, y1) in segments {
            let (lo, hi) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
            if yf < lo || yf >= hi { continue; }
            let t = (yf - y0) / (y1 - y0);
            xs.push(x0 + t * (x1 - x0));
        }
        if xs.len() < 2 { continue; }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            let x_start = (libm::ceil(xs[i]) as i32).max(0);
            let x_end   = (libm::ceil(xs[i+1]) as i32).min(bw as i32);
            for x in x_start..x_end {
                write_pixel(buf, bw, bh, x, y, color.0, color.1, color.2);
            }
            i += 2;
        }
    }
}
