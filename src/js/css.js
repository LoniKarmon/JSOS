// css.js – CSS parser + style cascade for JSOS Browser Engine
'use strict';

// ── CSS parser ────────────────────────────────────────────────────────────────

function parseCSS(cssText) {
    var rules = [];
    // Strip comments
    cssText = cssText.replace(/\/\*[\s\S]*?\*\//g, '');
    var ruleRe = /([^{]+)\{([^}]*)\}/g;
    var m;
    while ((m = ruleRe.exec(cssText)) !== null) {
        var selectors  = m[1].trim();
        var declBlock  = m[2].trim();
        var properties = parseDeclarations(declBlock);
        selectors.split(',').forEach(function(sel) {
            rules.push({ selector: sel.trim(), properties: properties });
        });
    }
    return rules;
}

function parseDeclarations(block) {
    var out = {};
    block.split(';').forEach(function(decl) {
        var idx = decl.indexOf(':');
        if (idx === -1) return;
        var prop = decl.slice(0, idx).trim();
        var val  = decl.slice(idx + 1).trim();
        if (!prop || !val) return;
        var camel = prop.replace(/-([a-z])/g, function(_, c) { return c.toUpperCase(); });
        out[camel] = val;
    });
    return out;
}

// ── Selector matching ─────────────────────────────────────────────────────────

function specificity(sel) {
    if (!sel) return 0;
    sel = sel.trim();
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

function matchesCompound(node, compound) {
    if (!node || node.nodeType !== 1) return false;
    compound = compound.trim();
    if (compound === '*') return true;
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
        } else if (p !== '*') {
            if (node.tagName !== p.toLowerCase()) return false;
        }
    }
    return true;
}

function matchesSelector(node, selector) {
    if (!node || node.nodeType !== 1) return false;
    selector = selector.trim();
    var compoundParts = selector.replace(/\s*>\s*/g, ' > ').split(/\s+/);
    if (compoundParts.length === 1) {
        return matchesCompound(node, compoundParts[0]);
    }
    var rightmost = compoundParts[compoundParts.length - 1];
    if (!matchesCompound(node, rightmost)) return false;
    var cur = node;
    for (var i = compoundParts.length - 2; i >= 0; i--) {
        var part = compoundParts[i];
        if (part === '>') continue;
        var isChild = (i + 1 < compoundParts.length && compoundParts[i + 1] === '>');
        if (isChild) {
            cur = cur.parent;
            if (!cur || !matchesCompound(cur, part)) return false;
        } else {
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

// ── Default tag styles ────────────────────────────────────────────────────────

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

// ── Style cascade ──────────────────────────────────────────────────────────────

var INHERITABLE = ['color', 'fontSize', 'fontWeight', 'fontFamily', 'lineHeight', 'textAlign', 'textDecoration', 'whiteSpace', 'listStyleType', 'fontStyle'];

function applyStyles(root, rules, parentStyle) {
    if (!root || root.nodeType !== 1) return;
    parentStyle = parentStyle || {};

    // Inherit from parent
    var computed = {};
    INHERITABLE.forEach(function(p) { if (parentStyle[p]) computed[p] = parentStyle[p]; });

    // Tag defaults
    var tagDef = TAG_DEFAULTS[root.tagName] || {};
    Object.keys(tagDef).forEach(function(k) { computed[k] = tagDef[k]; });

    // Apply rules in ascending specificity
    var sorted = rules.slice().sort(function(a, b) { return specificity(a.selector) - specificity(b.selector); });
    sorted.forEach(function(rule) {
        if (matchesSelector(root, rule.selector)) {
            Object.keys(rule.properties).forEach(function(k) { computed[k] = rule.properties[k]; });
        }
    });

    // Inline style wins
    if (root.attrs._inlineStyle) {
        Object.keys(root.attrs._inlineStyle).forEach(function(k) { computed[k] = root.attrs._inlineStyle[k]; });
    }

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

    root.computedStyle = computed;

    // Build inherited context for children
    var childParent = {};
    INHERITABLE.forEach(function(p) { if (computed[p]) childParent[p] = computed[p]; });

    root.children.forEach(function(child) { applyStyles(child, rules, childParent); });
}

module.exports = { parseCSS: parseCSS, applyStyles: applyStyles, matchesSelector: matchesSelector, matchesCompound: matchesCompound };
