globalThis.fetch = os.fetch;

globalThis.atob = function(s) { return os.base64.decode(s); };
globalThis.btoa = function(s) { return os.base64.encode(s); };

// JSON round-trip: no circular refs or non-JSON types
globalThis.structuredClone = function(obj) { return JSON.parse(JSON.stringify(obj)); };

globalThis.queueMicrotask = function(fn) { setTimeout(fn, 0); };

globalThis.performance = { now: function() { return os.uptime() * 1000; } };

globalThis.navigator = { userAgent: 'JSOS/1.0', platform: 'JSOS', language: 'en' };

if (typeof process !== 'undefined') {
    if (!process.argv) process.argv = ['jsos', String(typeof __PID !== 'undefined' ? __PID : 0)];
    if (!process.exit) process.exit = function() { os.exit(); };
    if (!process.platform) process.platform = 'jsos';
    if (!process.version) process.version = 'v18.0.0';
    if (!process.versions) process.versions = { node: '18.0.0' };
}
