// dom.js – HTML tokeniser + DOM tree builder for JSOS Browser Engine
'use strict';

// ── Entity decoding ───────────────────────────────────────────────────────────

var NAMED_ENTITIES = {
    'amp':    '&',
    'lt':     '<',
    'gt':     '>',
    'quot':   '"',
    'apos':   "'",
    'nbsp':   ' ',
    'mdash':  '—',
    'ndash':  '–',
    'hellip': '…',
    'laquo':  '«',
    'raquo':  '»',
    'copy':   '©',
    'reg':    '®',
    'trade':  '™',
    'ldquo':  '\u201C',
    'rdquo':  '\u201D',
    'lsquo':  '\u2018',
    'rsquo':  '\u2019',
    'bull':   '•',
    'middot': '·',
    'dagger': '†',
    '39':     "'"
};

function decodeEntities(str) {
    return str.replace(/&([^;]+);/g, function(match, entity) {
        // Named entity
        if (NAMED_ENTITIES[entity]) return NAMED_ENTITIES[entity];
        // Decimal numeric reference &#NNN;
        if (entity[0] === '#' && entity[1] !== 'x' && entity[1] !== 'X') {
            var code = parseInt(entity.slice(1), 10);
            if (!isNaN(code)) return String.fromCharCode(code);
        }
        // Hex numeric reference &#xNN; or &#XNN;
        if (entity[0] === '#' && (entity[1] === 'x' || entity[1] === 'X')) {
            var hexCode = parseInt(entity.slice(2), 16);
            if (!isNaN(hexCode)) return String.fromCharCode(hexCode);
        }
        return match; // Unknown entity: leave as-is
    });
}

// ── Node constructors ────────────────────────────────────────────────────────

function createElement(tagName, attrs) {
    return { nodeType: 1, tagName: tagName.toLowerCase(), attrs: attrs || {}, children: [], parent: null, computedStyle: {} };
}

function createTextNode(raw) {
    return {
        nodeType: 3,
        text: decodeEntities(raw),
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

var SELF_CLOSING = [
    'br', 'hr', 'img', 'input', 'meta', 'link', 'area', 'base',
    'col', 'embed', 'param', 'source', 'track', 'wbr'
];

// Tags whose entire subtree should be stripped (content not rendered)
var STRIP_SUBTREE = [
    'script', 'style', 'nav', 'footer', 'head', 'noscript',
    'template', 'aside'
];

// Table-related block tags that participate in the normal tree
var TABLE_TAGS = [
    'table', 'thead', 'tbody', 'tfoot', 'tr', 'td', 'th', 'colgroup', 'col'
];

function tokenize(html) {
    var tokens = [];
    var i = 0;
    var len = html.length;
    // Depth counter for stripped subtrees
    var stripTag = null;
    var stripDepth = 0;

    while (i < len) {
        if (html[i] !== '<') {
            // Text run
            var end = html.indexOf('<', i);
            if (end === -1) end = len;
            var text = html.slice(i, end);
            i = end;

            if (stripTag) continue; // Inside stripped subtree

            // Collapse whitespace but emit non-empty text
            var trimmed = text.replace(/[ \t\r\n]+/g, ' ').replace(/^ | $/g, '');
            if (trimmed.length > 0) {
                tokens.push({ type: 'text', text: trimmed });
            }
            continue;
        }

        // Handle <!-- comments --> spanning multiple >
        if (html.slice(i, i + 4) === '<!--') {
            var commentEnd = html.indexOf('-->', i + 4);
            if (commentEnd !== -1) i = commentEnd + 3;
            else i = len;
            continue;
        }

        // Find end of tag
        var closeAngle = html.indexOf('>', i + 1);
        if (closeAngle === -1) { i++; continue; }
        var raw = html.slice(i + 1, closeAngle).trim();
        i = closeAngle + 1;

        if (raw[0] === '!') continue; // DOCTYPE / CDATA etc.

        if (raw[0] === '/') {
            // Closing tag
            var closeName = raw.slice(1).trim().split(/[\s/]/)[0].toLowerCase();

            if (stripTag) {
                if (closeName === stripTag) {
                    stripDepth--;
                    if (stripDepth === 0) stripTag = null;
                }
                continue;
            }

            tokens.push({ type: 'close', name: closeName });
            continue;
        }

        // Opening tag (possibly self-closing with trailing /)
        var selfClose = raw[raw.length - 1] === '/';
        if (selfClose) raw = raw.slice(0, -1).trim();
        var spaceIdx = raw.search(/\s/);
        var tagName  = spaceIdx === -1 ? raw.toLowerCase() : raw.slice(0, spaceIdx).toLowerCase();
        var attrSrc  = spaceIdx === -1 ? '' : raw.slice(spaceIdx + 1);
        var isSC     = selfClose || SELF_CLOSING.indexOf(tagName) !== -1;

        // Manage stripped subtrees
        if (stripTag) {
            if (!isSC) {
                if (tagName === stripTag) stripDepth++;
            }
            continue;
        }

        if (STRIP_SUBTREE.indexOf(tagName) !== -1 && !isSC) {
            // Begin stripping
            stripTag = tagName;
            stripDepth = 1;
            continue;
        }

        var attrs = parseAttrs(attrSrc);

        // Inject newline text token for <br>
        if (tagName === 'br') {
            tokens.push({ type: 'text', text: '\n' });
            continue;
        }

        tokens.push({ type: 'open', name: tagName, attrs: attrs, selfClose: isSC });
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

// ── Text extraction helper ────────────────────────────────────────────────────

// Collect text content from a node, collapsing whitespace but preserving
// newlines that originated from <br> elements.
function extractText(node) {
    if (node.nodeType === 3) {
        // Text node: collapse runs of spaces/tabs but keep newlines
        return node.text.replace(/[ \t]+/g, ' ');
    }
    if (node.nodeType !== 1) return '';
    var parts = [];
    for (var i = 0; i < node.children.length; i++) {
        parts.push(extractText(node.children[i]));
    }
    var joined = parts.join('');
    // Collapse multiple consecutive spaces (not newlines)
    return joined.replace(/[ \t]{2,}/g, ' ');
}

// ── Public API ────────────────────────────────────────────────────────────────

function parseHTML(html) {
    // Extract embedded CSS before stripping style tags via tokeniser
    var styleContent = '';
    var styleMatch = html.match(/<style[^>]*>([\s\S]*?)<\/style>/i);
    if (styleMatch) styleContent = styleMatch[1];

    var tokens = tokenize(html);
    var tree   = buildTree(tokens);
    tree._embeddedCSS = styleContent;
    return tree;
}

module.exports = { parseHTML: parseHTML, createElement: createElement, createTextNode: createTextNode, extractText: extractText };
