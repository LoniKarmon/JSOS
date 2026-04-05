# demo_browser_v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a real web browser for JSOS that fetches and renders live web pages (including Wikipedia) with scrolling and clickable links, by upgrading the CSS/layout/paint engine.

**Architecture:** Upgrade css.js with compound/descendant/child selectors and new properties. Rewrite layout.js to support inline flow, tables, lists, and link hit-boxes. Extend paint.js with scroll viewport and new draw commands. Build demo_browser_v2.jsos as a 900x600 windowed app with URL bar, content area, status bar, fetch pipeline with Wikipedia mobile API rewriting, scroll, and link navigation.

**Tech Stack:** JavaScript (QuickJS), existing dom/css/layout/paint modules, os.fetch, os.window, libjsos.js Window/Keys

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/js/css.js` | Modify | Add compound/descendant/child selectors, new properties, specificity upgrade |
| `src/js/layout.js` | Modify (major) | Inline flow, tables, lists, links, hr/blockquote/pre, text-align, return `{cmds, links, totalHeight}` |
| `src/js/paint.js` | Modify | Scroll viewport offset, `line` and `underline` command types, culling |
| `src/jsos/demo_browser.jsos` | Modify (small) | Update to use new `layoutDOM()` return shape |
| `src/jsos/demo_browser_v2.jsos` | Create | Full browser app |
| `src/js_runtime.rs` | Modify (2 lines) | Register `demo_browser_v2.jsos` in BINS |
| `src/main.rs` | Modify (1 line) | Register `demo_browser_v2.jsos` in BUILTIN_BINS |
| `src/jsos/shell.jsos` | Modify (1 line) | Add `browser2` command to shell |

---

### Task 1: CSS Engine — Compound and Descendant Selectors

**Files:**
- Modify: `src/js/css.js`

- [ ] **Step 1: Rewrite the `specificity()` function**

Replace the existing `specificity` function (lines 39-45) with one that handles compound selectors:

```js
function specificity(sel) {
    if (!sel) return 0;
    sel = sel.trim();
    // Split compound parts: "div.foo#bar" → ["div", ".foo", "#bar"]
    var parts = sel.match(/#[\w-]+|\.[\w-]+|\[[\w-]+(?:=[^\]]+)?\]|:[\w-]+(?:\([^)]*\))?|[\w-]+|\*/g) || [];
    var a = 0, b = 0, c = 0;
    for (var i = 0; i < parts.length; i++) {
        var p = parts[i];
        if (p[0] === '#') a++;
        else if (p[0] === '.' || p[0] === '[' || p[0] === ':') b++;
        else if (p !== '*') c++;
    }
    return a * 100 + b * 10 + c;
}
```

- [ ] **Step 2: Rewrite the `matches()` function for compound selectors**

Replace the existing `matches` function (lines 47-57) with one that handles `.foo.bar`, `div.class`, `#id.class`, and attribute selectors:

```js
function matchesCompound(node, compound) {
    if (!node || node.nodeType !== 1) return false;
    compound = compound.trim();
    if (compound === '*') return true;
    // Tokenize compound selector: "div.foo#bar[href]:first-child"
    var parts = compound.match(/#[\w-]+|\.[\w-]+|\[[\w-]+(?:=[^\]"']+|="[^"]*"|='[^']*')?\]|:[\w-]+(?:\([^)]*\))?|[\w-]+|\*/g);
    if (!parts) return false;
    var classes = (node.attrs['class'] || '').split(/\s+/);
    for (var i = 0; i < parts.length; i++) {
        var p = parts[i];
        if (p[0] === '#') {
            if (node.attrs.id !== p.slice(1)) return false;
        } else if (p[0] === '.') {
            if (classes.indexOf(p.slice(1)) === -1) return false;
        } else if (p[0] === '[') {
            var inner = p.slice(1, -1);
            var eqIdx = inner.indexOf('=');
            if (eqIdx === -1) {
                if (!(inner in node.attrs)) return false;
            } else {
                var attrName = inner.slice(0, eqIdx);
                var attrVal = inner.slice(eqIdx + 1).replace(/^["']|["']$/g, '');
                if (node.attrs[attrName] !== attrVal) return false;
            }
        } else if (p[0] === ':') {
            var pseudo = p.slice(1);
            if (pseudo === 'first-child') {
                if (!node.parent || node.parent.children.indexOf(node) !== 0) return false;
            } else if (pseudo === 'last-child') {
                if (!node.parent || node.parent.children.indexOf(node) !== node.parent.children.length - 1) return false;
            }
            // Unknown pseudo-classes are ignored (match passes)
        } else if (p !== '*') {
            if (node.tagName !== p.toLowerCase()) return false;
        }
    }
    return true;
}
```

- [ ] **Step 3: Add descendant and child selector support in a new `matchesSelector()` function**

Add this function after `matchesCompound`:

```js
function matchesSelector(node, selector) {
    if (!node || node.nodeType !== 1) return false;
    selector = selector.trim();
    // Split on combinators: ' ' (descendant) and '>' (child)
    // "div > p .foo" → [{compound:"div"}, {combinator:">", compound:"p"}, {combinator:" ", compound:".foo"}]
    var tokens = [];
    var parts = selector.split(/\s*(>)\s*|\s+/);
    // The split captures '>' in odd slots. Rebuild token list.
    var compoundParts = selector.replace(/\s*>\s*/g, ' > ').split(/\s+/);
    if (compoundParts.length === 1) {
        return matchesCompound(node, compoundParts[0]);
    }
    // Right-to-left matching
    var rightmost = compoundParts[compoundParts.length - 1];
    if (!matchesCompound(node, rightmost)) return false;

    var cur = node;
    for (var i = compoundParts.length - 2; i >= 0; i--) {
        var part = compoundParts[i];
        if (part === '>') continue; // skip combinator token, handled below
        var isChild = (i + 1 < compoundParts.length && compoundParts[i + 1] === '>');
        if (isChild) {
            cur = cur.parent;
            if (!cur || !matchesCompound(cur, part)) return false;
        } else {
            // Descendant — walk up
            var found = false;
            cur = cur.parent;
            while (cur && cur.nodeType === 1) {
                if (matchesCompound(cur, part)) { found = true; break; }
                cur = cur.parent;
            }
            if (!found) return false;
        }
    }
    return true;
}
```

- [ ] **Step 4: Update `applyStyles()` to use `matchesSelector`**

In `applyStyles`, replace the call to `matches(root, rule.selector)` with `matchesSelector(root, rule.selector)`:

```js
// Old:
    sorted.forEach(function(rule) {
        if (matches(root, rule.selector)) {
// New:
    sorted.forEach(function(rule) {
        if (matchesSelector(root, rule.selector)) {
```

- [ ] **Step 5: Add new properties to TAG_DEFAULTS and BLOCK_TAGS**

Extend the existing objects:

```js
var BLOCK_TAGS = {
    'html':1,'body':1,'div':1,'p':1,'h1':1,'h2':1,'h3':1,'h4':1,'h5':1,'h6':1,
    'ul':1,'ol':1,'li':1,'section':1,'article':1,'header':1,'footer':1,'nav':1,'main':1,
    'table':1,'tr':1,'blockquote':1,'pre':1,'hr':1,'figure':1,'figcaption':1,'dl':1,'dt':1,'dd':1
};

var INLINE_TAGS = {
    'a':1,'span':1,'em':1,'strong':1,'b':1,'i':1,'u':1,'code':1,'small':1,'sub':1,'sup':1,'abbr':1,'cite':1,'mark':1,'s':1,'time':1
};

var TAG_DEFAULTS = {
    'h1': { fontSize: '20px', fontWeight: 'bold', marginTop: '8px', marginBottom: '4px' },
    'h2': { fontSize: '17px', fontWeight: 'bold', marginTop: '6px', marginBottom: '3px' },
    'h3': { fontSize: '15px', fontWeight: 'bold', marginTop: '4px', marginBottom: '2px' },
    'h4': { fontSize: '14px', fontWeight: 'bold', marginTop: '3px', marginBottom: '2px' },
    'h5': { fontSize: '13px', fontWeight: 'bold', marginTop: '2px', marginBottom: '1px' },
    'h6': { fontSize: '12px', fontWeight: 'bold', marginTop: '2px', marginBottom: '1px' },
    'p':  { marginTop: '4px', marginBottom: '4px' },
    'li': { marginTop: '2px', marginBottom: '2px', display: 'list-item' },
    'ul': { marginTop: '4px', marginBottom: '4px', paddingLeft: '20px', listStyleType: 'disc' },
    'ol': { marginTop: '4px', marginBottom: '4px', paddingLeft: '20px', listStyleType: 'decimal' },
    'a':  { color: '#6496ff', textDecoration: 'underline', display: 'inline' },
    'em': { fontStyle: 'italic', display: 'inline' },
    'i':  { fontStyle: 'italic', display: 'inline' },
    'strong': { fontWeight: 'bold', display: 'inline' },
    'b':  { fontWeight: 'bold', display: 'inline' },
    'u':  { textDecoration: 'underline', display: 'inline' },
    'code': { backgroundColor: '#1a1a2e', display: 'inline' },
    'pre': { backgroundColor: '#1a1a2e', whiteSpace: 'pre', padding: '4px', marginTop: '4px', marginBottom: '4px' },
    'blockquote': { paddingLeft: '12px', marginTop: '4px', marginBottom: '4px', borderLeft: '3px solid #444' },
    'hr': { marginTop: '8px', marginBottom: '8px' },
    'table': { marginTop: '4px', marginBottom: '4px' },
    'th': { fontWeight: 'bold', padding: '2px' },
    'td': { padding: '2px' },
    'span': { display: 'inline' },
    'small': { display: 'inline' },
};
```

- [ ] **Step 6: Extend INHERITABLE and update display defaults**

```js
var INHERITABLE = ['color', 'fontSize', 'fontWeight', 'fontFamily', 'lineHeight', 'textAlign', 'textDecoration', 'whiteSpace', 'listStyleType', 'fontStyle'];
```

Update the display default logic in `applyStyles`:

```js
    // display default
    if (!computed.display) {
        if (INLINE_TAGS[root.tagName]) {
            computed.display = 'inline';
        } else if (root.tagName === 'table') {
            computed.display = 'table';
        } else if (root.tagName === 'tr') {
            computed.display = 'table-row';
        } else if (root.tagName === 'td' || root.tagName === 'th') {
            computed.display = 'table-cell';
        } else if (BLOCK_TAGS[root.tagName]) {
            computed.display = 'block';
        } else {
            computed.display = 'inline';
        }
    }
```

- [ ] **Step 7: Export `matchesSelector` and `matchesCompound`, keep `matches` for compat**

```js
module.exports = { parseCSS: parseCSS, applyStyles: applyStyles, matchesSelector: matchesSelector, matchesCompound: matchesCompound };
```

- [ ] **Step 8: Commit**

```bash
git add src/js/css.js
git commit -m "feat(css): compound/descendant/child selectors, attribute selectors, new properties"
```

---

### Task 2: Layout Engine — Inline Flow Model

**Files:**
- Modify: `src/js/layout.js`

- [ ] **Step 1: Add inline element tracking constants**

Add after the existing `SKIP_TAGS` definition (line 82):

```js
var INLINE_DISPLAY = { 'inline':1, 'inline-block':1 };
```

- [ ] **Step 2: Refactor layoutDOM return value**

Change `layoutDOM` to return an object:

```js
function layoutDOM(root, viewport) {
    var cmds = [];
    var links = [];
    var endY = layoutNode(root, viewport.x, viewport.y, viewport.w, cmds, links);
    return { cmds: cmds, links: links, totalHeight: endY };
}
```

Update `layoutNode` signature to accept `links` array:

```js
function layoutNode(node, x, y, maxW, cmds, links) {
```

All recursive calls to `layoutNode` must pass `links` as the 6th argument.

- [ ] **Step 3: Rewrite inline child handling with line cursor**

Replace the existing `flushInline` and inline accumulation in `layoutNode` (lines 143-174) with a proper inline flow model:

```js
    // ── Lay out children ──────────────────────────────────────────────────────
    var lineX = contentX;
    var lineY = curY;
    var lineH = CHAR_H;
    var inRun = false; // are we currently in an inline run?

    function flushLine() {
        if (lineX > contentX) {
            // Line ended, advance Y
            curY = lineY + lineH;
            lineX = contentX;
            lineY = curY;
            lineH = CHAR_H;
        }
    }

    function layoutInlineNode(inode) {
        if (inode.nodeType === 3) {
            // Text node
            var ist = inode.parent ? (inode.parent.computedStyle || {}) : {};
            var iFg = hexToRgb(parseColor(ist.color) || fgHex);
            var iBold = ist.fontWeight === 'bold';
            var isUnderline = ist.textDecoration === 'underline';
            var href = null;
            // Check if parent is an <a> tag
            if (inode.parent && inode.parent.tagName === 'a') {
                href = inode.parent.attrs.href || null;
                iFg = hexToRgb(parseColor(ist.color) || '#6496ff');
                isUnderline = true;
            }
            var ws = ist.whiteSpace || 'normal';
            var text = inode.text;
            if (ws === 'pre') {
                // Preserve whitespace: render line by line
                var preLines = text.split('\n');
                for (var pl = 0; pl < preLines.length; pl++) {
                    var ptxt = preLines[pl];
                    if (ptxt.length > 0) {
                        cmds.push({ type:'text', x:lineX, y:lineY, text:ptxt, r:iFg[0], g:iFg[1], b:iFg[2], bold:iBold });
                        if (isUnderline) {
                            var uw = ptxt.length * CHAR_W;
                            cmds.push({ type:'underline', x:lineX, y:lineY + CHAR_H, w:uw, r:iFg[0], g:iFg[1], b:iFg[2] });
                        }
                        if (href) {
                            links.push({ x:lineX, y:lineY, w:ptxt.length * CHAR_W, h:CHAR_H, href:href });
                        }
                        lineX += ptxt.length * CHAR_W;
                    }
                    if (pl < preLines.length - 1) {
                        flushLine();
                    }
                }
                return;
            }
            // Normal: collapse whitespace, word-wrap
            var words = text.split(/\s+/).filter(function(w) { return w.length > 0; });
            for (var wi = 0; wi < words.length; wi++) {
                var word = words[wi];
                var wordW = word.length * CHAR_W;
                var spaceW = (lineX > contentX) ? CHAR_W : 0;
                if (lineX + spaceW + wordW > contentX + contentW && lineX > contentX) {
                    flushLine();
                    spaceW = 0;
                }
                var drawX = lineX + spaceW;
                cmds.push({ type:'text', x:drawX, y:lineY, text:word, r:iFg[0], g:iFg[1], b:iFg[2], bold:iBold });
                if (isUnderline) {
                    cmds.push({ type:'underline', x:drawX, y:lineY + CHAR_H, w:wordW, r:iFg[0], g:iFg[1], b:iFg[2] });
                }
                if (href) {
                    links.push({ x:drawX, y:lineY, w:wordW, h:CHAR_H, href:href });
                }
                lineX = drawX + wordW;
            }
        } else if (inode.nodeType === 1) {
            // Inline element — recurse into children
            var ichildren = inode.children || [];
            for (var ic = 0; ic < ichildren.length; ic++) {
                layoutInlineNode(ichildren[ic]);
            }
        }
    }

    var children = node.children || [];
    for (var ci = 0; ci < children.length; ci++) {
        var child = children[ci];
        if (child.nodeType === 3) {
            inRun = true;
            layoutInlineNode(child);
        } else if (child.nodeType !== 1) {
            continue;
        } else if (SKIP_TAGS[child.tagName]) {
            continue;
        } else {
            var childDisplay = (child.computedStyle || {}).display || 'block';
            if (childDisplay === 'none') continue;
            if (childDisplay === 'inline' || childDisplay === 'inline-block') {
                inRun = true;
                layoutInlineNode(child);
            } else {
                // Block element — flush any inline run first
                if (inRun) { flushLine(); inRun = false; }
                curY = lineY;
                curY = layoutNode(child, contentX, curY, contentW, cmds, links);
                lineX = contentX;
                lineY = curY;
            }
        }
    }
    if (inRun) { flushLine(); inRun = false; }
    curY = lineY;
```

- [ ] **Step 4: Commit**

```bash
git add src/js/layout.js
git commit -m "feat(layout): inline flow model with word wrap, link hit-boxes"
```

---

### Task 3: Layout Engine — Lists, HR, Blockquote, Pre

**Files:**
- Modify: `src/js/layout.js`

- [ ] **Step 1: Add list counter tracking in `layoutNode`**

Before the children loop, detect if this node is a list and set up counters:

```js
    var isList = node.tagName === 'ul' || node.tagName === 'ol';
    var listCounter = 0;
    var listStyle = style.listStyleType || (node.tagName === 'ol' ? 'decimal' : 'disc');
```

- [ ] **Step 2: Add list-item bullet/number rendering**

Inside the block-child branch of the children loop, before recursing into the child, check if the child is a `<li>`:

```js
            if (childDisplay === 'list-item' || child.tagName === 'li') {
                listCounter++;
                var bullet = listStyle === 'decimal' ? (listCounter + '. ') : '\u2022 ';
                var bulletFg = hexToRgb(parseColor((child.computedStyle || {}).color) || fgHex);
                cmds.push({ type:'text', x:contentX, y:curY, text:bullet, r:bulletFg[0], g:bulletFg[1], b:bulletFg[2] });
            }
```

- [ ] **Step 3: Add `<hr>` rendering**

In `layoutNode`, after the box model calculations and before children, handle `<hr>`:

```js
    if (node.tagName === 'hr') {
        curY += 4;
        cmds.push({ type:'line', x0:contentX, y0:curY, x1:contentX + contentW, y1:curY, r:100, g:110, b:140 });
        curY += 4;
        return curY + mBottom;
    }
```

- [ ] **Step 4: Add `<blockquote>` left-border rendering**

After laying out all children of a blockquote, draw the left bar:

```js
    if (node.tagName === 'blockquote') {
        // Draw left border bar from startY to curY
        var borderColor = parseColor((style.borderLeft || '').split(' ').pop()) || '#444444';
        var bc = hexToRgb(borderColor);
        cmds.splice(bgInsert, 0, { type:'line', x0:x + mLeft + 2, y0:startY + pTop, x1:x + mLeft + 2, y1:curY - pBottom, r:bc[0], g:bc[1], b:bc[2] });
    }
```

- [ ] **Step 5: Commit**

```bash
git add src/js/layout.js
git commit -m "feat(layout): lists, hr, blockquote rendering"
```

---

### Task 4: Layout Engine — Table Layout

**Files:**
- Modify: `src/js/layout.js`

- [ ] **Step 1: Add table layout handler**

Add a new function `layoutTable` before the `layoutDOM` function:

```js
function layoutTable(node, x, y, maxW, cmds, links) {
    var style = node.computedStyle || {};
    var mTop = parsePx(style.marginTop || style.margin, maxW);
    var mBottom = parsePx(style.marginBottom || style.margin, maxW);
    var startY = y + mTop;
    var curY = startY;
    var hasBorder = node.attrs.border || style.borderCollapse || style.border;

    // Collect rows and determine column count
    var rows = [];
    var maxCols = 0;
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) {
        var child = children[i];
        if (child.nodeType !== 1) continue;
        if (child.tagName === 'thead' || child.tagName === 'tbody' || child.tagName === 'tfoot') {
            // Unwrap section elements
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

        // Draw borders if requested
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
    // Bottom border
    if (hasBorder) {
        cmds.push({ type:'line', x0:x, y0:curY, x1:x + maxW, y1:curY, r:80, g:80, b:100 });
    }

    return curY + mBottom;
}
```

- [ ] **Step 2: Hook table layout into `layoutNode`**

In `layoutNode`, after the `<hr>` early return, add table detection:

```js
    if (node.tagName === 'table') {
        return layoutTable(node, x + mLeft, startY + pTop, contentW, cmds, links) + pBottom + mBottom;
    }
```

- [ ] **Step 3: Commit**

```bash
git add src/js/layout.js
git commit -m "feat(layout): equal-split table layout with optional borders"
```

---

### Task 5: Layout Engine — Text Alignment and max-width/min-width

**Files:**
- Modify: `src/js/layout.js`

- [ ] **Step 1: Add text-align support**

In `layoutInlineNode`, when pushing text commands, check the parent block's `textAlign`. Add this at the start of `layoutNode`, after computing `contentW`:

```js
    var textAlign = style.textAlign || 'left';
```

After all children are laid out and curY is computed, if `textAlign` is `center` or `right`, post-process the text commands emitted by this node. Add this before the background splice:

```js
    // Post-process text alignment
    if (textAlign === 'center' || textAlign === 'right') {
        // Group text cmds into lines by Y, adjust X
        var textCmds = [];
        for (var ti = bgInsert; ti < cmds.length; ti++) {
            if (cmds[ti].type === 'text') textCmds.push(cmds[ti]);
        }
        // Group by y coordinate
        var lineMap = {};
        for (var ti = 0; ti < textCmds.length; ti++) {
            var key = textCmds[ti].y;
            if (!lineMap[key]) lineMap[key] = [];
            lineMap[key].push(textCmds[ti]);
        }
        var lineKeys = Object.keys(lineMap);
        for (var li = 0; li < lineKeys.length; li++) {
            var lineCmds = lineMap[lineKeys[li]];
            var maxRight = 0;
            for (var lc = 0; lc < lineCmds.length; lc++) {
                var right = lineCmds[lc].x + (lineCmds[lc].text || '').length * CHAR_W;
                if (right > maxRight) maxRight = right;
            }
            var lineW = maxRight - contentX;
            var shift = textAlign === 'center' ? Math.floor((contentW - lineW) / 2) : (contentW - lineW);
            if (shift > 0) {
                for (var lc = 0; lc < lineCmds.length; lc++) {
                    lineCmds[lc].x += shift;
                }
            }
        }
    }
```

- [ ] **Step 2: Add max-width and min-width support**

In `layoutNode`, after computing `nodeW`, apply constraints:

```js
    var maxWidth = style.maxWidth ? parsePx(style.maxWidth, maxW) : 0;
    var minWidth = style.minWidth ? parsePx(style.minWidth, maxW) : 0;
    if (maxWidth > 0 && nodeW > maxWidth) nodeW = maxWidth;
    if (minWidth > 0 && nodeW < minWidth) nodeW = minWidth;
```

- [ ] **Step 3: Commit**

```bash
git add src/js/layout.js
git commit -m "feat(layout): text alignment, max-width, min-width"
```

---

### Task 6: Paint Engine — Scroll Viewport and New Commands

**Files:**
- Modify: `src/js/paint.js`

- [ ] **Step 1: Rewrite paint.js**

Replace the entire file with:

```js
// paint.js – Render layout commands to os.window for JSOS Browser Engine
'use strict';

/**
 * Paint a list of draw commands (produced by layout.layoutDOM) into a window.
 *
 * @param {Array}  cmds     Array of draw commands
 * @param {number} winId    Window ID
 * @param {number} scrollY  Vertical scroll offset (default 0)
 * @param {number} viewH    Viewport height for culling (default 0 = no culling)
 */
function paint(cmds, winId, scrollY, viewH) {
    scrollY = scrollY || 0;
    viewH = viewH || 0;

    for (var i = 0; i < cmds.length; i++) {
        var cmd = cmds[i];
        var y = (cmd.y !== undefined ? cmd.y : 0) - scrollY;

        // Culling: skip commands entirely above or below the viewport
        if (viewH > 0) {
            var cmdH = cmd.h || 16; // default text height
            if (y + cmdH < 0) continue;
            if (y > viewH) continue;
        }

        if (cmd.type === 'rect') {
            os.window.drawRect(winId, cmd.x, y, cmd.w, cmd.h, cmd.r, cmd.g, cmd.b);
        } else if (cmd.type === 'text') {
            os.window.drawString(winId, cmd.text, cmd.x, y, cmd.r, cmd.g, cmd.b);
            if (cmd.bold) {
                os.window.drawString(winId, cmd.text, cmd.x + 1, y, cmd.r, cmd.g, cmd.b);
            }
        } else if (cmd.type === 'line') {
            os.window.drawLine(winId, cmd.x0, cmd.y0 - scrollY, cmd.x1, cmd.y1 - scrollY, cmd.r, cmd.g, cmd.b);
        } else if (cmd.type === 'underline') {
            var uy = cmd.y - scrollY;
            os.window.drawLine(winId, cmd.x, uy, cmd.x + cmd.w, uy, cmd.r, cmd.g, cmd.b);
        }
    }
}

module.exports = { paint: paint };
```

- [ ] **Step 2: Commit**

```bash
git add src/js/paint.js
git commit -m "feat(paint): scroll viewport offset, line and underline commands, culling"
```

---

### Task 7: Update Existing demo_browser.jsos for New Return Shape

**Files:**
- Modify: `src/jsos/demo_browser.jsos`

- [ ] **Step 1: Update the call to `layout.layoutDOM`**

In `updateLayout()` (line 64), change:

```js
    cachedCmds = layout.layoutDOM(domRoot, { x: 0, y: 0, w: W, h: H });
```

To:

```js
    var result = layout.layoutDOM(domRoot, { x: 0, y: 0, w: W, h: H });
    cachedCmds = result.cmds;
```

- [ ] **Step 2: Commit**

```bash
git add src/jsos/demo_browser.jsos
git commit -m "fix(demo_browser): adapt to layoutDOM return object"
```

---

### Task 8: Register demo_browser_v2.jsos in Kernel

**Files:**
- Modify: `src/js_runtime.rs:246`
- Modify: `src/main.rs:160`
- Modify: `src/jsos/shell.jsos:270`

- [ ] **Step 1: Add to BINS in js_runtime.rs**

After line 246 (`demo_browser.jsos`), add:

```rust
        m.insert("demo_browser_v2.jsos".to_string(), include_str!("jsos/demo_browser_v2.jsos").to_string());
```

- [ ] **Step 2: Add to BUILTIN_BINS in main.rs**

After line 160 (`demo_browser.jsos`), add:

```rust
            ("demo_browser_v2.jsos", include_str!("jsos/demo_browser_v2.jsos")),
```

- [ ] **Step 3: Add shell command**

In `src/jsos/shell.jsos`, after the `browser` command (around line 270), add:

```js
    browser2: { desc: "Open browser v2",
        fn: () => console.log("PID " + os.spawn("demo_browser_v2.jsos")) },
```

- [ ] **Step 4: Commit**

```bash
git add src/js_runtime.rs src/main.rs src/jsos/shell.jsos
git commit -m "feat: register demo_browser_v2.jsos in kernel and shell"
```

---

### Task 9: Browser App — Window, URL Bar, and Status Bar

**Files:**
- Create: `src/jsos/demo_browser_v2.jsos`

- [ ] **Step 1: Create the browser app with UI chrome**

```js
// demo_browser_v2.jsos – Web browser for JSOS
// Fetches and renders live web pages with scrolling and clickable links.

import dom    from 'dom';
import css    from 'css';
import layout from 'layout';
import paint  from 'paint';

import { Window, Keys, Theme } from 'libjsos.js';

// ── Constants ────────────────────────────────────────────────────────────────
const W = 900, H = 600;
const URL_BAR_H = 30;
const STATUS_BAR_H = 24;
const CONTENT_Y = URL_BAR_H;
const CONTENT_H = H - URL_BAR_H - STATUS_BAR_H;
const CHAR_W = 8;
const CHAR_H = 16;
const SCROLL_STEP = 48;    // pixels per arrow key / scroll tick
const PAGE_SCROLL = CONTENT_H - 32;

const T = Theme.dark();

// ── State ────────────────────────────────────────────────────────────────────
const win = new Window(30, 20, W, H);

var urlText = 'https://en.m.wikipedia.org/wiki/Main_Page';
var urlCursor = urlText.length;
var urlFocused = true;
var statusText = 'Type a URL and press Enter';
var isLoading = false;

// Page state
var pageHtml = '';
var pageCmds = [];
var pageLinks = [];
var pageTotalHeight = 0;
var scrollY = 0;
var history = [];
var currentUrl = '';

// ── URL Bar ──────────────────────────────────────────────────────────────────
function drawUrlBar() {
    // Background
    win.rect(0, 0, W, URL_BAR_H, T.surface[0], T.surface[1], T.surface[2]);
    // Border bottom
    win.rect(0, URL_BAR_H - 1, W, 1, T.dim[0], T.dim[1], T.dim[2]);
    // URL text (show last ~108 chars to fit in bar)
    var maxChars = Math.floor((W - 16) / CHAR_W);
    var displayUrl = urlText.length > maxChars ? urlText.slice(urlText.length - maxChars) : urlText;
    win.text(8, 7, displayUrl, T.text[0], T.text[1], T.text[2]);
    // Cursor
    if (urlFocused) {
        var cursorOffset = Math.min(urlCursor, maxChars);
        win.rect(8 + cursorOffset * CHAR_W, 6, 1, CHAR_H, T.accent[0], T.accent[1], T.accent[2]);
    }
}

// ── Status Bar ───────────────────────────────────────────────────────────────
function drawStatusBar() {
    var y = H - STATUS_BAR_H;
    win.rect(0, y, W, STATUS_BAR_H, T.surface[0], T.surface[1], T.surface[2]);
    // Border top
    win.rect(0, y, W, 1, T.dim[0], T.dim[1], T.dim[2]);
    // Status text
    var maxChars = Math.floor((W - 16) / CHAR_W);
    var display = statusText.length > maxChars ? statusText.slice(0, maxChars) : statusText;
    win.text(8, y + 4, display, T.dim[0], T.dim[1], T.dim[2]);
    // Scroll position
    if (pageTotalHeight > CONTENT_H) {
        var pct = Math.round(scrollY / (pageTotalHeight - CONTENT_H) * 100);
        var scrollStr = pct + '%';
        win.text(W - 8 - scrollStr.length * CHAR_W, y + 4, scrollStr, T.dim[0], T.dim[1], T.dim[2]);
    }
}

// ── Content Area ─────────────────────────────────────────────────────────────
function drawContent() {
    // Clear content area
    win.rect(0, CONTENT_Y, W, CONTENT_H, T.bg[0], T.bg[1], T.bg[2]);

    if (pageCmds.length === 0 && !isLoading) {
        win.text(W / 2 - 80, H / 2 - 8, 'Enter a URL to browse', T.dim[0], T.dim[1], T.dim[2]);
        return;
    }

    if (isLoading) {
        win.text(W / 2 - 40, H / 2 - 8, 'Loading...', T.accent[0], T.accent[1], T.accent[2]);
        return;
    }

    // Paint with scroll offset, shifted into content area
    // We need to offset all commands by CONTENT_Y, so adjust scrollY
    paint.paint(pageCmds, win.id, scrollY - CONTENT_Y, CONTENT_H);
}

// ── Full Render ──────────────────────────────────────────────────────────────
function render() {
    drawUrlBar();
    drawContent();
    drawStatusBar();
    win.flush();
}

// ── Placeholder: navigate and fetch are in next task ─────────────────────────
function navigate(url) {}
function handleKey(charCode) {}
function handleMouse(mx, my, buttons) {}
function handleScroll(delta) {}

// ── IPC ──────────────────────────────────────────────────────────────────────
win.installIpc({
    render: render,
    key: function(charCode) { handleKey(charCode); },
    mouse: function(mx, my, buttons) { handleMouse(mx, my, buttons); },
    scroll: function(delta) { handleScroll(delta); },
});

// ── Boot ─────────────────────────────────────────────────────────────────────
render();
```

- [ ] **Step 2: Commit**

```bash
git add src/jsos/demo_browser_v2.jsos
git commit -m "feat(browser_v2): window chrome with URL bar, content area, status bar"
```

---

### Task 10: Browser App — Fetch Pipeline and HTML Pre-processing

**Files:**
- Modify: `src/jsos/demo_browser_v2.jsos`

- [ ] **Step 1: Implement the HTML pre-processor**

Replace the placeholder `navigate` function with the full fetch pipeline:

```js
// ── HTML Pre-processing ──────────────────────────────────────────────────────
function sanitizeHtml(html) {
    // Strip script, noscript, style (we use embedded CSS), svg, and meta tags
    html = html.replace(/<script[\s\S]*?<\/script>/gi, '');
    html = html.replace(/<noscript[\s\S]*?<\/noscript>/gi, '');
    html = html.replace(/<svg[\s\S]*?<\/svg>/gi, '');
    // Strip Wikipedia navigation, sidebars, footers
    html = html.replace(/<nav[\s\S]*?<\/nav>/gi, '');
    html = html.replace(/<footer[\s\S]*?<\/footer>/gi, '');
    // Strip elements with common nav/sidebar classes
    html = html.replace(/<[^>]+class="[^"]*(?:mw-jump-link|noprint|navbox|sidebar|mw-editsection|reference|reflist|mw-references)[^"]*"[^>]*>[\s\S]*?<\/[^>]+>/gi, '');
    // Strip <link>, <meta> tags
    html = html.replace(/<link[^>]*>/gi, '');
    html = html.replace(/<meta[^>]*>/gi, '');
    return html;
}

function rewriteWikipediaUrl(url) {
    // Rewrite desktop wikipedia URLs to mobile
    var m = url.match(/https?:\/\/(\w+)\.wikipedia\.org\/wiki\/(.+)/);
    if (m) {
        return 'https://' + m[1] + '.m.wikipedia.org/wiki/' + m[2];
    }
    return url;
}

function resolveUrl(href, baseUrl) {
    if (!href) return '';
    // Already absolute
    if (href.indexOf('http://') === 0 || href.indexOf('https://') === 0) return href;
    // Protocol-relative
    if (href.indexOf('//') === 0) return 'https:' + href;
    // Relative to origin
    try {
        var originMatch = baseUrl.match(/^(https?:\/\/[^\/]+)/);
        if (!originMatch) return href;
        var origin = originMatch[1];
        if (href[0] === '/') return origin + href;
        // Relative to current path
        var lastSlash = baseUrl.lastIndexOf('/');
        return baseUrl.slice(0, lastSlash + 1) + href;
    } catch(e) {
        return href;
    }
}

// ── Navigate ─────────────────────────────────────────────────────────────────
async function navigate(url) {
    if (isLoading) return;

    url = rewriteWikipediaUrl(url);

    // Push to history
    if (currentUrl) history.push(currentUrl);
    currentUrl = url;
    urlText = url;
    urlCursor = url.length;
    urlFocused = false;
    scrollY = 0;

    // Show loading
    isLoading = true;
    statusText = 'Loading ' + url + '...';
    pageCmds = [];
    pageLinks = [];
    render();

    try {
        var html = await os.fetch(url);
        pageHtml = sanitizeHtml(html);

        // Parse and layout
        var domRoot = dom.parseHTML(pageHtml);
        var rules = css.parseCSS(domRoot._embeddedCSS || '');
        css.applyStyles(domRoot, rules, {});
        var result = layout.layoutDOM(domRoot, { x: 0, y: CONTENT_Y, w: W, h: CONTENT_H });
        pageCmds = result.cmds;
        pageLinks = result.links;
        pageTotalHeight = result.totalHeight;
        scrollY = 0;

        statusText = url;
        isLoading = false;
        render();
    } catch(e) {
        isLoading = false;
        statusText = 'Error: ' + e;
        pageCmds = [];
        pageLinks = [];
        render();
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add src/jsos/demo_browser_v2.jsos
git commit -m "feat(browser_v2): fetch pipeline, HTML sanitizer, Wikipedia URL rewriting"
```

---

### Task 11: Browser App — Keyboard, Mouse, and Scroll Handlers

**Files:**
- Modify: `src/jsos/demo_browser_v2.jsos`

- [ ] **Step 1: Implement keyboard handler**

Replace the placeholder `handleKey`:

```js
function handleKey(charCode) {
    if (charCode === Keys.CTRL_Q || (charCode === Keys.ESCAPE && !urlFocused)) {
        os.exit();
        return;
    }

    // Ctrl+L — focus URL bar
    if (charCode === 12) { // Ctrl+L
        urlFocused = true;
        urlCursor = urlText.length;
        render();
        return;
    }

    // '[' — go back (when not in URL bar)
    if (charCode === 91 && !urlFocused) { // '['
        if (history.length > 0) {
            var prev = history.pop();
            currentUrl = '';
            urlText = prev;
            urlCursor = prev.length;
            navigate(prev);
        }
        return;
    }

    // Scroll keys (when not in URL bar)
    if (!urlFocused) {
        if (charCode === Keys.UP) {
            scrollY = Math.max(0, scrollY - SCROLL_STEP);
            render();
            return;
        }
        if (charCode === Keys.DOWN) {
            scrollY = Math.min(Math.max(0, pageTotalHeight - CONTENT_H), scrollY + SCROLL_STEP);
            render();
            return;
        }
        if (charCode === Keys.PAGE_UP) {
            scrollY = Math.max(0, scrollY - PAGE_SCROLL);
            render();
            return;
        }
        if (charCode === Keys.PAGE_DOWN) {
            scrollY = Math.min(Math.max(0, pageTotalHeight - CONTENT_H), scrollY + PAGE_SCROLL);
            render();
            return;
        }
        // Home
        if (charCode === Keys.CTRL_A) {
            scrollY = 0;
            render();
            return;
        }
        // End
        if (charCode === Keys.CTRL_E) {
            scrollY = Math.max(0, pageTotalHeight - CONTENT_H);
            render();
            return;
        }
        // Any printable key focuses URL bar
        if (Keys.isPrintable(charCode)) {
            urlFocused = true;
            urlText = '';
            urlCursor = 0;
        } else {
            return;
        }
    }

    // URL bar input
    if (Keys.isEnter(charCode)) {
        if (urlText.length > 0) {
            // Auto-add https:// if missing
            var navUrl = urlText;
            if (navUrl.indexOf('://') === -1) navUrl = 'https://' + navUrl;
            navigate(navUrl);
        }
        return;
    }
    if (charCode === Keys.ESCAPE) {
        urlFocused = false;
        render();
        return;
    }
    if (Keys.isBackspace(charCode)) {
        if (urlCursor > 0) {
            urlText = urlText.slice(0, urlCursor - 1) + urlText.slice(urlCursor);
            urlCursor--;
        }
        render();
        return;
    }
    if (charCode === Keys.DELETE) {
        if (urlCursor < urlText.length) {
            urlText = urlText.slice(0, urlCursor) + urlText.slice(urlCursor + 1);
        }
        render();
        return;
    }
    if (charCode === Keys.LEFT) {
        if (urlCursor > 0) urlCursor--;
        render();
        return;
    }
    if (charCode === Keys.RIGHT) {
        if (urlCursor < urlText.length) urlCursor++;
        render();
        return;
    }
    if (Keys.isPrintable(charCode)) {
        urlText = urlText.slice(0, urlCursor) + Keys.toChar(charCode) + urlText.slice(urlCursor);
        urlCursor++;
        render();
    }
}
```

- [ ] **Step 2: Implement mouse click handler for links**

Replace the placeholder `handleMouse`:

```js
var lastHoverLink = '';

function handleMouse(mx, my, buttons) {
    // Mouse coordinates are relative to window
    var contentMouseY = my - CONTENT_Y + scrollY;

    // Hover: show link URL in status bar
    var hoveredLink = '';
    for (var i = 0; i < pageLinks.length; i++) {
        var link = pageLinks[i];
        if (mx >= link.x && mx <= link.x + link.w &&
            contentMouseY >= link.y && contentMouseY <= link.y + link.h) {
            hoveredLink = link.href;
            break;
        }
    }
    if (hoveredLink !== lastHoverLink) {
        lastHoverLink = hoveredLink;
        if (hoveredLink) {
            statusText = resolveUrl(hoveredLink, currentUrl);
        } else {
            statusText = currentUrl || 'Type a URL and press Enter';
        }
        drawStatusBar();
        win.flush();
    }

    // Click
    if (buttons & 1) {
        // Click in URL bar?
        if (my < URL_BAR_H) {
            urlFocused = true;
            render();
            return;
        }
        // Click on a link?
        if (hoveredLink) {
            var fullUrl = resolveUrl(hoveredLink, currentUrl);
            if (fullUrl) {
                urlText = fullUrl;
                urlCursor = fullUrl.length;
                navigate(fullUrl);
            }
        } else {
            urlFocused = false;
            render();
        }
    }
}
```

- [ ] **Step 3: Implement scroll handler**

Replace the placeholder `handleScroll`:

```js
function handleScroll(delta) {
    scrollY += delta * SCROLL_STEP;
    if (scrollY < 0) scrollY = 0;
    var maxScroll = Math.max(0, pageTotalHeight - CONTENT_H);
    if (scrollY > maxScroll) scrollY = maxScroll;
    render();
}
```

- [ ] **Step 4: Commit**

```bash
git add src/jsos/demo_browser_v2.jsos
git commit -m "feat(browser_v2): keyboard, mouse link clicking, scroll handlers"
```

---

### Task 12: Integration — Build and Test

**Files:**
- All modified files

- [ ] **Step 1: Build the kernel**

```bash
cargo build --target x86_64-os.json
```

Expected: compiles without errors. If there are compile errors, they'll be from the Rust registration lines — fix typos in the `include_str!` paths.

- [ ] **Step 2: Run in QEMU and test manually**

```bash
sh run_qemu.sh target/x86_64-os/debug/os
```

Test checklist:
1. Type `browser2` in shell — browser window appears
2. URL bar shows default Wikipedia URL
3. Press Enter — page loads, text renders
4. Scroll with mouse wheel or arrow keys
5. Click a link — navigates to new page
6. Press `[` — goes back
7. Type a new URL in the URL bar
8. Ctrl+Q quits

- [ ] **Step 3: Run existing tests to verify no regressions**

```bash
cargo test --target x86_64-os.json
```

Expected: all existing tests pass. The `demo_browser.jsos` update (Task 7) ensures the old app still works.

- [ ] **Step 4: Commit any fixes**

```bash
git add -A
git commit -m "fix: integration fixes for demo_browser_v2"
```

---

### Task 13: Polish — Error States, Edge Cases

**Files:**
- Modify: `src/jsos/demo_browser_v2.jsos`

- [ ] **Step 1: Add error page rendering**

After the `catch(e)` in `navigate`, render an error page with the engine instead of just a status message:

```js
    // In the catch block, after setting statusText:
    var errorHtml = '<html><body style="background-color:#0a0a1a;color:#dc4646;padding:8px">' +
        '<h1>Failed to load page</h1>' +
        '<p style="color:#c8dcff">' + url + '</p>' +
        '<p style="color:#dc4646">' + String(e).replace(/</g, '&lt;') + '</p>' +
        '<p style="color:#607090">Press [ to go back, or type a new URL.</p>' +
        '</body></html>';
    var errDom = dom.parseHTML(errorHtml);
    var errRules = css.parseCSS(errDom._embeddedCSS || '');
    css.applyStyles(errDom, errRules, {});
    var errResult = layout.layoutDOM(errDom, { x: 0, y: CONTENT_Y, w: W, h: CONTENT_H });
    pageCmds = errResult.cmds;
    pageLinks = errResult.links;
    pageTotalHeight = errResult.totalHeight;
```

- [ ] **Step 2: Handle empty page (no content)**

In `drawContent`, if pageCmds is empty and we're not loading and we have a currentUrl, show "Page is empty":

```js
    if (pageCmds.length === 0 && !isLoading && currentUrl) {
        win.text(8, CONTENT_Y + 8, 'Page returned no renderable content.', T.dim[0], T.dim[1], T.dim[2]);
        return;
    }
```

- [ ] **Step 3: Commit**

```bash
git add src/jsos/demo_browser_v2.jsos
git commit -m "feat(browser_v2): error page rendering and empty page handling"
```
