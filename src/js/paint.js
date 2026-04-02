// paint.js – Render layout commands to os.window for JSOS Browser Engine
'use strict';

/**
 * Paint a list of draw commands (produced by layout.layoutDOM) into a window.
 *
 * @param {Array}  cmds   Array of { type:'rect'|'text', x, y, w, h, r, g, b, text?, bold? }
 * @param {number} winId  Window ID returned by os.window.create()
 */
function paint(cmds, winId) {
    for (var i = 0; i < cmds.length; i++) {
        var cmd = cmds[i];
        if (cmd.type === 'rect') {
            os.window.drawRect(winId, cmd.x, cmd.y, cmd.w, cmd.h, cmd.r, cmd.g, cmd.b);
        } else if (cmd.type === 'text') {
            os.window.drawString(winId, cmd.text, cmd.x, cmd.y, cmd.r, cmd.g, cmd.b);
            // Simulate bold by drawing the string offset by one pixel
            if (cmd.bold) {
                os.window.drawString(winId, cmd.text, cmd.x + 1, cmd.y, cmd.r, cmd.g, cmd.b);
            }
        }
    }
}

module.exports = { paint: paint };
