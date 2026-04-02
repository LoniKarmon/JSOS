# Image Decoding — Design Spec
_Date: 2026-03-29_

## Overview

Add JPEG and PNG image decoding to JSOS via a new `os.image` JS namespace. Decoded pixels are returned as an `ArrayBuffer` in `0x00RRGGBB` format, ready to write directly into a window pixel buffer obtained from `os.window.getPixelBuffer`. Also adds byte-returning variants of `os.store.get` and `os.base64.decode` to make binary data handling ergonomic.

---

## Architecture

Three components, each independently testable:

### 1. `src/image.rs` — Decode layer

Pure Rust, no_std+alloc module. Public interface:

```rust
pub fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u32>)>
```

- Detects format from magic bytes: `FF D8 FF` → JPEG, `89 50 4E 47` → PNG.
- Delegates to the appropriate crate decoder.
- Normalizes output to `Vec<u32>` of `0x00RRGGBB` pixels (one u32 per pixel, alpha ignored).
- Returns `None` on corrupt data or unsupported format.

### 2. `Cargo.toml` — Decoder crates

Two new dependencies, both `default-features = false` to remain no_std+alloc compatible:

| Crate | Purpose |
|-------|---------|
| `jpeg-decoder` | JPEG decode (no_std+alloc, no rayon) |
| `zune-png` | PNG decode (no_std+alloc fallback) |

If either crate fails to compile in the freestanding environment, fall back to `stb_image.h` (single-header C library, already supported by the existing clang build pipeline).

### 3. `src/js_runtime.rs` — JS bindings

New `os.image` sub-namespace with one function. Three additional functions added to existing namespaces.

---

## JS API

### `os.image.decode(buffer)`

```
os.image.decode(buffer: ArrayBuffer) → { width: number, height: number, data: ArrayBuffer } | null
```

- `buffer` — raw JPEG or PNG file bytes from any source.
- Returns an object with:
  - `width`, `height` — image dimensions in pixels.
  - `data` — `ArrayBuffer` of `width × height × 4` bytes. Each 4-byte group is one pixel in `0x00RRGGBB` format, matching `os.window.getPixelBuffer` exactly.
- Returns `null` on failure (corrupt data, unrecognized format, OOM).
- The returned `data` buffer is a fresh allocation owned by QuickJS's GC; it is freed automatically when the JS value is collected.

### `os.store.getBytes(key)`

```
os.store.getBytes(key: string) → ArrayBuffer
```

Returns the raw bytes stored under `key` as an `ArrayBuffer`. Counterpart to `os.store.get` which returns a string.

### `os.base64.decodeBytes(s)`

```
os.base64.decodeBytes(s: string) → ArrayBuffer
```

Decodes a Base64 string and returns the raw bytes as an `ArrayBuffer`. Counterpart to `os.base64.decode` which returns a UTF-8 string.

---

## Data Flow

```
JS ArrayBuffer (input)
  → JS_GetArrayBuffer → Rust &[u8] (zero-copy view)
  → image::decode()   → Vec<u32> pixels (new heap allocation)
  → JS_NewArrayBuffer → ArrayBuffer wrapping Vec memory (no copy)
  → JS object { width, height, data }
  → returned to JS caller
```

On GC of `data`: the ArrayBuffer free callback receives the raw pixel pointer and the pixel count (stored in the `opaque` slot as a `usize`). It reconstructs and drops the `Vec<u32>` to free the memory.

---

## Demo App — `imgview.jsos`

A built-in app that demonstrates end-to-end image decoding:

1. Accepts a URL (hardcoded or via IPC).
2. Fetches image bytes with `os.fetch`.
3. Calls `os.image.decode`.
4. Creates a window sized to the image.
5. Copies decoded pixels into the pixel buffer and calls `os.window.flush`.

---

## New API Surface Summary

| API | Input | Output |
|-----|-------|--------|
| `os.image.decode(ab)` | `ArrayBuffer` (JPEG or PNG bytes) | `{width, height, data: ArrayBuffer}` or `null` |
| `os.store.getBytes(key)` | `string` | `ArrayBuffer` |
| `os.base64.decodeBytes(s)` | `string` | `ArrayBuffer` |

---

## Out of Scope

- Scale-to-fit / resize — can be layered on later: `os.image.decode(ab, maxW, maxH)`
- Encoding / saving images
- Animated GIF / WebP
- Alpha compositing (alpha channel is discarded; output is always opaque `0x00RRGGBB`)

---

## Fallback Plan

If `jpeg-decoder` or `zune-png` fail to compile freestanding, integrate `stb_image.h`:
- Drop `stb_image.h` into `quickjs/`.
- Add a one-line `stb_image_impl.c` with `#define STB_IMAGE_IMPLEMENTATION`.
- Add to `build.rs` file list.
- Replace Rust crate calls in `src/image.rs` with `extern "C"` FFI to `stbi_load_from_memory`.
- No JS API changes needed.
