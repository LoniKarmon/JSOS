console.dir = function(obj) {
    console.log(JSON.stringify(obj, null, 2));
};
console.table = function(data) {
    console.log(JSON.stringify(data, null, 2));
};
console.assert = function(condition, msg) {
    if (!condition) console.error('Assertion failed: ' + (msg !== undefined ? msg : ''));
};
console.group = function(label) {
    if (label !== undefined) console.log('> ' + label);
};
console.groupEnd = function() {};
(function() {
    const _times = {};
    console.time = function(label) {
        _times[label || 'default'] = Date.now();
    };
    console.timeEnd = function(label) {
        const key = label || 'default';
        const start = _times[key];
        if (start !== undefined) {
            console.log(key + ': ' + (Date.now() - start) + 'ms');
            delete _times[key];
        }
    };
})();
console.count = (function() {
    const counts = {};
    return function(label) {
        const key = label || 'default';
        counts[key] = (counts[key] || 0) + 1;
        console.log(key + ': ' + counts[key]);
    };
})();

// URLSearchParams
globalThis.URLSearchParams = function(init) {
    this._map = {};
    if (typeof init === 'string') {
        const s = init.charAt(0) === '?' ? init.slice(1) : init;
        if (s) {
            s.split('&').forEach(function(pair) {
                const eq = pair.indexOf('=');
                const k = decodeURIComponent(eq >= 0 ? pair.slice(0, eq) : pair);
                const v = eq >= 0 ? decodeURIComponent(pair.slice(eq + 1)) : '';
                if (k) this._map[k] = v;
            }, this);
        }
    } else if (init && typeof init === 'object') {
        Object.keys(init).forEach(function(k) { this._map[k] = String(init[k]); }, this);
    }
};
globalThis.URLSearchParams.prototype.get = function(k) {
    return this._map.hasOwnProperty(k) ? this._map[k] : null;
};
globalThis.URLSearchParams.prototype.set = function(k, v) { this._map[k] = String(v); };
globalThis.URLSearchParams.prototype.has = function(k) { return this._map.hasOwnProperty(k); };
globalThis.URLSearchParams.prototype.delete = function(k) { delete this._map[k]; };
globalThis.URLSearchParams.prototype.toString = function() {
    return Object.keys(this._map).map(function(k) {
        return encodeURIComponent(k) + '=' + encodeURIComponent(this._map[k]);
    }, this).join('&');
};
globalThis.URLSearchParams.prototype.forEach = function(fn) {
    Object.keys(this._map).forEach(function(k) { fn(this._map[k], k, this); }, this);
};

// URL
globalThis.URL = function(url, base) {
    let full = url;
    if (base && !/^[a-z]+:\/\//i.test(url)) {
        const baseHref = typeof base === 'string' ? base : base.href;
        full = baseHref.replace(/\/[^\/]*$/, '/') + url;
    }
    const m = full.match(/^([a-z][a-z0-9+\-.]*):\/\/([^\/\?#]*)([^\?#]*)(\?[^#]*)?(#.*)?$/i);
    if (!m) { this.href = full; this.protocol = ''; this.host = ''; this.pathname = full; this.search = ''; this.hash = ''; }
    else {
        this.protocol = m[1] + ':';
        this.host = m[2] || '';
        this.pathname = m[3] || '/';
        this.search = m[4] || '';
        this.hash = m[5] || '';
        this.href = full;
    }
    this.hostname = this.host.replace(/:\d+$/, '');
    this.port = (this.host.match(/:(\d+)$/) || [])[1] || '';
    this.origin = this.protocol + '//' + this.host;
    this.searchParams = new URLSearchParams(this.search);
};
globalThis.URL.prototype.toString = function() { return this.href; };
