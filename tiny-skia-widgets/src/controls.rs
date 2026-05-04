//! Composite widget drawing functions: buttons, sliders, stepped selectors.

use tiny_skia::{Color, Pixmap};

use crate::primitives::*;
use crate::text::TextRenderer;

// ---------------------------------------------------------------------------
// Composite widgets
// ---------------------------------------------------------------------------

/// Draw a button with a centred label.
///
/// `hovered` brightens the background slightly; `pressed` darkens it.
#[allow(clippy::too_many_arguments)]
pub fn draw_button(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    active: bool,
    _hovered: bool,
) {
    let bg = if active {
        color_accent()
    } else {
        color_control_bg()
    };

    draw_rect(pixmap, x, y, w, h, bg);
    draw_rect_outline(pixmap, x, y, w, h, color_border(), 1.0);

    // Centre the label inside the button.
    let text_size = (h * 0.5).max(10.0);
    let tw = text_renderer.text_width(label, text_size);
    let tx = x + (w - tw) * 0.5;
    let ty = y + (h + text_size) * 0.5 - 2.0; // approximate baseline offset
    let text_color = if active {
        Color::from_rgba8(0x1a, 0x1c, 0x22, 0xff) // dark text on accent bg
    } else {
        color_text()
    };
    text_renderer.draw_text(pixmap, tx, ty, label, text_size, text_color);
}

/// Draw a horizontal slider with a fill bar, a left-aligned label, and a
/// right-aligned value string.
///
/// `normalized_value` should be in 0.0..=1.0.
///
/// When `editing_text` is `Some(buf)`, the value readout is replaced with a
/// highlighted edit field displaying the buffer, and a caret is drawn if
/// `caret_on` is true.
#[allow(clippy::too_many_arguments)]
pub fn draw_slider(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value_text: &str,
    normalized_value: f32,
    editing_text: Option<&str>,
    caret_on: bool,
) {
    let nv = normalized_value.clamp(0.0, 1.0);

    // Track background
    draw_rect(pixmap, x, y, w, h, color_control_bg());
    draw_rect_outline(pixmap, x, y, w, h, color_border(), 1.0);

    // Fill bar
    let fill_w = (w - 2.0) * nv;
    if fill_w > 0.0 {
        draw_rect(pixmap, x + 1.0, y + 1.0, fill_w, h - 2.0, color_accent());
    }

    // Label (left-aligned, vertically centred)
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;
    let pad = 6.0;
    text_renderer.draw_text(pixmap, x + pad, text_y, label, text_size, color_text());

    // Value readout: buffer + caret when editing, otherwise formatted value
    if let Some(buf) = editing_text {
        let ref_w = text_renderer.text_width("-999.99", text_size);
        let box_w = ref_w + 12.0;
        let box_h = text_size + 6.0;
        let box_x = x + w - box_w - pad;
        let box_y = y + (h - box_h) * 0.5;
        draw_rect(pixmap, box_x, box_y, box_w, box_h, color_edit_bg());
        draw_rect_outline(pixmap, box_x, box_y, box_w, box_h, color_accent(), 1.0);

        let buf_x = box_x + 6.0;
        text_renderer.draw_text(pixmap, buf_x, text_y, buf, text_size, color_text());

        if caret_on {
            let buf_w = text_renderer.text_width(buf, text_size);
            let caret_x = buf_x + buf_w + 1.0;
            let caret_y = box_y + 3.0;
            let caret_h = box_h - 6.0;
            draw_rect(pixmap, caret_x, caret_y, 1.0, caret_h, color_text());
        }
    } else {
        let vw = text_renderer.text_width(value_text, text_size);
        text_renderer.draw_text(
            pixmap,
            x + w - vw - pad,
            text_y,
            value_text,
            text_size,
            color_text(),
        );
    }
}

/// Draw a segmented control (stepped selector).
///
/// Each segment is an equal-width button; the one at `active_index` is
/// highlighted with the accent colour.
#[allow(clippy::too_many_arguments)]
pub fn draw_stepped_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: &[&str],
    active_index: usize,
) {
    if options.is_empty() {
        return;
    }

    let seg_w = w / options.len() as f32;
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;

    for (i, &opt) in options.iter().enumerate() {
        let sx = x + i as f32 * seg_w;
        let is_active = i == active_index;

        let bg = if is_active {
            color_accent()
        } else {
            color_control_bg()
        };
        let fg = if is_active {
            Color::from_rgba8(0x10, 0x10, 0x10, 0xff)
        } else {
            color_text()
        };

        draw_rect(pixmap, sx, y, seg_w, h, bg);
        draw_rect_outline(pixmap, sx, y, seg_w, h, color_border(), 1.0);

        let tw = text_renderer.text_width(opt, text_size);
        let tx = sx + (seg_w - tw) * 0.5;
        text_renderer.draw_text(pixmap, tx, text_y, opt, text_size, fg);
    }
}

// ---------------------------------------------------------------------------
// Outline variants — transparent background with colored border
// ---------------------------------------------------------------------------

/// Draw an outline button: transparent background, colored border, centered label.
/// Active state uses a brighter border and subtle fill.
/// `border_color` and `text_color` are caller-supplied (theme-dependent).
#[allow(clippy::too_many_arguments)]
pub fn draw_outline_button(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    active: bool,
    border_color: Color,
    text_color: Color,
    active_border_color: Color,
    active_fill_color: Color,
) {
    let border = if active {
        active_border_color
    } else {
        border_color
    };
    if active {
        draw_rect(pixmap, x, y, w, h, active_fill_color);
    }
    draw_rect_outline(pixmap, x, y, w, h, border, 1.0);

    let text_size = (h * 0.5).max(10.0);
    let tw = text_renderer.text_width(label, text_size);
    let tx = x + (w - tw) * 0.5;
    let ty = y + (h + text_size) * 0.5 - 2.0;
    let fg = if active {
        active_border_color
    } else {
        text_color
    };
    text_renderer.draw_text(pixmap, tx, ty, label, text_size, fg);
}

/// Draw an outline stepped selector: transparent segments with colored borders.
/// Active segment gets a brighter border and subtle fill.
#[allow(clippy::too_many_arguments)]
pub fn draw_outline_stepped_selector(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    options: &[&str],
    active_index: usize,
    border_color: Color,
    text_color: Color,
    active_border_color: Color,
    active_text_color: Color,
    active_fill_color: Color,
) {
    if options.is_empty() {
        return;
    }

    let seg_w = w / options.len() as f32;
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;

    for (i, &opt) in options.iter().enumerate() {
        let sx = x + i as f32 * seg_w;
        let is_active = i == active_index;

        let border = if is_active {
            active_border_color
        } else {
            border_color
        };
        let fg = if is_active {
            active_text_color
        } else {
            text_color
        };

        if is_active {
            draw_rect(pixmap, sx, y, seg_w, h, active_fill_color);
        }
        draw_rect_outline(pixmap, sx, y, seg_w, h, border, 1.0);

        let tw = text_renderer.text_width(opt, text_size);
        let tx = sx + (seg_w - tw) * 0.5;
        text_renderer.draw_text(pixmap, tx, text_y, opt, text_size, fg);
    }
}

/// Draw an outline slider: transparent track with colored border and fill bar.
///
/// When `editing_text` is `Some(buf)`, the value readout is replaced with a
/// highlighted edit field displaying the buffer, and a caret is drawn if
/// `caret_on` is true.
#[allow(clippy::too_many_arguments)]
pub fn draw_outline_slider(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    value_text: &str,
    normalized_value: f32,
    border_color: Color,
    text_color: Color,
    fill_color: Color,
    editing_text: Option<&str>,
    caret_on: bool,
) {
    let nv = normalized_value.clamp(0.0, 1.0);

    // Outline track
    draw_rect_outline(pixmap, x, y, w, h, border_color, 1.0);

    // Fill bar
    let fill_w = (w - 2.0) * nv;
    if fill_w > 0.0 {
        draw_rect(pixmap, x + 1.0, y + 1.0, fill_w, h - 2.0, fill_color);
    }

    // Label (left) and value (right)
    let text_size = (h * 0.5).max(10.0);
    let text_y = y + (h + text_size) * 0.5 - 2.0;
    let pad = 6.0;
    text_renderer.draw_text(pixmap, x + pad, text_y, label, text_size, text_color);

    // Value readout: buffer + caret when editing, otherwise formatted value
    if let Some(buf) = editing_text {
        let ref_w = text_renderer.text_width("-999.99", text_size);
        let box_w = ref_w + 12.0;
        let box_h = text_size + 6.0;
        let box_x = x + w - box_w - pad;
        let box_y = y + (h - box_h) * 0.5;
        draw_rect(pixmap, box_x, box_y, box_w, box_h, color_edit_bg());
        draw_rect_outline(pixmap, box_x, box_y, box_w, box_h, color_accent(), 1.0);

        let buf_x = box_x + 6.0;
        text_renderer.draw_text(pixmap, buf_x, text_y, buf, text_size, text_color);

        if caret_on {
            let buf_w = text_renderer.text_width(buf, text_size);
            let caret_x = buf_x + buf_w + 1.0;
            let caret_y = box_y + 3.0;
            let caret_h = box_h - 6.0;
            draw_rect(pixmap, caret_x, caret_y, 1.0, caret_h, text_color);
        }
    } else {
        let vw = text_renderer.text_width(value_text, text_size);
        text_renderer.draw_text(
            pixmap,
            x + w - vw - pad,
            text_y,
            value_text,
            text_size,
            text_color,
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_font::test_font_data;
    use tiny_skia::{Pixmap, PremultipliedColorU8};

    /// Read a single pixel from a pixmap by (x, y) coordinates.
    fn pixel_at(pm: &Pixmap, x: u32, y: u32) -> PremultipliedColorU8 {
        pm.pixels()[(y * pm.width() + x) as usize]
    }

    #[test]
    fn test_draw_button_states() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(200, 50).unwrap();
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            false,
            false,
        );
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            true,
            false,
        );
        draw_button(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            80.0,
            30.0,
            "OK",
            false,
            true,
        );
    }

    #[test]
    fn test_draw_slider() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
            None,
            false,
        );
        // Fill should cover roughly the left half of the slider track.
        let left_px = pixel_at(&pm, 10, 18);
        assert!(left_px.alpha() > 0, "slider fill area should be drawn");
    }

    #[test]
    fn test_draw_slider_clamps_value() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        // Values outside 0..1 should be clamped, not panic.
        draw_slider(
            &mut pm,
            &mut renderer,
            0.0,
            0.0,
            200.0,
            28.0,
            "X",
            "0",
            -0.5,
            None,
            false,
        );
        draw_slider(
            &mut pm,
            &mut renderer,
            0.0,
            0.0,
            200.0,
            28.0,
            "X",
            "0",
            1.5,
            None,
            false,
        );
    }

    #[test]
    fn test_draw_slider_editing_paints_highlight() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm_plain = Pixmap::new(300, 50).unwrap();
        let mut pm_edit = Pixmap::new(300, 50).unwrap();
        draw_slider(
            &mut pm_plain,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
            None,
            false,
        );
        draw_slider(
            &mut pm_edit,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
            Some("-3.0"),
            true,
        );

        // Sample inside the highlight box. The box sits near the right edge
        // of the slider, at roughly (x + w - readout_w - pad). Use a coord
        // clearly inside it regardless of font metrics.
        let sample_x: u32 = 240; // rightward, inside the readout box
        let sample_y: u32 = 19; // vertical middle of the 28px-tall slider

        let plain_px = pm_plain.pixels()[(sample_y * pm_plain.width() + sample_x) as usize];
        let edit_px = pm_edit.pixels()[(sample_y * pm_edit.width() + sample_x) as usize];
        assert!(
            plain_px.red() != edit_px.red()
                || plain_px.green() != edit_px.green()
                || plain_px.blue() != edit_px.blue()
                || plain_px.alpha() != edit_px.alpha(),
            "editing overlay must change the readout-region pixels vs. non-editing"
        );
    }

    #[test]
    fn test_draw_outline_slider_editing_paints_highlight() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm_plain = Pixmap::new(300, 50).unwrap();
        let mut pm_edit = Pixmap::new(300, 50).unwrap();
        let border_c = color_accent();
        let text_c = color_text();
        let fill_c = color_accent();

        draw_outline_slider(
            &mut pm_plain,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
            border_c,
            text_c,
            fill_c,
            None,
            false,
        );
        draw_outline_slider(
            &mut pm_edit,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            "Gain",
            "-3.0 dB",
            0.5,
            border_c,
            text_c,
            fill_c,
            Some("-3.0"),
            true,
        );

        // Sample inside the highlight box.
        let sample_x: u32 = 240;
        let sample_y: u32 = 19;

        let plain_px = pm_plain.pixels()[(sample_y * pm_plain.width() + sample_x) as usize];
        let edit_px = pm_edit.pixels()[(sample_y * pm_edit.width() + sample_x) as usize];
        assert!(
            plain_px.red() != edit_px.red()
                || plain_px.green() != edit_px.green()
                || plain_px.blue() != edit_px.blue()
                || plain_px.alpha() != edit_px.alpha(),
            "editing overlay must change the readout-region pixels vs. non-editing"
        );
    }

    #[test]
    fn test_draw_stepped_selector() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(300, 50).unwrap();
        draw_stepped_selector(
            &mut pm,
            &mut renderer,
            5.0,
            5.0,
            250.0,
            28.0,
            &["Stereo", "Left", "Right"],
            1,
        );
    }

    #[test]
    fn test_draw_stepped_selector_empty_options() {
        let data = test_font_data();
        let mut renderer = TextRenderer::new(&data);
        let mut pm = Pixmap::new(100, 50).unwrap();
        draw_stepped_selector(&mut pm, &mut renderer, 0.0, 0.0, 100.0, 28.0, &[], 0);
    }
}
