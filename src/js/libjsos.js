// src/js/libjsos.js — JSOS Userland Standard Library
// Load via: const { Window, Theme, Keys, Store } = require('libjsos.js');

const Keys = {
    ENTER:      13,
    NEWLINE:    10,
    BACKSPACE:  8,
    DELETE:     0x7F,
    ESCAPE:     27,
    TAB:        9,
    SPACE:      32,
    LEFT:   0x02,
    RIGHT:  0x06,
    UP:     0x10,
    DOWN:   0x0E,
    PAGE_UP:   0x1B,
    PAGE_DOWN: 0x1C,
    CTRL_A: 0x01,
    CTRL_C: 0x03,
    CTRL_E: 0x05,
    CTRL_K: 0x0B,
    CTRL_LEFT:  0x1D,
    CTRL_RIGHT: 0x1E,
    CTRL_Q: 17,
    CTRL_S: 19,
    CTRL_U: 0x15,
    CTRL_W: 0x17,
    isQuit:      (c) => c === 17 || c === 27,  // Ctrl+Q or ESC
    isEnter:     (c) => c === 13 || c === 10,
    isBackspace: (c) => c === 8,
    isDelete:    (c) => c === 0x7F,
    isPrintable: (c) => c >= 32 && c <= 126,
    toChar:      (c) => String.fromCharCode(c),
};

const Theme = {
    dark() {
        return {
            bg:      [10,  10,  20 ],
            surface: [20,  22,  35 ],
            text:    [200, 220, 255],
            dim:     [100, 110, 140],
            accent:  [70,  140, 255],
            success: [80,  220, 120],
            warning: [255, 159, 10 ],
            error:   [220, 70,  70 ],
            white:   [255, 255, 255],
            black:   [0,   0,   0  ],
        };
    },
};

function Window(x, y, w, h) {
    this.id = os.window.create(x, y, w, h);
    this.w  = w;
    this.h  = h;
    this._timers = {};
    this._nextTimerId = 1;

    const self = this;
    globalThis.__fireTimer = function(id) {
        const entry = self._timers[id];
        if (!entry) return;
        entry.fn();
        if (self._timers[id]) {
            os._setTimeout(String(globalThis.__PID), id, String(entry.interval));
        }
    };
}

Window.prototype.clear = function(r, g, b) {
    os.window.drawRect(this.id, 0, 0, this.w, this.h, r||0, g||0, b||0);
};
Window.prototype.rect = function(x, y, w, h, r, g, b) {
    os.window.drawRect(this.id, x, y, w, h, r, g, b);
};
Window.prototype.text = function(x, y, str, r, g, b, size) {
    os.window.drawString(this.id, str, x, y, r, g, b, size);
};
Window.prototype.line = function(x0, y0, x1, y1, r, g, b) {
    os.window.drawLine(this.id, x0, y0, x1, y1, r, g, b);
};
Window.prototype.circle = function(cx, cy, rad, r, g, b) {
    os.window.drawCircle(this.id, cx, cy, rad, r, g, b);
};
Window.prototype.fillCircle = function(cx, cy, rad, r, g, b) {
    os.window.fillCircle(this.id, cx, cy, rad, r, g, b);
};
Window.prototype.flush = function() {
    os.window.flush(this.id);
};
Window.prototype.pixels = function() {
    return new Uint32Array(os.window.getPixelBuffer(this.id));
};
Window.prototype.getContext = function() {
    return os.window.getContext(this.id);
};
Window.prototype.setInterval = function(ms, fn) {
    const id = String(this._nextTimerId++);
    this._timers[id] = { fn, interval: ms };
    os._setTimeout(String(globalThis.__PID), id, String(ms));
    return id;
};
Window.prototype.clearInterval = function(id) {
    delete this._timers[id];
};
Window.prototype.installIpc = function(handlers) {
    // Wire on_key so foreground key events (direct kernel dispatch) also work.
    if (handlers.key) globalThis.on_key = handlers.key;
    globalThis.on_ipc = function(msg) {
        try {
            const p = JSON.parse(msg);
            if      (p.type === 'KEY'    && handlers.key)    handlers.key(p.charCode);
            else if (p.type === 'RENDER' && handlers.render) handlers.render();
            else if (p.type === 'MOUSE'  && handlers.mouse)  handlers.mouse(p.x, p.y, p.buttons);
            else if (p.type === 'SCROLL' && handlers.scroll) handlers.scroll(p.delta);
        } catch(e) {}
        if (handlers.raw) handlers.raw(msg);
    };
};

const Store = {
    get:      (key)      => os.store.get(key),
    set:      (key, val) => os.store.set(key, val),
    remove:   (key)      => os.store.delete(key),
    list:     ()         => { try { return JSON.parse(os.store.list()); } catch(e) { return []; } },
    getJSON:  (key, def) => { try { const v = os.store.get(key); return v ? JSON.parse(v) : def; } catch(e) { return def; } },
    setJSON:  (key, val) => os.store.set(key, JSON.stringify(val)),
    isFocused: ()        => os.store.get('focus_pid') === String(globalThis.__PID),
};

module.exports = { Window, Theme, Keys, Store };
