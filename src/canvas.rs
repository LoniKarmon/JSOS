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
