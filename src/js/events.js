class EventEmitter {
    constructor() {
        this._events = {};
    }
    on(event, listener) {
        if (!this._events[event]) this._events[event] = [];
        this._events[event].push(listener);
        return this;
    }
    once(event, listener) {
        const wrapped = (...args) => {
            this.removeListener(event, wrapped);
            listener(...args);
        };
        return this.on(event, wrapped);
    }
    emit(event, ...args) {
        if (!this._events[event]) return false;
        const listeners = [...this._events[event]];
        for (const listener of listeners) {
            listener(...args);
        }
        return true;
    }
    removeListener(event, listener) {
        if (!this._events[event]) return this;
        this._events[event] = this._events[event].filter(l => l !== listener);
        return this;
    }
    off(event, listener) {
        return this.removeListener(event, listener);
    }
}
module.exports = EventEmitter;
