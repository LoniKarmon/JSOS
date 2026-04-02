// TextEncoder: encodes JS string to UTF-8 Uint8Array
globalThis.TextEncoder = function() { this.encoding = 'utf-8'; };
globalThis.TextEncoder.prototype.encode = function(str) {
    const bytes = [];
    for (let i = 0; i < str.length; i++) {
        const c = str.charCodeAt(i);
        if (c < 0x80) {
            bytes.push(c);
        } else if (c < 0x800) {
            bytes.push(0xC0 | (c >> 6), 0x80 | (c & 0x3F));
        } else if (c < 0xD800 || c >= 0xE000) {
            bytes.push(0xE0 | (c >> 12), 0x80 | ((c >> 6) & 0x3F), 0x80 | (c & 0x3F));
        } else {
            // Surrogate pair → encode as U+FFFD replacement
            bytes.push(0xEF, 0xBF, 0xBD);
            i++; // skip low surrogate
        }
    }
    return new Uint8Array(bytes);
};

// TextDecoder: decodes UTF-8 bytes to JS string
globalThis.TextDecoder = function(encoding) { this.encoding = encoding || 'utf-8'; };
globalThis.TextDecoder.prototype.decode = function(input) {
    const arr = (input instanceof Uint8Array) ? input : new Uint8Array(input);
    let s = '';
    let i = 0;
    while (i < arr.length) {
        const b = arr[i++];
        if (b < 0x80) {
            s += String.fromCharCode(b);
        } else if ((b & 0xE0) === 0xC0) {
            s += String.fromCharCode(((b & 0x1F) << 6) | (arr[i++] & 0x3F));
        } else if ((b & 0xF0) === 0xE0) {
            const b2 = arr[i++], b3 = arr[i++];
            s += String.fromCharCode(((b & 0x0F) << 12) | ((b2 & 0x3F) << 6) | (b3 & 0x3F));
        } else {
            // 4-byte sequence — emit replacement char
            i += 3;
            s += '\uFFFD';
        }
    }
    return s;
};

// crypto: minimal polyfill (Math.random-based, not cryptographically secure)
globalThis.crypto = {
    getRandomValues: function(arr) {
        for (let i = 0; i < arr.length; i++) {
            arr[i] = Math.floor(Math.random() * 256);
        }
        return arr;
    },
    randomUUID: function() {
        return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function(c) {
            const r = Math.floor(Math.random() * 16);
            return (c === 'x' ? r : (r & 0x3 | 0x8)).toString(16);
        });
    }
};
