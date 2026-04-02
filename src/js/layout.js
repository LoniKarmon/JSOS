// layout.js – Block layout engine + box model for JSOS Browser Engine
'use strict';

// ── Font metrics (fixed monospace bitmap font) ────────────────────────────────
var CHAR_W  = 8;   // px per character
var CHAR_H  = 16;  // px per line

// ── Unit parsing ──────────────────────────────────────────────────────────────

function parsePx(val, containerSize) {
    if (!val) return 0;
    var s = String(val).trim();
    if (s === '0') return 0;
    if (s.endsWith('%')) return Math.round(parseFloat(s) * (containerSize || 0) / 100);
    if (s.endsWith('px')) return Math.round(parseFloat(s));
    var n = parseFloat(s);
    return isNaN(n) ? 0 : Math.round(n);
}

// ── Colour utilities ──────────────────────────────────────────────────────────

var NAMED_COLORS = {
    'black':'#000000','white':'#ffffff','red':'#ff0000','green':'#00cc00',
    'blue':'#0000ff','yellow':'#ffff00','cyan':'#00ffff','magenta':'#ff00ff',
    'gray':'#808080','grey':'#808080','orange':'#ffa500','purple':'#800080',
    'lime':'#00ff00','navy':'#000080','teal':'#008080','silver':'#c0c0c0',
    'transparent': null
};

function parseColor(val) {
    if (!val) return null;
    val = String(val).trim();
    var named = NAMED_COLORS[val.toLowerCase()];
    if (named !== undefined) return named;
    if (val[0] === '#') return val;
    // rgb(r,g,b)
    var m = val.match(/rgb\s*\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)/i);
    if (m) {
        return '#' + [m[1], m[2], m[3]].map(function(n) {
            return parseInt(n).toString(16).padStart(2, '0');
        }).join('');
    }
    return val;
}

function hexToRgb(hex) {
    if (!hex || hex[0] !== '#') return [200, 200, 200];
    var s = hex.slice(1);
    if (s.length === 3) s = s[0]+s[0]+s[1]+s[1]+s[2]+s[2];
    var n = parseInt(s, 16);
    return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

// ── Text wrapping ─────────────────────────────────────────────────────────────

function wrapText(text, maxW) {
    var maxChars = Math.max(1, Math.floor(maxW / CHAR_W));
    var words = String(text).split(' ');
    var lines = [];
    var cur   = '';
    for (var i = 0; i < words.length; i++) {
        var word = words[i];
        var candidate = cur ? cur + ' ' + word : word;
        if (candidate.length <= maxChars) {
            cur = candidate;
        } else {
            if (cur) lines.push(cur);
            // Word longer than line? Hard break.
            while (word.length > maxChars) {
                lines.push(word.slice(0, maxChars));
                word = word.slice(maxChars);
            }
            cur = word;
        }
    }
    if (cur) lines.push(cur);
    return lines.length ? lines : [''];
}

// ── Skip tags ─────────────────────────────────────────────────────────────────

var SKIP_TAGS = { head:1, style:1, script:1, meta:1, link:1, title:1 };

// ── Layout engine ─────────────────────────────────────────────────────────────

/**
 * Entry point. Returns a flat array of draw commands:
 *   { type:'rect', x,y,w,h, r,g,b }
 *   { type:'text', x,y, text, r,g,b, bold? }
 */
function layoutDOM(root, viewport) {
    // viewport: { x, y, w, h }
    var cmds = [];
    layoutNode(root, viewport.x, viewport.y, viewport.w, cmds);
    return cmds;
}

// Returns the Y coordinate after laying out this node.
function layoutNode(node, x, y, maxW, cmds) {
    if (node.nodeType === 3) {
        // Raw text node at block level (rare – normally caught by inline flush below)
        var lines = wrapText(node.text, maxW);
        var fg = hexToRgb(parseColor('color' in (node.computedStyle || {}) ? node.computedStyle.color : null) || '#c8dcff');
        for (var i = 0; i < lines.length; i++) {
            cmds.push({ type:'text', x:x, y:y, text:lines[i], r:fg[0], g:fg[1], b:fg[2] });
            y += CHAR_H;
        }
        return y;
    }
    if (node.nodeType !== 1) return y;
    if (SKIP_TAGS[node.tagName]) return y;

    var style = node.computedStyle || {};

    // Box model
    var mTop    = parsePx(style.marginTop    || style.margin,    maxW);
    var mBottom = parsePx(style.marginBottom || style.margin,    maxW);
    var mLeft   = parsePx(style.marginLeft   || style.margin,    maxW);
    var pTop    = parsePx(style.paddingTop   || style.padding,   maxW);
    var pBottom = parsePx(style.paddingBottom|| style.padding,   maxW);
    var pLeft   = parsePx(style.paddingLeft  || style.padding,   maxW);
    var pRight  = parsePx(style.paddingRight || style.padding,   maxW);

    var nodeW    = style.width  ? parsePx(style.width,  maxW) : maxW;
    var explicitH = style.height ? parsePx(style.height, 0)   : 0;

    var bgRaw = style.backgroundColor || style.background;
    var bgHex = bgRaw ? parseColor(bgRaw) : null;

    var contentX = x + mLeft + pLeft;
    var contentW = nodeW - mLeft - pLeft - pRight;
    var startY   = y + mTop;
    var curY     = startY + pTop;

    var fgHex = parseColor(style.color) || '#c8dcff';
    var fg    = hexToRgb(fgHex);
    var bold  = style.fontWeight === 'bold';

    // Remember insert position so we can put the background BEFORE children
    var bgInsert = cmds.length;

    // ── Lay out children ──────────────────────────────────────────────────────
    var inlineItems = []; // accumulate inline children

    function flushInline() {
        if (!inlineItems.length) return;
        var text = inlineItems.map(function(it) {
            return it.nodeType === 3 ? it.text : (it.children || []).map(function(c) { return c.nodeType === 3 ? c.text : ''; }).join('');
        }).join(' ').trim();
        if (text) {
            var lines = wrapText(text, contentW);
            var ifg = inlineItems[0] && inlineItems[0].computedStyle && inlineItems[0].computedStyle.color
                    ? hexToRgb(parseColor(inlineItems[0].computedStyle.color)) : fg;
            for (var li = 0; li < lines.length; li++) {
                cmds.push({ type:'text', x:contentX, y:curY, text:lines[li], r:ifg[0], g:ifg[1], b:ifg[2], bold:bold });
                curY += CHAR_H;
            }
        }
        inlineItems = [];
    }

    var children = node.children || [];
    for (var ci = 0; ci < children.length; ci++) {
        var child = children[ci];
        var childDisplay = child.nodeType === 3 ? 'inline' : ((child.computedStyle || {}).display || 'block');

        if (childDisplay === 'inline' || child.nodeType === 3) {
            inlineItems.push(child);
        } else {
            flushInline();
            curY = layoutNode(child, contentX, curY, contentW, cmds);
        }
    }
    flushInline();

    curY += pBottom;
    var nodeH = explicitH || Math.max(0, curY - startY);

    // Draw background before children by inserting at bgInsert
    if (bgHex) {
        var bg = hexToRgb(bgHex);
        cmds.splice(bgInsert, 0, { type:'rect', x: x + mLeft, y: startY, w: nodeW - mLeft, h: nodeH, r:bg[0], g:bg[1], b:bg[2] });
    }

    return curY + mBottom;
}

module.exports = { layoutDOM: layoutDOM, hexToRgb: hexToRgb, parseColor: parseColor };
