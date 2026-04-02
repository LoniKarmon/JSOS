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
    if (sel.startsWith('#')) return 100;
    if (sel.startsWith('.')) return 10;
    return 1;
}

function matches(node, sel) {
    if (!node || node.nodeType !== 1) return false;
    sel = sel.trim();
    if (sel === '*' || sel === '') return true;
    if (sel[0] === '#') return node.attrs.id === sel.slice(1);
    if (sel[0] === '.') {
        var classes = (node.attrs['class'] || '').split(/\s+/);
        return classes.indexOf(sel.slice(1)) !== -1;
    }
    return node.tagName === sel.toLowerCase();
}

// ── Default tag styles ────────────────────────────────────────────────────────

var BLOCK_TAGS = {
    'html':1,'body':1,'div':1,'p':1,'h1':1,'h2':1,'h3':1,'h4':1,'h5':1,'h6':1,
    'ul':1,'ol':1,'li':1,'section':1,'article':1,'header':1,'footer':1,'nav':1,'main':1
};

var TAG_DEFAULTS = {
    'h1': { fontSize: '20px', fontWeight: 'bold', marginTop: '8px', marginBottom: '4px' },
    'h2': { fontSize: '17px', fontWeight: 'bold', marginTop: '6px', marginBottom: '3px' },
    'h3': { fontSize: '15px', fontWeight: 'bold', marginTop: '4px', marginBottom: '2px' },
    'p':  { marginTop: '4px', marginBottom: '4px' },
    'li': { marginTop: '2px', marginBottom: '2px' }
};

// ── Style cascade ──────────────────────────────────────────────────────────────

var INHERITABLE = ['color', 'fontSize', 'fontWeight', 'fontFamily', 'lineHeight'];

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
        if (matches(root, rule.selector)) {
            Object.keys(rule.properties).forEach(function(k) { computed[k] = rule.properties[k]; });
        }
    });

    // Inline style wins
    if (root.attrs._inlineStyle) {
        Object.keys(root.attrs._inlineStyle).forEach(function(k) { computed[k] = root.attrs._inlineStyle[k]; });
    }

    // display default
    if (!computed.display) {
        computed.display = BLOCK_TAGS[root.tagName] ? 'block' : 'inline';
    }

    root.computedStyle = computed;

    // Build inherited context for children
    var childParent = {};
    INHERITABLE.forEach(function(p) { if (computed[p]) childParent[p] = computed[p]; });

    root.children.forEach(function(child) { applyStyles(child, rules, childParent); });
}

module.exports = { parseCSS: parseCSS, applyStyles: applyStyles };
