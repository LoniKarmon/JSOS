// Polls for messages and state changes every 50ms via setInterval.
globalThis.WebSocket = function(url) {
    this.url = url;
    this.readyState = 0;
    this.onopen = null;
    this.onmessage = null;
    this.onclose = null;
    this.onerror = null;
    this._id = os.websocket.connect(url);
    const self = this;
    const poll = setInterval(function() {
        try {
            const state = os.websocket.state(self._id);
            if (state === 1 && self.readyState === 0) {
                self.readyState = 1;
                if (self.onopen) self.onopen({ type: 'open' });
            }
            if (self.readyState === 1) {
                let msg;
                while ((msg = os.websocket.recv(self._id)) !== null && msg !== undefined) {
                    if (self.onmessage) self.onmessage({ type: 'message', data: msg });
                }
            }
            if (state === 3 && self.readyState !== 3) {
                self.readyState = 3;
                clearInterval(poll);
                if (self.onclose) self.onclose({ type: 'close', code: 1000, reason: '' });
            }
        } catch(e) {
            if (self.onerror) self.onerror({ type: 'error', message: String(e) });
        }
    }, 50);
    this._poll = poll;
};
globalThis.WebSocket.prototype.send = function(data) {
    if (this.readyState !== 1) throw new Error('WebSocket is not open');
    os.websocket.send(this._id, data);
};
globalThis.WebSocket.prototype.close = function(code, reason) {
    if (this.readyState === 3) return;
    this.readyState = 2;
    os.websocket.close(this._id);
};
globalThis.WebSocket.prototype.addEventListener = function(type, fn) {
    if (type === 'open') this.onopen = fn;
    else if (type === 'message') this.onmessage = fn;
    else if (type === 'close') this.onclose = fn;
    else if (type === 'error') this.onerror = fn;
};
globalThis.WebSocket.CONNECTING = 0;
globalThis.WebSocket.OPEN = 1;
globalThis.WebSocket.CLOSING = 2;
globalThis.WebSocket.CLOSED = 3;
