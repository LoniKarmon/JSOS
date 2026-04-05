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
            var cmdH = cmd.h || 16;
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
