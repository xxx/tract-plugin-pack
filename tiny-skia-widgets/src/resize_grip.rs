//! Corner-resize grip widget.
//!
//! Renders 3 diagonal lines at the bottom-right corner of a panel (classic
//! drag-to-resize affordance). Provides hit-testing so the editor can detect
//! a click in the grip region and translate mouse drag into window resize.

use tiny_skia::{Color, Paint, PixmapMut, Rect, Transform};

/// Square hit / draw region in pixels.
pub const GRIP_SIZE: i32 = 14;

pub struct ResizeGrip;

impl ResizeGrip {
    /// Returns true if `(mx, my)` falls inside the grip's bottom-right square.
    pub fn hit_test(
        panel_x: i32,
        panel_y: i32,
        panel_w: i32,
        panel_h: i32,
        mx: f32,
        my: f32,
    ) -> bool {
        let mxi = mx as i32;
        let myi = my as i32;
        mxi >= panel_x + panel_w - GRIP_SIZE
            && mxi < panel_x + panel_w
            && myi >= panel_y + panel_h - GRIP_SIZE
            && myi < panel_y + panel_h
    }

    /// Draw the grip at the bottom-right corner.
    pub fn draw(
        pixmap: &mut PixmapMut<'_>,
        panel_x: i32,
        panel_y: i32,
        panel_w: i32,
        panel_h: i32,
        color: Color,
    ) {
        let x = panel_x + panel_w - GRIP_SIZE;
        let y = panel_y + panel_h - GRIP_SIZE;
        // 3 diagonal pixel runs at offsets 0, 4, 8 from the inner corner.
        for offset in [0_i32, 4, 8] {
            let len = GRIP_SIZE - 4 - offset;
            if len <= 0 {
                continue;
            }
            for i in 0..len {
                let px = x + GRIP_SIZE - 1 - i;
                let py = y + GRIP_SIZE - 1 - i - offset;
                draw_pixel(pixmap, px, py, color);
            }
        }
    }
}

fn draw_pixel(pixmap: &mut PixmapMut<'_>, x: i32, y: i32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.blend_mode = if color.is_opaque() {
        tiny_skia::BlendMode::Source
    } else {
        tiny_skia::BlendMode::SourceOver
    };
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, 1.0, 1.0) {
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_inside_grip_corner() {
        // Panel from (0,0) to (200, 100). Grip at bottom-right 14×14.
        assert!(ResizeGrip::hit_test(0, 0, 200, 100, 195.0, 95.0));
        assert!(ResizeGrip::hit_test(0, 0, 200, 100, 199.99, 99.99));
        // Top-left corner of grip
        assert!(ResizeGrip::hit_test(0, 0, 200, 100, 186.0, 86.0));
    }

    #[test]
    fn hit_test_outside_grip() {
        // Center of panel: not in grip
        assert!(!ResizeGrip::hit_test(0, 0, 200, 100, 100.0, 50.0));
        // Just past right/bottom edge of grip
        assert!(!ResizeGrip::hit_test(0, 0, 200, 100, 200.0, 100.0));
        // Top edge of grip's row, but well inside panel
        assert!(!ResizeGrip::hit_test(0, 0, 200, 100, 100.0, 85.0));
    }

    #[test]
    fn render_no_panic() {
        let mut pixmap = tiny_skia::Pixmap::new(200, 100).unwrap();
        let mut pmut = pixmap.as_mut();
        ResizeGrip::draw(&mut pmut, 0, 0, 200, 100, Color::WHITE);
    }

    #[test]
    fn render_smaller_than_grip_no_panic() {
        // Edge case: panel narrower than GRIP_SIZE
        let mut pixmap = tiny_skia::Pixmap::new(40, 40).unwrap();
        let mut pmut = pixmap.as_mut();
        ResizeGrip::draw(&mut pmut, 0, 0, 40, 40, Color::WHITE);
    }
}
