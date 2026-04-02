// dom.js – HTML tokeniser + DOM tree builder for JSOS Browser Engine
'use strict';

// ── Node constructors ────────────────────────────────────────────────────────

function createElement(tagName, attrs) {
    return { nodeType: 1, tagName: tagName.toLowerCase(), attrs: attrs || {}, children: [], parent: null, computedStyle: {} };
}

function createTextNode(raw) {
    return {
        nodeType: 3,
        text: raw.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&nbsp;/g, ' ').replace(/&#39;/g, "'"),
        parent: null,
        computedStyle: {}
    };
}

// ── Attribute parser ─────────────────────────────────────────────────────────

// Parse "key="val" key2='v2' bare" → { key:'val', key2:'v2', bare:'' }
function parseAttrs(src) {
    const attrs = {};
    const re = /([\w-]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|(\S+)))?/g;
    let m;
    while ((m = re.exec(src)) !== null) {
        const name = m[1].toLowerCase();
        const val  = m[2] !== undefined ? m[2] :
                     m[3] !== undefined ? m[3] :
                     m[4] !== undefined ? m[4] : '';
        attrs[name] = val;
    }
    if (attrs.style) { attrs._inlineStyle = parseInlineStyle(attrs.style); }
    return attrs;
}

function parseInlineStyle(s) {
    const out = {};
    s.split(';').forEach(function(decl) {
        var idx = decl.indexOf(':');
        if (idx === -1) return;
        var prop = decl.slice(0, idx).trim();
        var val  = decl.slice(idx + 1).trim();
        if (prop && val) {
            var camel = prop.replace(/-([a-z])/g, function(_, c) { return c.toUpperCase(); });
            out[camel] = val;
        }
    });
    return out;
}

// ── Tokeniser ────────────────────────────────────────────────────────────────

var SELF_CLOSING = ['br','hr','img','input','meta','link','area','base','col','embed','param','source','track','wbr'];

function tokenize(html) {
    var tokens = [];
    var i = 0;
    var len = html.length;

    while (i < len) {
        if (html[i] !== '<') {
            // Text run
            var end = html.indexOf('<', i);
            if (end === -1) end = len;
            var text = html.slice(i, end);
            var trimmed = text.replace(/^\s+|\s+$/g, '');
            // Preserve single spaces between tags
            var ws = text.replace(/[^\s]/g, '').length > 0 && trimmed === '' ? ' ' : trimmed;
            if (ws && ws !== ' ') tokens.push({ type: 'text', text: trimmed });
            else if (ws === ' ' && tokens.length > 0) { /* skip leading/trailing whitespace */ }
            i = end;
            continue;
        }

        // Find end of tag (naively, ignoring strings with > inside attributes)
        var closeAngle = html.indexOf('>', i + 1);
        if (closeAngle === -1) { i++; continue; }
        var raw = html.slice(i + 1, closeAngle).trim();
        i = closeAngle + 1;

        if (raw.slice(0, 3) === '!--') {
            // Comment — skip to -->
            var commentEnd = html.indexOf('-->', i);
            if (commentEnd !== -1) i = commentEnd + 3;
            continue;
        }
        if (raw[0] === '!') continue; // DOCTYPE etc.

        if (raw[0] === '/') {
            // Closing tag
            tokens.push({ type: 'close', name: raw.slice(1).trim().split(/\s/)[0].toLowerCase() });
            continue;
        }

        // Opening tag (possibly self-closing)
        var selfClose = raw[raw.length - 1] === '/';
        if (selfClose) raw = raw.slice(0, -1).trim();
        var spaceIdx = raw.search(/[\s]/);
        var tagName  = spaceIdx === -1 ? raw.toLowerCase() : raw.slice(0, spaceIdx).toLowerCase();
        var attrSrc  = spaceIdx === -1 ? '' : raw.slice(spaceIdx + 1);
        var attrs    = parseAttrs(attrSrc);
        tokens.push({ type: 'open', name: tagName, attrs: attrs, selfClose: selfClose || SELF_CLOSING.indexOf(tagName) !== -1 });
    }
    return tokens;
}

// ── Tree builder ─────────────────────────────────────────────────────────────

function buildTree(tokens) {
    var root  = createElement('#document', {});
    var stack = [root];

    for (var i = 0; i < tokens.length; i++) {
        var tok = tokens[i];
        var current = stack[stack.length - 1];

        if (tok.type === 'text') {
            var tn = createTextNode(tok.text);
            tn.parent = current;
            current.children.push(tn);

        } else if (tok.type === 'open') {
            var node = createElement(tok.name, tok.attrs);
            node.parent = current;
            current.children.push(node);
            if (!tok.selfClose) stack.push(node);

        } else if (tok.type === 'close') {
            // Pop back to matching open tag
            for (var k = stack.length - 1; k >= 1; k--) {
                if (stack[k].tagName === tok.name) {
                    stack.length = k;
                    break;
                }
            }
        }
    }
    return root;
}

// ── Public API ────────────────────────────────────────────────────────────────

function parseHTML(html) {
    // Strip <head>...</head> content but keep <style>
    var styleContent = '';
    var styleMatch = html.match(/<style[^>]*>([\s\S]*?)<\/style>/i);
    if (styleMatch) styleContent = styleMatch[1];

    var tokens = tokenize(html);
    var tree   = buildTree(tokens);
    tree._embeddedCSS = styleContent;
    return tree;
}

module.exports = { parseHTML: parseHTML, createElement: createElement, createTextNode: createTextNode };
