class Buffer extends Uint8Array {
    static from(data, encoding) {
        if (typeof data === 'string') {
            const arr = new Uint8Array(data.length);
            for (let i = 0; i < data.length; i++) {
                arr[i] = data.charCodeAt(i) & 0xFF; // basic ASCII
            }
            const buf = new Buffer(arr.buffer, arr.byteOffset, arr.length);
            return buf;
        } else if (Array.isArray(data) || data instanceof Uint8Array) {
            const buf = new Buffer(data.length);
            buf.set(data);
            return buf;
        }
        return new Buffer(0);
    }
    static alloc(size) {
        return new Buffer(size);
    }
    toString(encoding) {
        let str = '';
        for (let i = 0; i < this.length; i++) {
            str += String.fromCharCode(this[i]);
        }
        return str;
    }
    slice(start, end) {
        const sliced = super.slice(start, end);
        return new Buffer(sliced.buffer, sliced.byteOffset, sliced.length);
    }
}
module.exports = Buffer;
module.exports.Buffer = Buffer;
