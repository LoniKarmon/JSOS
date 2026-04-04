globalThis.fetch = os.fetch;

globalThis.atob = function(s) { return os.base64.decode(s); };
globalThis.btoa = function(s) { return os.base64.encode(s); };

// JSON round-trip: no circular refs or non-JSON types
globalThis.structuredClone = function(obj) { return JSON.parse(JSON.stringify(obj)); };

globalThis.queueMicrotask = function(fn) { setTimeout(fn, 0); };

globalThis.performance = { now: function() { return os.uptime() * 1000; } };

globalThis.navigator = { userAgent: 'JSOS/1.0', platform: 'JSOS', language: 'en' };

globalThis.requestAnimationFrame = function(fn) {
    return setTimeout(fn, 16);
};

globalThis.cancelAnimationFrame = function(id) {
    clearTimeout(id);
};

globalThis.AbortController = function() {
    this.signal = { aborted: false };
};
globalThis.AbortController.prototype.abort = function() {
    this.signal.aborted = true;
};

globalThis.AbortSignal = {
    timeout: function(ms) {
        var ctrl = new AbortController();
        setTimeout(function() { ctrl.abort(); }, ms);
        return ctrl.signal;
    }
};
