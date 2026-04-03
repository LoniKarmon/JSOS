// src/canvas.rs
use alloc::vec::Vec;
use alloc::string::String;

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
