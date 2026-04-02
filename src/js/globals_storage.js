// 'ls:' prefix avoids collisions with other JSKV store consumers
const _LS_PREFIX = 'ls:';

globalThis.localStorage = {
    getItem: function(k) {
        const v = os.store.get(_LS_PREFIX + k);
        return (v === undefined || v === null) ? null : v;
    },
    setItem: function(k, v) { os.store.set(_LS_PREFIX + k, String(v)); },
    removeItem: function(k) { os.store.delete(_LS_PREFIX + k); },
    clear: function() {
        const keys = os.store.list().filter(function(k) { return k.indexOf(_LS_PREFIX) === 0; });
        keys.forEach(function(k) { os.store.delete(k); });
    },
    key: function(i) {
        const keys = os.store.list().filter(function(k) { return k.indexOf(_LS_PREFIX) === 0; });
        return i < keys.length ? keys[i].slice(_LS_PREFIX.length) : null;
    },
    get length() {
        return os.store.list().filter(function(k) { return k.indexOf(_LS_PREFIX) === 0; }).length;
    }
};

(function() {
    const _store = {};
    globalThis.sessionStorage = {
        getItem: function(k) { return _store.hasOwnProperty(k) ? _store[k] : null; },
        setItem: function(k, v) { _store[k] = String(v); },
        removeItem: function(k) { delete _store[k]; },
        clear: function() { Object.keys(_store).forEach(function(k) { delete _store[k]; }); },
        key: function(i) { return Object.keys(_store)[i] || null; },
        get length() { return Object.keys(_store).length; }
    };
})();
