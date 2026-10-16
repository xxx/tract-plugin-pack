//! Multosis geometry and navigation around the shared effect editor.

use crate::effects::{EffectKind, MAX_EFFECT_PARAMS, TrackEffect};
use crate::modulation::TriggerSource;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;
pub use tract_effects::editor::view::*;

pub fn effect_layout(scale: f32) -> EffectLayout {
    use super::grid_view::{GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
    tract_effects::editor::view::effect_layout(
        (
            MARGIN + TRACK_PANEL_W,
            STATUS_H + GUTTER,
            super::WINDOW_WIDTH as f32 - MARGIN * 2.0 - TRACK_PANEL_W,
            super::WINDOW_HEIGHT as f32 - STATUS_H - GUTTER - MARGIN,
        ),
        scale,
    )
}

pub fn draw_heading(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track_index: usize,
    scale: f32,
) {
    let lay = effect_layout(scale);
    // Editor bar.
    widgets::controls::draw_button(
        pixmap, tr, lay.back.0, lay.back.1, lay.back.2, lay.back.3, "< Grid", false, false,
    );
    let title_size = 16.0 * scale;
    tr.draw_text(
        pixmap,
        lay.back.0 + lay.back.2 + 16.0 * scale,
        lay.back.1 + (lay.back.3 + title_size) * 0.5 - 2.0,
        &format!("Editing Track {}", track_index + 1),
        title_size,
        tiny_skia::Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn draw_effect_section(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track: &TrackEffect,
    track_index: usize,
    kind_dropdown_open: bool,
    editing_dial: Option<(usize, &str, bool)>,
    editing_mix: Option<(&str, bool)>,
    open_param_dropdown: Option<usize>,
    modulated_norms: &[Option<(f32, u8)>; MAX_EFFECT_PARAMS],
    scale: f32,
) {
    draw_heading(pixmap, tr, track_index, scale);
    tract_effects::editor::view::draw_effect_section(
        pixmap,
        tr,
        track,
        kind_dropdown_open,
        editing_dial,
        editing_mix,
        open_param_dropdown,
        modulated_norms,
        scale,
        &effect_layout(scale),
    );
}

pub fn effect_hit(
    px: f32,
    py: f32,
    scale: f32,
    count: usize,
    slot: usize,
    trigger: TriggerSource,
) -> Option<EffectHit> {
    tract_effects::editor::view::effect_hit(
        px,
        py,
        scale,
        &effect_layout(scale),
        count,
        slot,
        trigger,
    )
}

pub fn draw_mseg_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    mseg: &widgets::MsegData,
    scale: f32,
) {
    tract_effects::editor::view::draw_mseg_controls(pixmap, tr, mseg, scale, &effect_layout(scale));
}
pub fn draw_trigger_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    trigger: TriggerSource,
    open: bool,
    scale: f32,
) {
    tract_effects::editor::view::draw_trigger_controls(
        pixmap,
        tr,
        trigger,
        open,
        scale,
        &effect_layout(scale),
    );
}
#[allow(clippy::too_many_arguments)]
pub fn draw_modulation_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    slot: usize,
    kind: EffectKind,
    target: Option<usize>,
    depth: f32,
    open: bool,
    scale: f32,
) {
    tract_effects::editor::view::draw_modulation_controls(
        pixmap,
        tr,
        slot,
        kind,
        target,
        depth,
        open,
        scale,
        &effect_layout(scale),
    );
}
