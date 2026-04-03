# Canvas 2D API — Design Spec

**Date:** 2026-04-04  
**Status:** Approved  

---

## Context

JSOS apps currently draw via flat, stateless functions (`os.window.drawRect`, `os.window.drawLine`, etc.) that write directly to the global framebuffer back-buffer. This makes complex graphics difficult: no paths, no fills, no transforms, no image compositing. This spec adds a stateful Canvas 2D context API — `os.window.getContext(winId)` — compatible enough with the HTML5 Canvas API that standard canvas-using JS code mostly works without changes.

---

## Architecture

### Files Modified / Created

| File | Change |
|---|---|
| `src/canvas.rs` | **New.** All canvas state structs, rasterization, color parsing. |
| `src/js_runtime.rs` | Add `CANVAS_CONTEXTS`, `getContext` binding, ~25 `js_canvas_*` native functions, `JS_ToFloat64` to extern block, `js_val_to_f64` helper. |
| `src/framebuffer.rs` | Add `PixelBufDrawTarget` (implements `embedded_graphics::DrawTarget` for `&mut [u32]`) and `draw_string_to_buffer` wrapper. |
| `src/main.rs` | Clean up `CANVAS_CONTEXTS` entries on window/process teardown (wherever `WINDOW_BUFFERS` entries are removed). |
| `docs/API.md` | Document the new `os.window.getContext` API. |

---

## `src/canvas.rs` — Data Structures

```rust
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
    pub fill_style: String,     // CSS color string, e.g. "#ff0000"
    pub stroke_style: String,
    pub line_width: f64,
    pub font: String,           // e.g. "16px monospace"
    pub transform: [f64; 6],    // [a, b, c, d, e, f] affine matrix
}

pub struct CanvasContext {
    pub win_id: u32,
    pub path: Vec<PathCmd>,
    pub current_pos: (f64, f64),
    pub transform: [f64; 6],    // identity = [1,0,0,1,0,0]
    pub state_stack: Vec<CanvasState>,
}
```

**Key invariant:** Style state (`fillStyle`, `strokeStyle`, `lineWidth`, `font`) lives on the JS object as plain properties — users write `ctx.fillStyle = 'red'` naturally. Native functions read them via `JS_GetPropertyStr(ctx, this_val, b"fillStyle\0")` at call time. `save()`/`restore()` snapshot and rewrite these JS properties. Only the path and transform live in `CanvasContext`.

---

## `src/canvas.rs` — Functions

```rust
// Color
pub fn parse_css_color(s: &str) -> (u8, u8, u8)
// Handles: #rgb, #rrggbb, rgb(...), rgba(...), ~20 named colors (black, white, red, ...)

// Transform
pub fn transform_point(m: &[f64; 6], x: f64, y: f64) -> (f64, f64)
pub fn multiply_transform(a: &[f64; 6], b: &[f64; 6]) -> [f64; 6]

// Pixel write (bounds-checked, ARGB format 0x00RRGGBB)
pub fn write_pixel(buf: &mut [u32], w: usize, h: usize, x: i32, y: i32, r: u8, g: u8, b: u8)

// Rasterizers (all write to &mut [u32] pixel buffer)
pub fn fill_rect_buf(buf, w, h, x, y, rw, rh, color, transform)
pub fn stroke_rect_buf(buf, w, h, x, y, rw, rh, color, lw, transform)
pub fn clear_rect_buf(buf, w, h, x, y, rw, rh)
pub fn draw_line_buf(buf, w, h, x0, y0, x1, y1, color, line_width)  // thick Bresenham
pub fn fill_path(buf, w, h, path: &[PathCmd], color, transform)     // scanline even-odd
pub fn stroke_path(buf, w, h, path: &[PathCmd], color, lw, transform)
pub fn blit_image(buf, w, h, src: &[u32], sw, sh, sx, sy, suw, suh, dx, dy, dw, dh)
// ^ nearest-neighbor scaling; sx/sy/suw/suh = source crop; dx/dy/dw/dh = dest

// Path flattening (arcs → polylines via angle stepping; beziers → de Casteljau)
fn flatten_path(path: &[PathCmd]) -> Vec<(f64, f64, f64, f64)>  // line segments [(x0,y0,x1,y1)]
fn arc_to_lines(cx, cy, r, start, end, ccw) -> Vec<(f64, f64)>
fn bezier_to_lines(cp1x, cp1y, cp2x, cp2y, x, y, from_x, from_y) -> Vec<(f64, f64)>
```

**Scanline fill algorithm:** Flatten path to segments, compute bounding box, for each scanline find all edge intersections, sort, fill between even-odd pairs.

**Thick line:** For `lineWidth > 1`, draw `ceil(lineWidth)` parallel lines offset perpendicular to the segment direction. For `lineWidth == 1`, plain Bresenham.

**Arc step count:** `max(8, (r * |end - start|).ceil() as usize)` line segments — visually smooth down to r=4.

---

## `src/framebuffer.rs` — Text Rendering to Buffer

```rust
// ~20-line DrawTarget impl wrapping &mut [u32]
pub struct PixelBufDrawTarget<'a> {
    pub buf: &'a mut [u32],
    pub width: usize,
    pub height: usize,
    pub color: (u8, u8, u8),
}

impl DrawTarget for PixelBufDrawTarget<'_> { ... }

// Public entry point used by canvas fillText
pub fn draw_string_to_buffer(
    buf: &mut [u32], buf_w: usize, buf_h: usize,
    text: &str, x: i32, y: i32,
    r: u8, g: u8, b: u8,
    large: bool,
)
```

`draw_string_to_buffer` creates a `PixelBufDrawTarget`, then calls the same `Text::new(...).draw(...)` path as `draw_string_sized`, reusing all u8g2 font data.

---

## `src/js_runtime.rs` — FFI Additions

**Extern block additions:**
```rust
fn JS_ToFloat64(ctx: *mut JSContext, pres: *mut f64, val: JSValue) -> c_int;
```

**New helpers:**
```rust
unsafe fn js_val_to_f64(ctx: *mut JSContext, val: JSValue) -> f64
unsafe fn js_str(ctx: *mut JSContext, s: &str) -> JSValue  // JS_NewStringLen wrapper
unsafe fn read_str_prop(ctx: *mut JSContext, obj: JSValue, key: &str) -> String
unsafe fn read_f64_prop(ctx: *mut JSContext, obj: JSValue, key: &str) -> f64
unsafe fn read_ctx_id(ctx: *mut JSContext, this_val: JSValue) -> u32  // reads this._id
```

**New global:**
```rust
lazy_static! {
    // Keyed by win_id — one CanvasContext per window.
    static ref CANVAS_CONTEXTS: Mutex<BTreeMap<u32, CanvasContext>> = Mutex::new(BTreeMap::new());
}
```

`_id` on the returned JS context object stores `win_id` directly — no separate context ID needed.  
`read_ctx_id` reads `this._id`, which is used to look up both `CANVAS_CONTEXTS` and (via `CanvasContext::win_id`) `WINDOW_BUFFERS`.

**`getContext` added to `os.window` namespace** in `register_os_namespace`:
```rust
set_func(ctx, window, "getContext", js_os_window_get_context, 1);
```

**`js_os_window_get_context` behavior:**
1. Read `win_id` from `argv[0]`
2. If `CANVAS_CONTEXTS` has no entry for `win_id`, insert a new `CanvasContext`; otherwise reuse existing state
3. Set `pixel_buffer_active = true` on the `WindowBuffer`
4. Create a fresh JS object; set `_id = win_id`, `fillStyle`, `strokeStyle`, `lineWidth`, `font` as JS properties
5. Attach all 25 canvas methods via `set_func`
6. Return the JS object (caller must `JS_FreeValue` it — standard QuickJS ownership)

---

## JS API Surface

```javascript
const ctx = os.window.getContext(winId);

// Settable properties (plain JS properties — read by native functions at draw time)
ctx.fillStyle   = '#rrggbb' | 'rgb(r,g,b)' | 'rgba(r,g,b,a)' | namedColor;
ctx.strokeStyle = '...';
ctx.lineWidth   = 1;           // default 1
ctx.font        = '16px monospace';  // size parsed: 8–15 → small font, 16+ → large font

// Rectangles
ctx.fillRect(x, y, w, h);
ctx.strokeRect(x, y, w, h);
ctx.clearRect(x, y, w, h);    // fills with 0x000000

// Paths
ctx.beginPath();               // clears current path
ctx.moveTo(x, y);
ctx.lineTo(x, y);
ctx.arc(cx, cy, r, startAngle, endAngle [, anticlockwise]);
ctx.bezierCurveTo(cp1x, cp1y, cp2x, cp2y, x, y);
ctx.quadraticCurveTo(cpx, cpy, x, y);
ctx.rect(x, y, w, h);         // adds rect subpath
ctx.closePath();               // line back to last moveTo
ctx.fill();                    // scanline fill current path with fillStyle
ctx.stroke();                  // stroke current path with strokeStyle + lineWidth

// Text
ctx.fillText(text, x, y);     // renders with current fillStyle color
ctx.strokeText(text, x, y);   // same as fillText (no separate stroke outline for text)

// Images (imgObj = result of os.image.decode(arrayBuffer))
ctx.drawImage(imgObj, dx, dy);
ctx.drawImage(imgObj, dx, dy, dw, dh);
ctx.drawImage(imgObj, sx, sy, sw, sh, dx, dy, dw, dh);

// Pixel data
ctx.getImageData(x, y, w, h);   // returns { width, height, data: Uint8ClampedArray (RGBA) }
ctx.putImageData(imageData, x, y);

// Transforms (2D affine, applied to all coordinates)
ctx.save();                    // push fillStyle, strokeStyle, lineWidth, font, transform
ctx.restore();                 // pop and restore
ctx.translate(x, y);
ctx.rotate(angle);             // radians; uses libm::sin/cos
ctx.scale(x, y);
ctx.setTransform(a, b, c, d, e, f);
ctx.resetTransform();          // identity [1,0,0,1,0,0]

// JSOS-specific (not in browser spec)
ctx.flush();                   // blit pixel buffer to framebuffer (delegates to os.window.flush)
```

---

## Rendering Pipeline

All canvas drawing targets the window's `pixels: Vec<u32>` (ARGB `0x00RRGGBB`) — never the global framebuffer directly. The transform is applied to coordinates before any pixel write.

`ctx.flush()` delegates to the existing `js_os_window_flush` path, which clones the pixel Vec and calls `blit_window_pixels`. The `pixel_buffer_active` flag is set when `getContext` is first called.

---

## Cleanup

When a window is destroyed (process exit or `os.exit()`), remove the corresponding entry from `CANVAS_CONTEXTS` keyed by `win_id`. This happens in the same code path that removes from `WINDOW_BUFFERS`.

---

## Verification

1. **Build:** `cargo build --target x86_64-os.json` — zero new warnings
2. **Smoke test app** — write `src/jsos/canvastest.jsos`:
   ```javascript
   const win = os.window.create(100, 100, 400, 300);
   const ctx = os.window.getContext(win);
   ctx.fillStyle = '#1a1a2e';
   ctx.fillRect(0, 0, 400, 300);          // background
   ctx.fillStyle = '#e94560';
   ctx.beginPath();
   ctx.arc(200, 150, 80, 0, Math.PI * 2);
   ctx.fill();                             // filled circle
   ctx.strokeStyle = '#ffffff';
   ctx.lineWidth = 3;
   ctx.beginPath();
   ctx.moveTo(50, 50);
   ctx.lineTo(350, 250);
   ctx.stroke();                           // diagonal line
   ctx.fillStyle = '#ffffff';
   ctx.fillText('Canvas 2D', 140, 270);
   const img = os.image.decode(someArrayBuffer);
   ctx.drawImage(img, 10, 10, 64, 64);    // image blit
   ctx.flush();
   ```
3. **Transform test:** translate + rotate + fill a rectangle; verify pixels appear at correct offset and angle
4. **save/restore:** set fillStyle, save, change fillStyle, draw, restore, draw — verify colors match expectations
5. **drawImage (3-arg, 5-arg, 9-arg):** all three overloads render correctly
