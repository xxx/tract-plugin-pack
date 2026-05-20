//! The per-track effect editor — Phase 2 Milestone 2c. Drawn in the main area
//! (right of the track panel) when the window is in `View::Effect`. The EFFECT
//! section holds the kind dropdown and parameter dials; the MODULATION section
//! holds the MSEG selector, target/depth controls, and the MSEG pane.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §3.

use crate::editor::grid_view::{GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::editor::WINDOW_WIDTH;
use crate::effects::{Effect, EffectKind, TrackEffect, MAX_EFFECT_PARAMS};
use crate::modulation::TriggerSource;
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
    /// Trigger-source dropdown trigger.
    pub trigger: (f32, f32, f32, f32),
    /// Trigger-rate dial — only hot when the source is `FreeHz`.
    pub trigger_rate: (f32, f32, f32, f32),
    /// Active-MSEG sync-mode selector (Time / Beat).
    pub mseg_sync: (f32, f32, f32, f32),
    /// Active-MSEG length slider (beats or seconds, depending on sync mode).
    pub mseg_length: (f32, f32, f32, f32),
}

/// Compute the effect-editor layout at `scale`.
pub fn effect_layout(scale: f32) -> EffectLayout {
    // Logical main-area origin (right of the track panel, below the toolbar).
    let ox = MARGIN + TRACK_PANEL_W;
    let oy = STATUS_H + GUTTER;
    let mw = WINDOW_WIDTH as f32 - MARGIN - ox;
    // Left margin between the track panel and the effect-editor controls so
    // they don't butt against the listing on the left.
    let inset = 14.0_f32;
    let l = |x: f32, y: f32, w: f32, h: f32| ((x + inset) * scale, y * scale, w * scale, h * scale);
    // Editor bar.
    let back = l(ox, oy + 4.0, 90.0, 26.0);
    // EFFECT section.
    let kind = l(ox, oy + 50.0, 150.0, 28.0);
    let dials = std::array::from_fn(|i| l(ox + 180.0 + i as f32 * 96.0, oy + 44.0, 80.0, 80.0));
    // MODULATION section — trigger + rate on the left, then MSEG selector +
    // target + depth. The trigger and rate are PER-TRACK (govern all 3 MSEGs).
    let trigger = l(ox, oy + 168.0, 130.0, 26.0);
    let trigger_rate = l(ox + 146.0, oy + 162.0, 60.0, 38.0);
    let mseg_selector = l(ox + 222.0, oy + 168.0, 240.0, 26.0);
    let target = l(ox + 478.0, oy + 167.0, 170.0, 28.0);
    // Depth dial: 60×60 and raised to oy+144 so its value text doesn't fall
    // into the MSEG pane below (which starts at oy+208).
    let depth = l(ox + 664.0, oy + 144.0, 60.0, 60.0);
    // Active-MSEG sync + length, on the modulation row to the right of depth.
    let mseg_sync = l(ox + 740.0, oy + 168.0, 110.0, 26.0);
    let mseg_length = l(ox + 860.0, oy + 168.0, 140.0, 26.0);
    let mseg_pane = l(ox, oy + 208.0, mw - inset, 422.0);
    EffectLayout {
        back,
        kind,
        dials,
        mseg_selector,
        target,
        depth,
        mseg_pane,
        trigger,
        trigger_rate,
        mseg_sync,
        mseg_length,
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
    /// The per-track trigger-source dropdown.
    Trigger,
    /// The per-track trigger-rate dial (only hot when the source is FreeHz).
    TriggerRate,
    /// Active-MSEG sync-mode selector segment (0 = Time, 1 = Beat).
    MsegSync(usize),
    /// Active-MSEG length slider (its scale depends on the sync mode).
    MsegLength,
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
    is_free_hz: bool,
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
    // Per-track trigger controls — checked before the per-MSEG selector.
    if in_rect(lay.trigger, px, py) {
        return Some(EffectHit::Trigger);
    }
    if is_free_hz && in_rect(lay.trigger_rate, px, py) {
        return Some(EffectHit::TriggerRate);
    }
    // MSEG selector — three equal segments.
    let (sx, sy, sw, sh) = lay.mseg_selector;
    if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
        let seg = (((px - sx) / (sw / 3.0)) as usize).min(2);
        return Some(EffectHit::MsegSelector(seg));
    }
    // Active-MSEG sync mode (2-segment selector) + length slider.
    let (sx, sy, sw, sh) = lay.mseg_sync;
    if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
        let seg = (((px - sx) / (sw / 2.0)) as usize).min(1);
        return Some(EffectHit::MsegSync(seg));
    }
    if in_rect(lay.mseg_length, px, py) {
        return Some(EffectHit::MsegLength);
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
/// config; `track_index` is its row (0-based) for the title. `editing_dial`
/// is `Some((i, buffer, caret_on))` when a right-click text edit is active on
/// dial slot `i`; that dial renders the buffer + caret in place of its
/// formatted value.
#[allow(clippy::too_many_arguments)]
pub fn draw_effect_section(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track: &TrackEffect,
    track_index: usize,
    kind_dropdown_open: bool,
    editing_dial: Option<(usize, &str, bool)>,
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
        let norm = crate::effects::value_to_norm(value, spec.min, spec.max, spec.scaling);
        let value_text = crate::effects::format_value(value, spec.format);
        match editing_dial {
            Some((idx, buf, caret_on)) if idx == i => {
                widgets::param_dial::draw_dial_ex(
                    pixmap,
                    tr,
                    dx + dw / 2.0,
                    dy + dh / 2.0,
                    (dw.min(dh) / 2.0) - 8.0 * scale,
                    spec.name,
                    &value_text,
                    norm,
                    None,
                    Some(buf),
                    caret_on,
                );
            }
            _ => {
                widgets::param_dial::draw_dial(
                    pixmap,
                    tr,
                    dx + dw / 2.0,
                    dy + dh / 2.0,
                    (dw.min(dh) / 2.0) - 8.0 * scale,
                    spec.name,
                    &value_text,
                    norm,
                );
            }
        }
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

/// The trigger-source dropdown items, in `TriggerSource` discriminant order.
pub fn trigger_items() -> [&'static str; 3] {
    ["Free run", "Cell light", "Free Hz"]
}

/// Build a `TriggerSource` from a dropdown item index. `carried_hz` is the
/// `hz` to seed `FreeHz` with (the dial's current value, or a default).
pub fn trigger_from_item(item: usize, carried_hz: f32) -> TriggerSource {
    match item {
        0 => TriggerSource::Free,
        1 => TriggerSource::CellLight,
        _ => TriggerSource::FreeHz { hz: carried_hz },
    }
}

/// The dropdown item index for a `TriggerSource`.
pub fn trigger_to_item(src: TriggerSource) -> usize {
    match src {
        TriggerSource::Free => 0,
        TriggerSource::CellLight => 1,
        TriggerSource::FreeHz { .. } => 2,
    }
}

/// The trigger-rate dial range (Hz).
pub const TRIGGER_RATE_MIN_HZ: f32 = 0.05;
pub const TRIGGER_RATE_MAX_HZ: f32 = 20.0;

/// Length slider range when the active MSEG is Beat-synced (beats per cycle).
pub const MSEG_LENGTH_BEATS_MIN: f32 = 1.0;
pub const MSEG_LENGTH_BEATS_MAX: f32 = 64.0;
/// Length slider range when the active MSEG is Time-synced (seconds per cycle).
pub const MSEG_LENGTH_TIME_MIN: f32 = 0.05;
pub const MSEG_LENGTH_TIME_MAX: f32 = 32.0;

/// Draw the active-MSEG sync-mode selector + length slider on the modulation
/// row. Reads sync mode + length from `mseg`.
pub fn draw_mseg_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    mseg: &tiny_skia_widgets::MsegData,
    scale: f32,
) {
    let lay = effect_layout(scale);
    let active = match mseg.sync_mode {
        tiny_skia_widgets::SyncMode::Time => 0,
        tiny_skia_widgets::SyncMode::Beat => 1,
    };
    widgets::controls::draw_stepped_selector(
        pixmap,
        tr,
        lay.mseg_sync.0,
        lay.mseg_sync.1,
        lay.mseg_sync.2,
        lay.mseg_sync.3,
        &["Time", "Beat"],
        active,
    );
    let (norm, label) = match mseg.sync_mode {
        tiny_skia_widgets::SyncMode::Time => {
            let v = mseg.time_seconds;
            let n = crate::effects::value_to_norm(
                v,
                MSEG_LENGTH_TIME_MIN,
                MSEG_LENGTH_TIME_MAX,
                crate::effects::ParamScaling::Log,
            );
            (n, format!("{v:.2} s"))
        }
        tiny_skia_widgets::SyncMode::Beat => {
            let v = mseg.beats;
            let n = crate::effects::value_to_norm(
                v,
                MSEG_LENGTH_BEATS_MIN,
                MSEG_LENGTH_BEATS_MAX,
                crate::effects::ParamScaling::Log,
            );
            let rounded = v.round() as i32;
            let suffix = if rounded == 1 { "" } else { "s" };
            (n, format!("{rounded} beat{suffix}"))
        }
    };
    widgets::draw_slider(
        pixmap,
        tr,
        lay.mseg_length.0,
        lay.mseg_length.1,
        lay.mseg_length.2,
        lay.mseg_length.3,
        "Length",
        &label,
        norm,
        None,
        false,
    );
}

/// Draw a thin vertical playhead line at `phase` (0..1) over the MSEG editor's
/// plot area. `mseg_pane` is the same rect passed to `draw_mseg`. Drawn last
/// (after the curve) so it sits on top.
pub fn draw_mseg_playhead(
    pixmap: &mut Pixmap,
    mseg_pane: (f32, f32, f32, f32),
    phase: f32,
    scale: f32,
) {
    let layout = widgets::mseg::mseg_layout(mseg_pane, false, scale);
    let (px_, py_, pw_, ph_) = layout.plot;
    if pw_ <= 0.0 || ph_ <= 0.0 {
        return;
    }
    let x = widgets::mseg::phase_to_x(&layout, phase.clamp(0.0, 1.0));
    // Keep the line strictly inside the plot so it doesn't overflow at phase 1.
    let line_w = (scale).max(1.0);
    let x = x.min(px_ + pw_ - line_w);
    let color = tiny_skia::Color::from_rgba8(0xE8, 0xC9, 0x8A, 0x80);
    widgets::draw_rect(pixmap, x, py_, line_w, ph_, color);
}

/// Draw the per-track trigger dropdown trigger and (when the source is
/// `FreeHz`) the rate dial. Called as part of the MODULATION section draw.
pub fn draw_trigger_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    trigger: TriggerSource,
    trigger_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    let label = match trigger {
        TriggerSource::Free => "Free run",
        TriggerSource::CellLight => "Cell light",
        TriggerSource::FreeHz { .. } => "Free Hz",
    };
    widgets::dropdown::draw_dropdown_trigger(pixmap, tr, lay.trigger, label, trigger_dropdown_open);
    if let TriggerSource::FreeHz { hz } = trigger {
        let (rx, ry, rw, rh) = lay.trigger_rate;
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            rx + rw / 2.0,
            ry + rh / 2.0,
            (rw.min(rh) / 2.0) - 6.0 * scale,
            "Rate",
            &crate::effects::format_value(hz, crate::effects::ParamFormat::Hertz),
            crate::effects::value_to_norm(
                hz,
                TRIGGER_RATE_MIN_HZ,
                TRIGGER_RATE_MAX_HZ,
                crate::effects::ParamScaling::Log,
            ),
        );
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

    #[test]
    fn trigger_items_lists_three_sources() {
        let items = trigger_items();
        assert_eq!(items, ["Free run", "Cell light", "Free Hz"]);
    }

    #[test]
    fn trigger_from_and_to_item_round_trip() {
        // 0 -> Free, 1 -> CellLight, 2 -> FreeHz{<carried hz>}.
        assert_eq!(trigger_from_item(0, 1.0), TriggerSource::Free);
        assert_eq!(trigger_from_item(1, 1.0), TriggerSource::CellLight);
        assert_eq!(trigger_from_item(2, 3.5), TriggerSource::FreeHz { hz: 3.5 });
        assert_eq!(trigger_to_item(TriggerSource::Free), 0);
        assert_eq!(trigger_to_item(TriggerSource::CellLight), 1);
        assert_eq!(trigger_to_item(TriggerSource::FreeHz { hz: 99.0 }), 2);
    }

    #[test]
    fn layout_includes_trigger_rects_disjoint_from_other_controls() {
        let lay = effect_layout(1.0);
        assert!(!rects_overlap(lay.trigger, lay.mseg_selector));
        assert!(!rects_overlap(lay.trigger, lay.trigger_rate));
        assert!(!rects_overlap(lay.trigger_rate, lay.mseg_selector));
        // Trigger sits to the LEFT of the MSEG selector on the same row.
        assert!(lay.trigger.0 < lay.mseg_selector.0);
        // Both fit within the main area.
        assert!(lay.trigger.0 >= 0.0);
    }

    #[test]
    fn effect_hit_returns_trigger_on_the_dropdown_rect() {
        let lay = effect_layout(1.0);
        let (tx, ty, tw, th) = lay.trigger;
        // Trigger hit fires regardless of selected_mseg.
        assert_eq!(
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 0, false),
            Some(EffectHit::Trigger)
        );
        assert_eq!(
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 1, false),
            Some(EffectHit::Trigger)
        );
    }

    #[test]
    fn effect_hit_returns_trigger_rate_only_when_free_hz() {
        let lay = effect_layout(1.0);
        let (rx, ry, rw, rh) = lay.trigger_rate;
        // FreeHz: rate dial is hot.
        assert_eq!(
            effect_hit(rx + rw / 2.0, ry + rh / 2.0, 1.0, 2, 0, true),
            Some(EffectHit::TriggerRate)
        );
        // Not FreeHz: rate dial is not returned (falls through).
        let other = effect_hit(rx + rw / 2.0, ry + rh / 2.0, 1.0, 2, 0, false);
        // The fall-through may resolve to MsegPane or None depending on
        // layout; the important check is that it is NOT TriggerRate.
        assert_ne!(other, Some(EffectHit::TriggerRate));
    }
}
