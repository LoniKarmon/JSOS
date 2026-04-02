use crate::framebuffer;

pub struct Graphics;

impl Graphics {
    /// Clears the screen with the current background color.
    pub fn clear_screen() {
        framebuffer::clear_screen();
    }

    /// Sets the text foreground color (RGB).
    pub fn set_foreground_color(r: u8, g: u8, b: u8) {
        framebuffer::set_foreground_color(r, g, b);
    }

    /// Draws a styled line using Bresenham's algorithm.
    pub fn draw_line(x0: isize, y0: isize, x1: isize, y1: isize, r: u8, g: u8, b: u8) {
        framebuffer::draw_line(x0, y0, x1, y1, r, g, b);
    }

    /// Draws a styled circle.
    pub fn draw_circle(cx: isize, cy: isize, radius: isize, r: u8, g: u8, b: u8) {
        framebuffer::draw_circle(cx, cy, radius, r, g, b);
    }

    /// Draws a hollow wireframe rectangle.
    pub fn draw_rect(x: usize, y: usize, width: usize, height: usize, r: u8, g: u8, b: u8) {
        Self::draw_line(
            x as isize,
            y as isize,
            (x + width) as isize,
            y as isize,
            r,
            g,
            b,
        );
        Self::draw_line(
            (x + width) as isize,
            y as isize,
            (x + width) as isize,
            (y + height) as isize,
            r,
            g,
            b,
        );
        Self::draw_line(
            (x + width) as isize,
            (y + height) as isize,
            x as isize,
            (y + height) as isize,
            r,
            g,
            b,
        );
        Self::draw_line(
            x as isize,
            (y + height) as isize,
            x as isize,
            y as isize,
            r,
            g,
            b,
        );
    }

    /// Draws a solid filled rectangle.
    pub fn fill_rect(x: usize, y: usize, width: usize, height: usize, r: u8, g: u8, b: u8) {
        framebuffer::fill_rect(x, y, width, height, r, g, b);
    }

    /// Renders a simple 3D wireframe cube projected onto the 2D plane.
    pub fn draw_wireframe_cube(
        cx: isize,
        cy: isize,
        size: isize,
        dx: isize,
        dy: isize,
        edge_r: u8,
        edge_g: u8,
        edge_b: u8,
    ) {
        // Front face
        Self::draw_line(
            cx - size,
            cy - size,
            cx + size,
            cy - size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx - size,
            cy + size,
            cx + size,
            cy + size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx - size,
            cy - size,
            cx - size,
            cy + size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx + size,
            cy - size,
            cx + size,
            cy + size,
            edge_r,
            edge_g,
            edge_b,
        );

        // Back face
        let (bx, by) = (cx + dx, cy + dy);
        Self::draw_line(
            bx - size,
            by - size,
            bx + size,
            by - size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            bx - size,
            by + size,
            bx + size,
            by + size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            bx - size,
            by - size,
            bx - size,
            by + size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            bx + size,
            by - size,
            bx + size,
            by + size,
            edge_r,
            edge_g,
            edge_b,
        );

        // Connecting edges
        Self::draw_line(
            cx - size,
            cy - size,
            bx - size,
            by - size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx + size,
            cy - size,
            bx + size,
            by - size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx - size,
            cy + size,
            bx - size,
            by + size,
            edge_r,
            edge_g,
            edge_b,
        );
        Self::draw_line(
            cx + size,
            cy + size,
            bx + size,
            by + size,
            edge_r,
            edge_g,
            edge_b,
        );
    }
}
