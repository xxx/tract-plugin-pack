//! Track control strip for vertical mode.
//!
//! Renders track name, color swatch, solo/mute buttons.
//! Returns hit regions for the editor to handle clicks.

use crate::theme;
use tiny_skia_widgets as widgets;

/// Hit region action from a control strip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlAction {
    CycleColor(usize),   // slot index
    ToggleSolo(usize),   // slot index
    ToggleMute(usize),   // slot index
    HoverName(usize),    // slot index — for tooltip on truncated names
}

/// A rectangular hit region with an action.
#[derive(Clone, Debug)]
pub struct ControlHitRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub action: ControlAction,
}

/// Draw a single track's control strip and return hit regions.
///
/// - `pixmap`: target pixmap
/// - `tr`: text renderer
/// - `x, y, w, h`: bounds for this strip
/// - `slot_index`: which slot this strip is for
/// - `track_name`: display name
/// - `color`: ARGB display color
/// - `solo`: solo state
/// - `mute`: mute state
/// - `scale`: UI scale factor
#[allow(clippy::too_many_arguments)]
pub fn draw_control_strip(
    pixmap: &mut tiny_skia::Pixmap,
    tr: &mut tiny_skia_widgets::TextRenderer,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    slot_index: usize,
    track_name: &str,
    color: u32,
    solo: bool,
    mute: bool,
    scale: f32,
) -> Vec<ControlHitRegion> {
    let mut regions = Vec::new();

    // Background
    tiny_skia_widgets::draw_rect(pixmap, x, y, w, h, theme::to_color(theme::BG));
    // Right border
    tiny_skia_widgets::draw_rect(
        pixmap,
        x + w - 1.0,
        y,
        1.0,
        h,
        theme::to_color(theme::BORDER),
    );

    // Compact layout when track height is small (many tracks).
    // Full layout: name → swatch → S/M buttons (vertical, ~61px)
    // Compact layout: name + S/M on one row, no swatch (~20px)
    let compact = h < 130.0;

    if compact {
        let pad = 2.0 * scale;
        let font_size = 9.0 * scale;
        let btn_h = (h - 2.0 * pad).max(10.0);
        let btn_w = 16.0 * scale;

        // Full name
        let full_name = if track_name.is_empty() {
            format!("Track {}", slot_index + 1)
        } else {
            track_name.to_string()
        };

        // S/M buttons on the right side
        let border_c = theme::to_color_alpha(color, 0.4);
        let text_c = theme::to_color_alpha(color, 0.5);
        let active_border_c = theme::to_color(color);
        let active_fill_c = theme::to_color_alpha(color, 0.15);

        let gap = 2.0 * scale;
        let mute_x = x + w - pad - btn_w - 1.0;
        let solo_x = mute_x - btn_w - gap;
        let btn_y = y + pad;

        widgets::draw_outline_button(
            pixmap, tr, solo_x, btn_y, btn_w, btn_h, "S", solo,
            border_c, text_c, active_border_c, active_fill_c,
        );
        regions.push(ControlHitRegion {
            x: solo_x, y: btn_y, w: btn_w, h: btn_h,
            action: ControlAction::ToggleSolo(slot_index),
        });

        widgets::draw_outline_button(
            pixmap, tr, mute_x, btn_y, btn_w, btn_h, "M", mute,
            border_c, text_c, active_border_c, active_fill_c,
        );
        regions.push(ControlHitRegion {
            x: mute_x, y: btn_y, w: btn_w, h: btn_h,
            action: ControlAction::ToggleMute(slot_index),
        });

        // Track name to the left of buttons, vertically centered
        let swatch_w = 5.0 * scale;
        let name_x = x + swatch_w + pad;
        let max_text_w = solo_x - name_x - pad;
        let (display_name, truncated) = truncate_name(&full_name, max_text_w, font_size, tr);
        let name_y = y + (h - font_size) / 2.0;
        tr.draw_text(
            pixmap, name_x, name_y + font_size,
            &display_name, font_size, theme::to_color(color),
        );
        if truncated {
            regions.push(ControlHitRegion {
                x, y, w: solo_x - x, h,
                action: ControlAction::HoverName(slot_index),
            });
        }

        // Color swatch: thin vertical bar on far left
        tiny_skia_widgets::draw_rect(pixmap, x, y, swatch_w, h, theme::to_color(color));
        regions.push(ControlHitRegion {
            x, y, w: swatch_w + pad, h,
            action: ControlAction::CycleColor(slot_index),
        });
    } else {
        // Full layout
        let pad = 6.0 * scale;
        let font_size = 11.0 * scale;
        let btn_h = 18.0 * scale;
        let btn_w = 24.0 * scale;
        let swatch_size = 14.0 * scale;

        let mut cy = y + pad;

        // Track name (centered, truncated with ellipsis if too wide)
        let full_name = if track_name.is_empty() {
            format!("Track {}", slot_index + 1)
        } else {
            track_name.to_string()
        };
        let max_text_w = w - 2.0 * pad;
        let (display_name, truncated) = truncate_name(&full_name, max_text_w, font_size, tr);
        let text_w = tr.text_width(&display_name, font_size);
        let name_y = cy;
        tr.draw_text(
            pixmap,
            x + (w - text_w) / 2.0,
            cy + font_size,
            &display_name,
            font_size,
            theme::to_color(color),
        );
        if truncated {
            regions.push(ControlHitRegion {
                x, y: name_y, w, h: font_size + pad,
                action: ControlAction::HoverName(slot_index),
            });
        }
        cy += font_size + pad;

        // Color swatch (clickable)
        let swatch_x = x + (w - swatch_size) / 2.0;
        tiny_skia_widgets::draw_rect(
            pixmap, swatch_x, cy, swatch_size, swatch_size,
            theme::to_color(color),
        );
        regions.push(ControlHitRegion {
            x: swatch_x, y: cy, w: swatch_size, h: swatch_size,
            action: ControlAction::CycleColor(slot_index),
        });
        cy += swatch_size + pad;

        // Outline button colors derived from track color
        let border_c = theme::to_color_alpha(color, 0.4);
        let text_c = theme::to_color_alpha(color, 0.5);
        let active_border_c = theme::to_color(color);
        let active_fill_c = theme::to_color_alpha(color, 0.15);

        // Solo / Mute buttons side by side
        let total_btn_w = btn_w * 2.0 + 4.0 * scale;
        let btn_x = x + (w - total_btn_w) / 2.0;

        widgets::draw_outline_button(
            pixmap, tr, btn_x, cy, btn_w, btn_h, "S", solo,
            border_c, text_c, active_border_c, active_fill_c,
        );
        regions.push(ControlHitRegion {
            x: btn_x, y: cy, w: btn_w, h: btn_h,
            action: ControlAction::ToggleSolo(slot_index),
        });

        let mute_x = btn_x + btn_w + 4.0 * scale;
        widgets::draw_outline_button(
            pixmap, tr, mute_x, cy, btn_w, btn_h, "M", mute,
            border_c, text_c, active_border_c, active_fill_c,
        );
        regions.push(ControlHitRegion {
            x: mute_x, y: cy, w: btn_w, h: btn_h,
            action: ControlAction::ToggleMute(slot_index),
        });
    }

    regions
}

/// Truncate a name to fit within `max_w` pixels, appending ellipsis if needed.
fn truncate_name(
    full_name: &str,
    max_w: f32,
    font_size: f32,
    tr: &mut tiny_skia_widgets::TextRenderer,
) -> (String, bool) {
    let full_w = tr.text_width(full_name, font_size);
    if full_w <= max_w {
        return (full_name.to_string(), false);
    }
    let ellipsis = "...";
    let ellipsis_w = tr.text_width(ellipsis, font_size);
    let target_w = max_w - ellipsis_w;
    let mut trunc = full_name.to_string();
    while !trunc.is_empty() && tr.text_width(&trunc, font_size) > target_w {
        trunc.pop();
    }
    trunc.push_str(ellipsis);
    (trunc, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_actions_are_distinct() {
        assert_ne!(ControlAction::CycleColor(0), ControlAction::ToggleSolo(0));
        assert_ne!(ControlAction::ToggleSolo(0), ControlAction::ToggleMute(0));
    }
}
