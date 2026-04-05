# demo_browser_v2 Design Spec

A real web browser for JSOS capable of fetching and rendering live web pages (Wikipedia, etc.) with scrolling and clickable links.

## Overview

- **Window:** 900x600, with URL bar (30px top), content area (540px), status bar (30px bottom)
- **Engine:** Upgraded dom/css/layout/paint pipeline to handle real-world HTML
- **Networking:** `os.fetch()` with Wikipedia mobile API auto-detection
- **Interaction:** Keyboard URL entry, mouse scroll, clickable links, back navigation

## CSS Engine Upgrades (`css.js`)

### New Selectors
- Descendant: `div p`, `.card h2`
- Child: `div > p`
- Multi-class: `.foo.bar`
- Attribute: `[href]`, `[class="x"]`
- Comma-grouped: `h1, h2, h3 { ... }` (verify existing support)
- Pseudo-classes: `:first-child`, `:last-child`

### New Properties
- `display`: `block`, `inline`, `inline-block`, `none`, `table`, `table-row`, `table-cell`, `list-item`
- `text-decoration`: `underline`, `none`
- `list-style-type`: `disc`, `decimal`, `none`
- `border` shorthand: `1px solid #color`
- `max-width`, `min-width`
- `text-align`: `left`, `center`, `right`
- `vertical-align`: `top`, `middle`, `bottom` (table cells)
- `white-space`: `normal`, `nowrap`, `pre`
- `overflow`: `hidden` (clip)

### Specificity
Existing system (ID:100, class:10, tag:1) extended with combinator awareness.

## Layout Engine Upgrades (`layout.js`)

### Inline Layout
- `<a>`, `<span>`, `<em>`, `<strong>`, `<code>`, `<b>`, `<i>`, `<u>` flow horizontally on the same line.
- Line cursor (x, y) advances horizontally, wraps to next line when exceeding container width.
- Each inline run of a link records a hit-box `{x, y, w, h, href}`.

### List Support
- `<ul>`, `<ol>`, `<li>` — indent ~20px, prepend bullet or number.
- Nested lists increase indent level.

### Table Layout
- `<table>`, `<tr>`, `<td>`/`<th>` — equal-split column widths across available width.
- Each cell is a mini block layout context. Rows vertical, cells horizontal.
- `<th>` renders bold.
- 1px cell borders when CSS/attribute specifies.

### Text Alignment
- `center`: offset by `(containerW - textW) / 2`
- `right`: offset by `containerW - textW`

### Additional Block Elements
- `<hr>` — horizontal line with margin.
- `<blockquote>` — indented with left border line.
- `<pre>`/`<code>` — preserve whitespace, background highlight.

### Return Value Change
`layoutDOM()` returns `{cmds, links, totalHeight}` instead of just `cmds`.
- `links`: array of `{x, y, w, h, href}` for every `<a>`.
- `totalHeight`: full document height for scroll range.

## Paint Engine Upgrades (`paint.js`)

### Scroll Viewport
`paint(cmds, winId, scrollY)` — offset all commands by `-scrollY`, cull commands outside viewport.

### New Command Types
- `{type: 'line', x0, y0, x1, y1, r, g, b}` — for `<hr>`, table borders, blockquote bars.
- `{type: 'underline', x, y, w, r, g, b}` — link underlines, 2px below text baseline.

## dom.js

No changes needed. Existing parser handles tags, attributes, inline styles.

## Browser App (`demo_browser_v2.jsos`)

### UI Layout (900x600)
- **Top bar (30px):** URL text field, "Go" indicator.
- **Content area (540px):** Rendered HTML with scroll.
- **Status bar (30px):** Loading state, hovered link URL, scroll position.

### Navigation
- Keyboard-driven URL bar: type URL, Backspace deletes, Enter fetches.
- Wikipedia detection: `wikipedia.org` URLs rewritten to `en.m.wikipedia.org` mobile path.
- Back history: array of visited URLs. Press `[` to go back.

### Scrolling
- Mouse wheel via `handlers.scroll(delta)` — adjust scrollY, clamp to `[0, totalHeight - viewportH]`.
- Page Up / Page Down: scroll by viewport height.
- Arrow Up / Down: scroll by a few lines.

### Link Clicking
- On mouse click, test `(clickX, clickY + scrollY)` against `links` array.
- Hit: push current URL to history, navigate to href (resolve relative URLs against current origin).

### Fetch Pipeline
1. User enters URL, status bar shows "Loading..."
2. `await os.fetch(url)` returns HTML string
3. Pre-process: strip `<script>`, `<noscript>`, Wikipedia nav/sidebar elements
4. `dom.parseHTML()` -> `css.applyStyles()` -> `layout.layoutDOM()` -> cache
5. `paint()` with `scrollY = 0`

### Keyboard Controls
| Key | Action |
|-----|--------|
| `Ctrl+L` | Focus URL bar |
| `Ctrl+Q` / `Escape` | Quit |
| `Up` / `Down` | Scroll |
| `Page Up` / `Page Down` | Scroll fast |
| `Home` / `End` | Top / bottom |
| `Enter` (URL bar) | Navigate |
| `[` | Back |

## Backward Compatibility

- `layout.layoutDOM()` return value changes from array to object. Existing `demo_browser.jsos` must be updated to use `.cmds`.
- `paint.paint()` gains optional `scrollY` param (defaults to 0), so existing callers are unaffected.
- `css.js` additions are purely additive — existing selectors continue to work.
