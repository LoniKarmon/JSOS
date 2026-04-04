# JSOS Native API Documentation

This document describes all the native functions available to JavaScript applications running inside JSOS (via the QuickJS-NG engine).
All these APIs are registered under the global `os` namespace.

## Top-Level APIs

| Function | Signature | Description |
|---|---|---|
| `os.log` | `os.log(msg: string): void` | Prints a message to the kernel serial output. |
| `os.spawn` | `os.spawn(binName: string): number` | Spawns a new process from a given binary name (e.g., `"shell.jsos"`) and returns its PID. |
| `os.processes` | `os.processes(): string` | Returns a JSON string of running processes. |
| `os.sendIpc` | `os.sendIpc(pid: number, msg: string): void` | Sends an IPC string message to the specified process. |
| `os.awaken` | `os.awaken(pid: number): void` | Brings the specified process to the foreground (shifts focus). |
| `os._setTimeout`| `os._setTimeout(pid: string, timerId: string, msDelay: string): void` | Low-level timer registration. Do not use directly; use standard `setTimeout`. |
| `os.uptime` | `os.uptime(): number` | Returns the system uptime in seconds. |
| `os.clear` | `os.clear(): void` | Clears the entire framebuffer. |
| `os.reboot` | `os.reboot(): void` | Reboots the system. |
| `os.shutdown` | `os.shutdown(): void` | Shuts down the system via ACPI. |
| `os.fetchNative`| `os.fetchNative(url: string, method: string, body: string, headers: string, alpnJson?: string): void` | Low-level fetch. Do not use directly; use the standard `os.fetch(url, options)` promise wrapper. |
| `os.exit` | `os.exit(pid?: number): void` | Terminates the calling process (or specified PID) and cleans up its resources. |
| `os.exec` | `os.exec(key: string): void` | Executes a script stored in the KV store. |
| `os.listBin` | `os.listBin(): string` | Returns a JSON string array of all registered application binaries. |
| `os.sysinfo` | `os.sysinfo(): string` | Returns a JSON string containing CPU and memory hardware stats. |
| `os.rtc` | `os.rtc(): string` | Returns a JSON string with the current RTC date and time. |
| `os.screen` | `os.screen(): string` | Returns a JSON string with screen dimensions (`{"width": 1920, "height": 1080}`). |
| `os.notify` | `os.notify(msg: string): void` | Shows a blue info toast overlay for ~3 seconds. Stacks with crash notifications (max 4 visible). |

## Sub-namespaces

### `os.graphics`
Direct framebuffer manipulation. *Note: Usually you should use `os.window` instead for GUI applications.*
- `os.graphics.fillRect(x, y, w, h, r, g, b): void` — Draws a filled rectangle.
- `os.graphics.drawString(text, x, y, r, g, b, size?): void` — Draws text at position (x, y). Optional `size`: `os.FONT_SMALL` (default, 8×16 px) or `os.FONT_LARGE` (10×20 px).
- `os.graphics.clear(): void` — Clears the screen.
- `os.graphics.screenshot(): string` — Takes a screenshot and returns a hex string of the PPM image.

### `os.fetch`
High-level HTTP/HTTPS fetch API returning a Promise.

```js
os.fetch(url, options)
```

**Options object:**

| Field | Type | Description |
|---|---|---|
| `method` | `string` | HTTP method. Defaults to `"GET"`. |
| `body` | `string` | Request body. Defaults to `""`. |
| `headers` | `object` | HTTP headers as a key-value object. |
| `alpn` | `string[]` | Optional. ALPN protocol list for TLS negotiation. Defaults to `["http/1.1"]`. Example: `["h2", "http/1.1"]`. |

**Example:**
```js
const text = await os.fetch("https://example.com/api", {
    method: "POST",
    body: JSON.stringify({ key: "value" }),
    headers: { "Content-Type": "application/json" },
    alpn: ["h2", "http/1.1"],
});
```

### `os.net`
Networking operations.
- `os.net.listen(port: number): void` — Listens for incoming TCP connections on the specified port.
- `os.net.config(): string` — Returns the current network configuration (MAC and IPs) as a multi-line string.
- `os.net.serveStatic(html: string): void` — Registers a static HTML string to be served on port 80.

### `os.store`
Persistent Key-Value store.
- `os.store.set(key: string, value: string): void`
- `os.store.get(key: string): string`
- `os.store.list(): string` — Returns a JSON array of all stored keys.

### Font size constants
- `os.FONT_SMALL` (`0`) — Default 8×16 px bitmap font (`u8g2_font_unifont_t_hebrew`). ~8 px per character.
- `os.FONT_LARGE` (`1`) — Large 10×20 px bitmap font (`u8g2_font_10x20_tf`). ~10 px per character.

Pass these as the optional last argument to any `drawString` call.

### `os.window`
Window management and drawing APIs.
- `os.window.create(x, y, w, h): number` — Creates a window and returns its internal ID.
- `os.window.drawRect(winId, x, y, w, h, r, g, b): void` — Draws a filled rectangle relative to the window.
- `os.window.drawString(winId, text, x, y, r, g, b, size?): void` — Draws text relative to the window. Optional `size`: `os.FONT_SMALL` (default) or `os.FONT_LARGE`.
- `os.window.drawLine(winId, x0, y0, x1, y1, r, g, b): void` — Draws a line from (x0,y0) to (x1,y1) relative to the window using Bresenham's algorithm.
- `os.window.drawCircle(winId, cx, cy, radius, r, g, b): void` — Draws a circle outline relative to the window using the midpoint algorithm.
- `os.window.fillCircle(winId, cx, cy, radius, r, g, b): void` — Draws a filled circle relative to the window.
- `os.window.flush(winId): void` — Blits the window's pixel buffer to the screen. Call after writing to the buffer returned by `getPixelBuffer()`, or after any draw calls to display the frame.
- `os.window.getPixelBuffer(winId): ArrayBuffer` — Returns the window's raw pixel buffer as an `ArrayBuffer` of `width × height × 4` bytes. Wrap it as `new Uint32Array(os.window.getPixelBuffer(winId))` to get a `Uint32Array` where each element is `0x00RRGGBB`. Writes to this array are reflected on the next `flush(winId)` call. The buffer shares memory directly with the kernel — no copy is made.
- `os.window.setCursor(x, y): void` — Manually sets the hardware mouse cursor position.
- `os.window.list(): string` — Returns a JSON string of all active windows.
- `os.window.move(winId, x, y): void` — Moves the window to a new absolute position.

### `os.window.getContext`

`os.window.getContext(winId)` → Canvas context object

Returns a stateful Canvas 2D context for the given window. All drawing targets the window's pixel buffer; call `ctx.flush()` to blit to the framebuffer.

**Properties** (readable and writable):
- `ctx.fillStyle` — CSS color string (default `'#000000'`). Accepts `#rgb`, `#rrggbb`, `rgb(r,g,b)`, named colors.
- `ctx.strokeStyle` — CSS color string (default `'#000000'`).
- `ctx.lineWidth` — stroke width in pixels (default `1`).
- `ctx.font` — font spec string (default `'16px monospace'`). Size < 16 → 8×16 font; ≥ 16 → 10×20 font.

**Methods:**

| Method | Description |
|---|---|
| `fillRect(x,y,w,h)` | Fill rectangle with `fillStyle` |
| `strokeRect(x,y,w,h)` | Stroke rectangle with `strokeStyle` + `lineWidth` |
| `clearRect(x,y,w,h)` | Fill with black (0x000000) |
| `beginPath()` | Clear the current path |
| `moveTo(x,y)` | Move to point without drawing |
| `lineTo(x,y)` | Add line segment to path |
| `arc(cx,cy,r,start,end[,ccw])` | Add arc to path (angles in radians) |
| `bezierCurveTo(cp1x,cp1y,cp2x,cp2y,x,y)` | Add cubic Bézier curve |
| `quadraticCurveTo(cpx,cpy,x,y)` | Add quadratic Bézier curve |
| `rect(x,y,w,h)` | Add rectangle subpath |
| `closePath()` | Close current subpath |
| `fill()` | Fill path with `fillStyle` (even-odd rule) |
| `stroke()` | Stroke path with `strokeStyle` + `lineWidth` |
| `fillText(text,x,y)` | Draw filled text |
| `strokeText(text,x,y)` | Same as `fillText` |
| `drawImage(img,dx,dy)` | Blit image (from `os.image.decode`) at original size |
| `drawImage(img,dx,dy,dw,dh)` | Blit image scaled to dw×dh |
| `drawImage(img,sx,sy,sw,sh,dx,dy,dw,dh)` | Crop source then scale |
| `getImageData(x,y,w,h)` | Returns `{width,height,data:ArrayBuffer}` (RGBA bytes) |
| `putImageData(imgData,dx,dy)` | Write RGBA ImageData to window buffer |
| `save()` | Push fillStyle, strokeStyle, lineWidth, font, transform |
| `restore()` | Pop and restore saved state |
| `translate(x,y)` | Apply translation to current transform |
| `rotate(angle)` | Apply rotation (radians) |
| `scale(x,y)` | Apply scaling |
| `setTransform(a,b,c,d,e,f)` | Set transform matrix directly |
| `resetTransform()` | Reset to identity |
| `flush()` | Blit pixel buffer to framebuffer |

### `os.mouse`
- `os.mouse.scroll(): number` — Returns accumulated mouse wheel scroll delta and resets it.

### `os.clipboard`
- `os.clipboard.write(text: string): void` — Copies text to the system clipboard.
- `os.clipboard.read(): string` — Returns the current clipboard string.

### `os.base64`
- `os.base64.encode(text: string): string`
- `os.base64.decode(b64: string): string`

### `os.websocket`
Low-level websocket APIs.
- `os.websocket.connect(url: string): number`
- `os.websocket.send(id: number, text: string): void`
- `os.websocket.recv(id: number): string`
- `os.websocket.close(id: number): void`
- `os.websocket.state(id: number): number`

### `console`
Standard console methods are piped into `os.log` for serial output.
- `console.log(msg)`
- `console.warn(msg)`
- `console.error(msg)`
- `console.info(msg)`

## Browser Compatibility Globals

These standard browser globals are injected into every sandbox at startup via `globals_compat.js`.

| Global | Signature | Description |
|---|---|---|
| `fetch` | `fetch(url: string, options?): Promise<string>` | Alias for `os.fetch`. Returns a Promise resolving to the response body. |
| `atob` | `atob(s: string): string` | Base64 decode. Wraps `os.base64.decode`. |
| `btoa` | `btoa(s: string): string` | Base64 encode. Wraps `os.base64.encode`. |
| `structuredClone` | `structuredClone(obj: any): any` | Deep clone via JSON round-trip (`JSON.parse(JSON.stringify(obj))`). |
| `queueMicrotask` | `queueMicrotask(fn: Function): void` | Schedules `fn` to run after the current task via `setTimeout(fn, 0)`. |
| `performance.now` | `performance.now(): number` | Returns milliseconds since boot (`os.uptime() * 1000`). |
| `navigator.userAgent` | — | `"JSOS/1.0"` |
| `navigator.platform` | — | `"JSOS"` |
| `navigator.language` | — | `"en"` |
