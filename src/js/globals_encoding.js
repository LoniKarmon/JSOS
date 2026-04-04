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

// crypto: hardware-backed via os.randomBytes() (RDRAND)
globalThis.crypto = {
    getRandomValues: function(arr) {
        const bytes = new Uint8Array(os.randomBytes(arr.length));
        for (let i = 0; i < arr.length; i++) arr[i] = bytes[i];
        return arr;
    },
    randomUUID: function() {
        const bytes = new Uint8Array(os.randomBytes(16));
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        const hex = Array.from(bytes).map(b => b.toString(16).padStart(2, '0'));
        return `${hex.slice(0,4).join('')}-${hex.slice(4,6).join('')}-${hex.slice(6,8).join('')}-${hex.slice(8,10).join('')}-${hex.slice(10,16).join('')}`;
    }
};
