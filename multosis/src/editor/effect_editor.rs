//! The per-track effect editor — Phase 2 Milestone 2c. Drawn in the main area
//! (right of the track panel) when the window is in `View::Effect`. The EFFECT
//! section holds the kind dropdown and parameter dials; the MODULATION section
//! holds the MSEG selector, target/depth controls, and the MSEG pane.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §3.

use crate::editor::grid_view::{GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::editor::WINDOW_WIDTH;
use crate::effects::{Effect, EffectKind, TrackEffect, MAX_EFFECT_PARAMS};
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// The number of parameter-dial slots the layout reserves. Matches the widest
/// effect's parameter count headroom.
pub const DIAL_SLOTS: usize = MAX_EFFECT_PARAMS;

/// All physical-pixel sub-rects of the effect editor at `scale`. Computed once;
/// consumed by both hit-testing and drawing.
#[derive(Clone, Copy, Debug)]
pub struct EffectLayout {
    /// `< Grid` back button.
    pub back: (f32, f32, f32, f32),
    /// Effect-kind dropdown trigger.
    pub kind: (f32, f32, f32, f32),
    /// Parameter dial bounding boxes, slot order. Only the first
    /// `parameters().len()` are used by the current effect.
    pub dials: [(f32, f32, f32, f32); DIAL_SLOTS],
    /// MSEG selector (stepped) — laid out here, used in Task 9.
    pub mseg_selector: (f32, f32, f32, f32),
    /// Target dropdown trigger — used in Task 9.
    pub target: (f32, f32, f32, f32),
    /// Depth dial — used in Task 9.
    pub depth: (f32, f32, f32, f32),
    /// MSEG editor pane — used in Task 9.
    pub mseg_pane: (f32, f32, f32, f32),
}

/// Compute the effect-editor layout at `scale`.
pub fn effect_layout(scale: f32) -> EffectLayout {
    // Logical main-area origin (right of the track panel, below the toolbar).
    let ox = MARGIN + TRACK_PANEL_W;
    let oy = STATUS_H + GUTTER;
    let mw = WINDOW_WIDTH as f32 - MARGIN - ox;
    let l = |x: f32, y: f32, w: f32, h: f32| (x * scale, y * scale, w * scale, h * scale);
    // Editor bar.
    let back = l(ox, oy + 4.0, 90.0, 26.0);
    // EFFECT section.
    let kind = l(ox, oy + 50.0, 150.0, 28.0);
    let dials = std::array::from_fn(|i| l(ox + 180.0 + i as f32 * 96.0, oy + 44.0, 80.0, 80.0));
    // MODULATION section.
    let mseg_selector = l(ox, oy + 168.0, 240.0, 26.0);
    let target = l(ox + 470.0, oy + 167.0, 170.0, 28.0);
    let depth = l(ox + 660.0, oy + 162.0, 70.0, 70.0);
    let mseg_pane = l(ox, oy + 208.0, mw, 422.0);
    EffectLayout {
        back,
        kind,
        dials,
        mseg_selector,
        target,
        depth,
        mseg_pane,
    }
}

/// True when physical-pixel point `(px, py)` is inside `rect`.
pub fn in_rect((rx, ry, rw, rh): (f32, f32, f32, f32), px: f32, py: f32) -> bool {
    px >= rx && px < rx + rw && py >= ry && py < ry + rh
}

/// A control the user pressed in the effect editor. `Dial`/`Depth` carry the
/// slot index; `MsegPane` carries nothing (handled by the MSEG widget).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectHit {
    Back,
    Kind,
    Dial(usize),
    MsegSelector(usize),
    Target,
    Depth,
    MsegPane,
}

/// The effect-editor control under physical-pixel point `(px, py)` at `scale`,
/// given the current effect's parameter count `param_count` and the active
/// MSEG `selected_mseg` (0 = amplitude, 1/2 = assignable). Returns `None` for
/// a point over no control.
pub fn effect_hit(
    px: f32,
    py: f32,
    scale: f32,
    param_count: usize,
    selected_mseg: usize,
) -> Option<EffectHit> {
    let lay = effect_layout(scale);
    if in_rect(lay.back, px, py) {
        return Some(EffectHit::Back);
    }
    if in_rect(lay.kind, px, py) {
        return Some(EffectHit::Kind);
    }
    for i in 0..param_count.min(DIAL_SLOTS) {
        if in_rect(lay.dials[i], px, py) {
            return Some(EffectHit::Dial(i));
        }
    }
    // MSEG selector — three equal segments.
    let (sx, sy, sw, sh) = lay.mseg_selector;
    if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
        let seg = (((px - sx) / (sw / 3.0)) as usize).min(2);
        return Some(EffectHit::MsegSelector(seg));
    }
    // Target dropdown + depth dial only exist for an assignable MSEG.
    if selected_mseg != 0 {
        if in_rect(lay.target, px, py) {
            return Some(EffectHit::Target);
        }
        if in_rect(lay.depth, px, py) {
            return Some(EffectHit::Depth);
        }
    }
    if in_rect(lay.mseg_pane, px, py) {
        return Some(EffectHit::MsegPane);
    }
    None
}

/// Draw the editor bar and EFFECT section. The MODULATION section is drawn by
/// `draw_modulation_section` (Task 9). `track` is the edited track's effect
/// config; `track_index` is its row (0-based) for the title.
pub fn draw_effect_section(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track: &TrackEffect,
    track_index: usize,
    kind_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    // Editor bar.
    widgets::controls::draw_button(
        pixmap, tr, lay.back.0, lay.back.1, lay.back.2, lay.back.3, "< Grid", false, false,
    );
    let title_size = 13.0 * scale;
    tr.draw_text(
        pixmap,
        lay.back.0 + lay.back.2 + 16.0 * scale,
        lay.back.1 + (lay.back.3 + title_size) * 0.5 - 2.0,
        &format!("Editing Track {}", track_index + 1),
        title_size,
        tiny_skia::Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF),
    );
    // EFFECT section: kind dropdown trigger.
    widgets::dropdown::draw_dropdown_trigger(
        pixmap,
        tr,
        lay.kind,
        track.kind.name(),
        kind_dropdown_open,
    );
    // Parameter dials.
    let instance = crate::effects::EffectInstance::new(track.kind);
    let specs = instance.parameters();
    for (i, spec) in specs.iter().enumerate() {
        let (dx, dy, dw, dh) = lay.dials[i];
        let value = track.params[i];
        let norm = if spec.max > spec.min {
            ((value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            spec.name,
            &format!("{value:.0}"),
            norm,
        );
    }
}

/// The list of effect-kind names for the kind dropdown, in `EffectKind::ALL`
/// order.
pub fn kind_items() -> Vec<&'static str> {
    EffectKind::ALL.iter().map(|k| k.name()).collect()
}

/// The target-dropdown items for `kind`: `(none)` followed by each parameter
/// name, in parameter-index order.
pub fn target_items(kind: EffectKind) -> Vec<&'static str> {
    let instance = crate::effects::EffectInstance::new(kind);
    let mut items = vec!["(none)"];
    items.extend(instance.parameters().iter().map(|s| s.name));
    items
}

/// The `targets` value for target-dropdown item `item` (0 = `(none)`).
pub fn target_from_item(item: usize) -> Option<usize> {
    item.checked_sub(1)
}

/// The target-dropdown item index for a `targets` value.
pub fn target_to_item(target: Option<usize>) -> usize {
    match target {
        None => 0,
        Some(i) => i + 1,
    }
}

/// Draw the MODULATION section: the MSEG selector, and — for an assignable
/// MSEG — the target dropdown trigger and depth dial. `selected_mseg` is
/// 0 (amplitude) / 1 / 2; `target` and `depth` belong to the active assignable
/// MSEG (ignored when `selected_mseg == 0`). The MSEG pane itself is drawn by
/// the caller (it needs the window's `MsegEditState`).
#[allow(clippy::too_many_arguments)]
pub fn draw_modulation_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    selected_mseg: usize,
    kind: EffectKind,
    target: Option<usize>,
    depth: f32,
    target_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    widgets::controls::draw_stepped_selector(
        pixmap,
        tr,
        lay.mseg_selector.0,
        lay.mseg_selector.1,
        lay.mseg_selector.2,
        lay.mseg_selector.3,
        &["Amp", "MSEG 1", "MSEG 2"],
        selected_mseg.min(2),
    );
    if selected_mseg != 0 {
        let label = match target {
            None => "(none)",
            Some(i) => crate::effects::EffectInstance::new(kind)
                .parameters()
                .get(i)
                .map(|s| s.name)
                .unwrap_or("(none)"),
        };
        widgets::dropdown::draw_dropdown_trigger(
            pixmap,
            tr,
            lay.target,
            label,
            target_dropdown_open,
        );
        // depth dial: bipolar -1..1 mapped to a 0..1 arc.
        let (dx, dy, dw, dh) = lay.depth;
        let norm = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            "Depth",
            &format!("{depth:+.2}"),
            norm,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rects_are_disjoint_for_the_main_controls() {
        let lay = effect_layout(1.0);
        // The back button, kind dropdown, and first dial do not overlap.
        assert!(!rects_overlap(lay.back, lay.kind));
        assert!(!rects_overlap(lay.kind, lay.dials[0]));
        assert!(!rects_overlap(lay.back, lay.dials[0]));
        // The MSEG pane sits below the EFFECT controls.
        assert!(lay.mseg_pane.1 > lay.kind.1 + lay.kind.3);
    }

    #[test]
    fn layout_scales_linearly() {
        let a = effect_layout(1.0);
        let b = effect_layout(2.0);
        assert!((b.kind.0 - a.kind.0 * 2.0).abs() < 1e-3);
        assert!((b.kind.2 - a.kind.2 * 2.0).abs() < 1e-3);
    }

    fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    #[test]
    fn target_items_lists_none_then_each_parameter() {
        let items = target_items(crate::effects::EffectKind::Lowpass);
        assert_eq!(items[0], "(none)");
        assert_eq!(items[1], "Cutoff");
        assert_eq!(items[2], "Resonance");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn target_index_round_trips_through_the_dropdown_indexing() {
        // Dropdown item 0 => None; item i+1 => Some(i).
        assert_eq!(target_from_item(0), None);
        assert_eq!(target_from_item(1), Some(0));
        assert_eq!(target_from_item(3), Some(2));
        // And back: None => 0; Some(i) => i+1.
        assert_eq!(target_to_item(None), 0);
        assert_eq!(target_to_item(Some(0)), 1);
        assert_eq!(target_to_item(Some(2)), 3);
    }
}
