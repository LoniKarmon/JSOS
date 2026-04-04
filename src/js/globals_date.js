(function() {
    function _rtc() {
        try { return JSON.parse(os.rtc()); } catch(e) { return {year:2000,month:1,day:1,hour:0,minute:0,second:0}; }
    }
    function _pad(n, w) { return String(n).padStart(w || 2, '0'); }

    function Date(year, month, day, hours, minutes, seconds, ms) {
        if (!(this instanceof Date)) return new Date().toString();
        if (arguments.length === 0) {
            const r = _rtc();
            this._y = r.year; this._mo = r.month - 1; this._d = r.day;
            this._h = r.hour; this._min = r.minute; this._s = r.second; this._ms = 0;
        } else if (arguments.length === 1) {
            if (typeof year === 'string') {
                const m = year.match(/^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2}):(\d{2}))?/);
                if (m) {
                    this._y = +m[1]; this._mo = +m[2]-1; this._d = +m[3];
                    this._h = +(m[4]||0); this._min = +(m[5]||0); this._s = +(m[6]||0); this._ms = 0;
                } else {
                    this._y = 2000; this._mo = 0; this._d = 1;
                    this._h = 0; this._min = 0; this._s = 0; this._ms = 0;
                }
            } else {
                const totalMs = +year;
                this._ms = totalMs % 1000;
                let sec = Math.floor(totalMs / 1000);
                this._s = sec % 60; sec = Math.floor(sec / 60);
                this._min = sec % 60; sec = Math.floor(sec / 60);
                this._h = sec % 24;
                this._d = 1; this._mo = 0; this._y = 2000;
            }
        } else {
            this._y = +year; this._mo = +(month||0); this._d = +(day||1);
            this._h = +(hours||0); this._min = +(minutes||0); this._s = +(seconds||0); this._ms = +(ms||0);
        }
    }
    Date.now = function() { return Math.floor(os.uptime() * 1000); };
    Date.parse = function() { return 0; };
    Date.prototype.getFullYear    = function() { return this._y; };
    Date.prototype.getMonth       = function() { return this._mo; };
    Date.prototype.getDate        = function() { return this._d; };
    Date.prototype.getDay         = function() { return 0; }; // stub (day of week)
    Date.prototype.getHours       = function() { return this._h; };
    Date.prototype.getMinutes     = function() { return this._min; };
    Date.prototype.getSeconds     = function() { return this._s; };
    Date.prototype.getMilliseconds = function() { return this._ms; };
    Date.prototype.getTime        = function() {
        return (this._h * 3600 + this._min * 60 + this._s) * 1000 + this._ms;
    };
    Date.prototype.setFullYear  = function(y) { this._y = y; return this.getTime(); };
    Date.prototype.setMonth     = function(m) { this._mo = m; return this.getTime(); };
    Date.prototype.setDate      = function(d) { this._d = d; return this.getTime(); };
    Date.prototype.setHours     = function(h) { this._h = h; return this.getTime(); };
    Date.prototype.setMinutes   = function(m) { this._min = m; return this.getTime(); };
    Date.prototype.setSeconds   = function(s) { this._s = s; return this.getTime(); };
    Date.prototype.toISOString  = function() {
        return _pad(this._y,4)+'-'+_pad(this._mo+1)+'-'+_pad(this._d)+'T'+
               _pad(this._h)+':'+_pad(this._min)+':'+_pad(this._s)+'.'+_pad(this._ms,3)+'Z';
    };
    Date.prototype.toString = function() {
        return _pad(this._y,4)+'-'+_pad(this._mo+1)+'-'+_pad(this._d)+' '+
               _pad(this._h)+':'+_pad(this._min)+':'+_pad(this._s);
    };
    Date.prototype.toLocaleDateString = function() {
        return _pad(this._d)+'/'+_pad(this._mo+1)+'/'+_pad(this._y,4);
    };
    Date.prototype.toLocaleTimeString = function() {
        return _pad(this._h)+':'+_pad(this._min)+':'+_pad(this._s);
    };
    Date.prototype.valueOf = function() { return this.getTime(); };
    Date.prototype[Symbol.toPrimitive] = function(hint) {
        return hint === 'number' ? this.getTime() : this.toString();
    };
    globalThis.Date = Date;
})();
