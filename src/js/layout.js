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

// ── Skip tags ─────────────────────────────────────────────────────────────────

var SKIP_TAGS = { head:1, style:1, script:1, meta:1, link:1, title:1 };

// ── Table layout ──────────────────────────────────────────────────────────────

function layoutTable(node, x, y, maxW, cmds, links) {
    var style = node.computedStyle || {};
    var mTop = parsePx(style.marginTop || style.margin, maxW);
    var mBottom = parsePx(style.marginBottom || style.margin, maxW);
    var startY = y + mTop;
    var curY = startY;
    var hasBorder = node.attrs.border || style.borderCollapse || style.border;

    var rows = [];
    var maxCols = 0;
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) {
        var child = children[i];
        if (child.nodeType !== 1) continue;
        if (child.tagName === 'thead' || child.tagName === 'tbody' || child.tagName === 'tfoot') {
            for (var j = 0; j < (child.children || []).length; j++) {
                var row = child.children[j];
                if (row.nodeType === 1 && row.tagName === 'tr') {
                    var cells = (row.children || []).filter(function(c) { return c.nodeType === 1 && (c.tagName === 'td' || c.tagName === 'th'); });
                    rows.push(cells);
                    if (cells.length > maxCols) maxCols = cells.length;
                }
            }
        } else if (child.tagName === 'tr') {
            var cells = (child.children || []).filter(function(c) { return c.nodeType === 1 && (c.tagName === 'td' || c.tagName === 'th'); });
            rows.push(cells);
            if (cells.length > maxCols) maxCols = cells.length;
        }
    }

    if (maxCols === 0) return curY + mBottom;
    var colW = Math.floor(maxW / maxCols);

    for (var ri = 0; ri < rows.length; ri++) {
        var row = rows[ri];
        var cellX = x;
        var rowStartY = curY;
        var rowMaxH = CHAR_H;
        for (var ci = 0; ci < row.length; ci++) {
            var cell = row[ci];
            var cellEndY = layoutNode(cell, cellX, curY, colW, cmds, links);
            var cellH = cellEndY - curY;
            if (cellH > rowMaxH) rowMaxH = cellH;
            cellX += colW;
        }
        if (hasBorder) {
            cellX = x;
            for (var ci = 0; ci <= row.length; ci++) {
                cmds.push({ type:'line', x0:cellX, y0:rowStartY, x1:cellX, y1:rowStartY + rowMaxH, r:80, g:80, b:100 });
                cellX += colW;
            }
            cmds.push({ type:'line', x0:x, y0:rowStartY, x1:x + maxW, y1:rowStartY, r:80, g:80, b:100 });
        }
        curY = rowStartY + rowMaxH;
    }
    if (hasBorder) {
        cmds.push({ type:'line', x0:x, y0:curY, x1:x + maxW, y1:curY, r:80, g:80, b:100 });
    }
    return curY + mBottom;
}

// ── Layout engine ─────────────────────────────────────────────────────────────

/**
 * Entry point. Returns { cmds, links, totalHeight }:
 *   cmds:  array of draw commands
 *   links: array of { x, y, w, h, href } hit-boxes
 *   totalHeight: final Y after layout
 */
function layoutDOM(root, viewport) {
    var cmds = [];
    var links = [];
    var endY = layoutNode(root, viewport.x, viewport.y, viewport.w, cmds, links);
    return { cmds: cmds, links: links, totalHeight: endY };
}

// Returns the Y coordinate after laying out this node.
function layoutNode(node, x, y, maxW, cmds, links) {
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

    // ── Table early return ────────────────────────────────────────────────────
    if (node.tagName === 'table') {
        return layoutTable(node, x, y, maxW, cmds, links);
    }

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

    // max-width / min-width clamping
    var maxWidth = style.maxWidth ? parsePx(style.maxWidth, maxW) : 0;
    var minWidth = style.minWidth ? parsePx(style.minWidth, maxW) : 0;
    if (maxWidth > 0 && nodeW > maxWidth) nodeW = maxWidth;
    if (minWidth > 0 && nodeW < minWidth) nodeW = minWidth;

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

    // ── HR early return ───────────────────────────────────────────────────────
    if (node.tagName === 'hr') {
        curY += 4;
        cmds.push({ type:'line', x0:contentX, y0:curY, x1:contentX + contentW, y1:curY, r:100, g:110, b:140 });
        curY += 4;
        return curY + mBottom;
    }

    // ── List tracking ─────────────────────────────────────────────────────────
    var isList = (node.tagName === 'ul' || node.tagName === 'ol');
    var listCounter = 0;

    // ── Inline flow state ─────────────────────────────────────────────────────
    var lineX = contentX;
    var lineY = curY;
    var lineH = 0;
    var lineStartIdx = cmds.length; // index of first cmd on current line (for text-align)
    var textAlign = style.textAlign || 'left';

    function flushLine() {
        if (lineH === 0) return;
        // text-align post-processing for the completed line
        if (textAlign === 'center' || textAlign === 'right') {
            var lineUsedW = lineX - contentX;
            var shiftX = 0;
            if (textAlign === 'center') {
                shiftX = Math.floor((contentW - lineUsedW) / 2);
            } else {
                shiftX = contentW - lineUsedW;
            }
            if (shiftX > 0) {
                for (var si = lineStartIdx; si < cmds.length; si++) {
                    var c = cmds[si];
                    if (c.type === 'text') c.x += shiftX;
                    if (c.type === 'underline') c.x += shiftX;
                }
                // Also shift any links emitted on this line
                for (var li = 0; li < links.length; li++) {
                    var lk = links[li];
                    if (lk._lineId === lineStartIdx) lk.x += shiftX;
                }
            }
        }
        lineY += lineH;
        lineX = contentX;
        lineH = 0;
        lineStartIdx = cmds.length;
    }

    function layoutInlineNode(inode, inheritFg, inheritBold, inheritUnderline, linkHref) {
        if (inode.nodeType === 3) {
            // Text node
            var text = inode.text || '';
            var ist = inode.computedStyle || {};
            var whiteSpace = ist.whiteSpace || '';

            if (whiteSpace === 'pre') {
                // Preserve newlines and spaces
                var preLines = text.split('\n');
                for (var pl = 0; pl < preLines.length; pl++) {
                    if (pl > 0) {
                        flushLine();
                    }
                    var preLine = preLines[pl];
                    if (preLine.length > 0) {
                        var tw = preLine.length * CHAR_W;
                        cmds.push({ type:'text', x:lineX, y:lineY, text:preLine, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2], bold:inheritBold });
                        if (inheritUnderline) {
                            cmds.push({ type:'underline', x:lineX, y:lineY + CHAR_H - 2, w:tw, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2] });
                        }
                        if (linkHref) {
                            links.push({ x:lineX, y:lineY, w:tw, h:CHAR_H, href:linkHref, _lineId:lineStartIdx });
                        }
                        lineX += tw;
                        if (CHAR_H > lineH) lineH = CHAR_H;
                    }
                }
                return;
            }

            // Normal word-wrap flow
            var words = text.split(/\s+/);
            for (var wi = 0; wi < words.length; wi++) {
                var word = words[wi];
                if (!word) continue;
                var wordW = word.length * CHAR_W;
                var spaceW = (lineX > contentX) ? CHAR_W : 0;

                // Wrap if needed
                if (lineX + spaceW + wordW > contentX + contentW && lineX > contentX) {
                    flushLine();
                    spaceW = 0;
                }

                // Hard-break very long words
                while (wordW > contentW) {
                    var fitChars = Math.max(1, Math.floor((contentX + contentW - lineX) / CHAR_W));
                    var part = word.slice(0, fitChars);
                    var partW = part.length * CHAR_W;
                    cmds.push({ type:'text', x:lineX, y:lineY, text:part, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2], bold:inheritBold });
                    if (inheritUnderline) {
                        cmds.push({ type:'underline', x:lineX, y:lineY + CHAR_H - 2, w:partW, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2] });
                    }
                    if (linkHref) {
                        links.push({ x:lineX, y:lineY, w:partW, h:CHAR_H, href:linkHref, _lineId:lineStartIdx });
                    }
                    lineX += partW;
                    if (CHAR_H > lineH) lineH = CHAR_H;
                    flushLine();
                    word = word.slice(fitChars);
                    wordW = word.length * CHAR_W;
                    spaceW = 0;
                }

                if (!word) continue;

                var drawX = lineX + spaceW;
                cmds.push({ type:'text', x:drawX, y:lineY, text:word, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2], bold:inheritBold });
                if (inheritUnderline) {
                    cmds.push({ type:'underline', x:drawX, y:lineY + CHAR_H - 2, w:wordW, r:inheritFg[0], g:inheritFg[1], b:inheritFg[2] });
                }
                if (linkHref) {
                    links.push({ x:drawX, y:lineY, w:wordW, h:CHAR_H, href:linkHref, _lineId:lineStartIdx });
                }
                lineX = drawX + wordW;
                if (CHAR_H > lineH) lineH = CHAR_H;
            }
            return;
        }

        // Inline element node
        if (inode.nodeType !== 1) return;
        if (SKIP_TAGS[inode.tagName]) return;

        var ist = inode.computedStyle || {};
        var iFgHex = parseColor(ist.color) || null;
        var iFg = iFgHex ? hexToRgb(iFgHex) : inheritFg;
        var iBold = ist.fontWeight === 'bold' ? true : inheritBold;
        var iUnderline = (ist.textDecoration === 'underline') ? true : inheritUnderline;
        var iLink = linkHref;

        // <a> tags: default color and underline, track href
        if (inode.tagName === 'a') {
            if (!iFgHex) iFg = hexToRgb('#6496ff');
            iUnderline = true;
            iLink = (inode.attrs && inode.attrs.href) ? inode.attrs.href : linkHref;
        }

        var iChildren = inode.children || [];
        for (var ic = 0; ic < iChildren.length; ic++) {
            layoutInlineNode(iChildren[ic], iFg, iBold, iUnderline, iLink);
        }
    }

    function flushInline() {
        if (lineX > contentX || lineH > 0) {
            flushLine();
        }
        curY = lineY;
    }

    // ── Lay out children ──────────────────────────────────────────────────────
    var children = node.children || [];
    var hasInline = false;

    for (var ci = 0; ci < children.length; ci++) {
        var child = children[ci];
        var childDisplay = child.nodeType === 3 ? 'inline' : ((child.computedStyle || {}).display || 'block');

        if (childDisplay === 'inline' || child.nodeType === 3) {
            if (!hasInline) {
                // Start new inline run
                lineX = contentX;
                lineY = curY;
                lineH = 0;
                lineStartIdx = cmds.length;
                hasInline = true;
            }
            layoutInlineNode(child, fg, bold, false, null);
        } else {
            // Block child – flush any pending inline content
            if (hasInline) {
                flushInline();
                hasInline = false;
            }

            // List item bullet/number prefix
            if (isList && child.tagName === 'li') {
                listCounter++;
                var bullet;
                if (node.tagName === 'ol') {
                    bullet = listCounter + '. ';
                } else {
                    bullet = '\u2022 '; // •
                }
                var bulletW = bullet.length * CHAR_W;
                cmds.push({ type:'text', x:contentX, y:curY, text:bullet, r:fg[0], g:fg[1], b:fg[2], bold:false });
            }

            curY = layoutNode(child, contentX, curY, contentW, cmds, links);
        }
    }

    // Flush trailing inline content
    if (hasInline) {
        flushInline();
    }

    curY += pBottom;
    var nodeH = explicitH || Math.max(0, curY - startY);

    // Draw background before children by inserting at bgInsert
    if (bgHex) {
        var bg = hexToRgb(bgHex);
        cmds.splice(bgInsert, 0, { type:'rect', x: x + mLeft, y: startY, w: nodeW - mLeft, h: nodeH, r:bg[0], g:bg[1], b:bg[2] });
    }

    // ── Blockquote left border ────────────────────────────────────────────────
    if (node.tagName === 'blockquote') {
        var borderColor = parseColor((style.borderLeft || '').split(' ').pop()) || '#444444';
        var bc = hexToRgb(borderColor);
        cmds.splice(bgInsert, 0, { type:'line', x0:x + mLeft + 2, y0:startY + pTop, x1:x + mLeft + 2, y1:curY - pBottom, r:bc[0], g:bc[1], b:bc[2] });
    }

    return curY + mBottom;
}

// ── Legacy wrapText (kept for block-level text fallback) ──────────────────────

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

module.exports = { layoutDOM: layoutDOM, hexToRgb: hexToRgb, parseColor: parseColor };
