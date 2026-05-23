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
    /// EFFECT section header band (caption + divider rule). Draw-only.
    pub effect_header: (f32, f32, f32, f32),
    /// Effect-kind dropdown trigger.
    pub kind: (f32, f32, f32, f32),
    /// Parameter dial bounding boxes, slot order. Only the first
    /// `parameters().len()` are used by the current effect.
    pub dials: [(f32, f32, f32, f32); DIAL_SLOTS],
    /// Per-track Mix dial — a fixed slot right of the parameter dials.
    pub mix: (f32, f32, f32, f32),
    /// MODULATION section header band (caption + divider rule). Draw-only.
    pub modulation_header: (f32, f32, f32, f32),
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
    /// Trigger-rate dial — only hot when the source is `FreeHz` (the slot also
    /// hosts the `Sens` dial when the source is `Transient`).
    pub trigger_rate: (f32, f32, f32, f32),
    /// Secondary trigger dial — only hot when the source is `Transient`
    /// (hosts the `Hold` refractory dial). Sits to the right of `trigger_rate`
    /// at the same vertical position.
    pub trigger_aux: (f32, f32, f32, f32),
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
    let back = l(ox, oy + 4.0, 90.0, 30.0);
    // EFFECT section — a header band, then the controls shifted down to clear it.
    let effect_header = l(ox, oy + 36.0, mw - inset, 16.0);
    let kind = l(ox, oy + 66.0, 150.0, 34.0);
    let dials = std::array::from_fn(|i| {
        // Slots 0..3 sit in the standard 96-px-spaced dial row; slot 4 sits
        // in the dial column immediately to its right at ox+580.
        if i < 4 {
            l(ox + 180.0 + i as f32 * 96.0, oy + 60.0, 88.0, 88.0)
        } else {
            l(ox + 580.0, oy + 60.0, 88.0, 88.0)
        }
    });
    // Per-track Mix dial: anchored at the right end of the dial row across
    // every effect, regardless of how many params the effect declares. Sits
    // 16 px past slot 4 so its column stays put whether or not slot 4 is in
    // use.
    let mix = l(ox + 684.0, oy + 60.0, 88.0, 88.0);
    // MODULATION section — its own header band, then the controls. The trigger
    // and rate are PER-TRACK (govern all 3 MSEGs).
    let modulation_header = l(ox, oy + 152.0, mw - inset, 16.0);
    let trigger = l(ox, oy + 200.0, 130.0, 34.0);
    // Same size + raised-y as the depth dial so its label/value text reads at
    // the same scale as the rest of the modulation-row dials.
    let trigger_rate = l(ox + 146.0, oy + 172.0, 64.0, 64.0);
    // The aux trigger dial sits flush to the right of trigger_rate. Together
    // they occupy ox+146..ox+216; the rest of the modulation row is shifted
    // 74 px right to clear them.
    let trigger_aux = l(ox + 216.0, oy + 172.0, 64.0, 64.0);
    let mseg_selector = l(ox + 296.0, oy + 200.0, 240.0, 34.0);
    let target = l(ox + 552.0, oy + 200.0, 170.0, 34.0);
    // Depth dial: raised so its value text doesn't fall into the MSEG pane below.
    let depth = l(ox + 738.0, oy + 172.0, 64.0, 64.0);
    // Active-MSEG sync + length, on the modulation row to the right of depth.
    let mseg_sync = l(ox + 814.0, oy + 200.0, 110.0, 34.0);
    let mseg_length = l(ox + 934.0, oy + 200.0, 140.0, 34.0);
    let mseg_pane = l(ox, oy + 240.0, mw - inset, 390.0);
    EffectLayout {
        back,
        effect_header,
        kind,
        dials,
        mix,
        modulation_header,
        mseg_selector,
        target,
        depth,
        mseg_pane,
        trigger,
        trigger_rate,
        trigger_aux,
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
    /// The trigger-rate dial. Hot when the source is `FreeHz` (`Rate`) or
    /// `Transient` (`Sens`).
    TriggerRate,
    /// The trigger aux dial. Hot only when the source is `Transient` (`Hold`).
    TriggerAux,
    /// Active-MSEG sync-mode selector segment (0 = Time, 1 = Beat).
    MsegSync(usize),
    /// Active-MSEG length slider (its scale depends on the sync mode).
    MsegLength,
    /// The per-track Mix dial.
    Mix,
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
    trigger: TriggerSource,
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
    if in_rect(lay.mix, px, py) {
        return Some(EffectHit::Mix);
    }
    // Per-track trigger controls — checked before the per-MSEG selector.
    if in_rect(lay.trigger, px, py) {
        return Some(EffectHit::Trigger);
    }
    let trigger_has_rate = matches!(
        trigger,
        TriggerSource::FreeHz { .. } | TriggerSource::Transient { .. }
    );
    let trigger_has_aux = matches!(trigger, TriggerSource::Transient { .. });
    if trigger_has_rate && in_rect(lay.trigger_rate, px, py) {
        return Some(EffectHit::TriggerRate);
    }
    if trigger_has_aux && in_rect(lay.trigger_aux, px, py) {
        return Some(EffectHit::TriggerAux);
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

/// Draw a section header into `rect`: a left-aligned caption followed by a
/// thin divider rule running to the rect's right edge.
pub fn draw_section_header(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    rect: (f32, f32, f32, f32),
    label: &str,
    scale: f32,
) {
    let (x, y, w, h) = rect;
    let size = 13.0 * scale;
    let baseline = y + (h + size) * 0.5 - 2.0;
    tr.draw_text(pixmap, x, baseline, label, size, widgets::color_muted());
    // Divider rule, vertically centred, starting a gap past the caption.
    let caption_w = tr.text_width(label, size);
    let rule_x = x + caption_w + 8.0 * scale;
    let rule_w = (x + w) - rule_x;
    if rule_w > 0.0 {
        let rule_h = scale.max(1.0);
        widgets::draw_rect(
            pixmap,
            rule_x,
            y + (h - rule_h) * 0.5,
            rule_w,
            rule_h,
            widgets::color_border(),
        );
    }
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
    // `Some((buffer, caret_on))` when a right-click text edit is active on
    // the Mix dial; it renders the buffer + caret in place of the percentage.
    editing_mix: Option<(&str, bool)>,
    // `Some(i)` when a per-param Enum dropdown is currently open — render its
    // trigger in the open (accented) state.
    open_param_dropdown: Option<usize>,
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
    // EFFECT section: kind dropdown trigger.
    widgets::dropdown::draw_dropdown_trigger(
        pixmap,
        tr,
        lay.kind,
        track.kind.name(),
        kind_dropdown_open,
    );
    // Parameter dials. An `Enum`-format param replaces its dial with a
    // dropdown trigger — uniform widget for any N-option discrete selector,
    // whatever the option count.
    let instance = crate::effects::EffectInstance::new(track.kind);
    let specs = instance.parameters();
    for (i, spec) in specs.iter().enumerate() {
        let (dx, dy, dw, dh) = lay.dials[i];
        let value = track.params[i];
        let value_text = crate::effects::format_value(value, spec.format);
        if matches!(spec.format, crate::effects::ParamFormat::Enum { .. }) {
            // Centre a labelled dropdown trigger in the dial slot. The label
            // sits above the trigger like a dial's name above its dial face.
            let label_size = (dh * 0.18).max(11.0 * scale);
            let label_w = tr.text_width(spec.name, label_size);
            tr.draw_text(
                pixmap,
                dx + (dw - label_w) * 0.5,
                dy + dh * 0.22,
                spec.name,
                label_size,
                widgets::color_text(),
            );
            let trigger_h = (dh * 0.32).max(22.0 * scale);
            // Overflow the dial slot by 6 px on each side so longer labels
            // (e.g. "Modulator") fit without truncation. The horizontal gap
            // between adjacent dials is 8 px at scale 1, so a 6 px overflow
            // leaves a comfortable margin to the neighbouring slot.
            let trigger_w = dw + 12.0 * scale;
            let trigger_x = dx + (dw - trigger_w) * 0.5;
            let trigger_y = dy + dh * 0.46;
            let is_open = open_param_dropdown == Some(i);
            widgets::dropdown::draw_dropdown_trigger(
                pixmap,
                tr,
                (trigger_x, trigger_y, trigger_w, trigger_h),
                &value_text,
                is_open,
            );
            continue;
        }
        let norm = crate::effects::value_to_norm(value, spec.min, spec.max, spec.scaling);
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
    // Per-track Mix dial — value shown as a percentage, or the edit buffer
    // when a text entry is active on it.
    let (mx, my, mw, mh) = lay.mix;
    let mix_cx = mx + mw / 2.0;
    let mix_cy = my + mh / 2.0;
    let mix_radius = (mw.min(mh) / 2.0) - 8.0 * scale;
    let mix_pct = format!("{}%", (track.mix * 100.0).round() as i32);
    match editing_mix {
        Some((buf, caret_on)) => {
            widgets::param_dial::draw_dial_ex(
                pixmap,
                tr,
                mix_cx,
                mix_cy,
                mix_radius,
                "Mix",
                &mix_pct,
                track.mix,
                None,
                Some(buf),
                caret_on,
            );
        }
        None => {
            widgets::param_dial::draw_dial(
                pixmap, tr, mix_cx, mix_cy, mix_radius, "Mix", &mix_pct, track.mix,
            );
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
pub fn trigger_items() -> [&'static str; 5] {
    [
        "Free run",
        "Cell light",
        "Cell step",
        "Free Hz",
        "Transient",
    ]
}

/// Build a `TriggerSource` from a dropdown item index. `carried_hz` seeds a
/// fresh `FreeHz` with the last-used rate dial value; `carried_threshold`
/// and `carried_hold_ms` seed a fresh `Transient` from the last-used
/// sensitivity / hold dials.
pub fn trigger_from_item(
    item: usize,
    carried_hz: f32,
    carried_threshold: f32,
    carried_hold_ms: f32,
) -> TriggerSource {
    match item {
        0 => TriggerSource::Free,
        1 => TriggerSource::CellLight,
        2 => TriggerSource::CellStep,
        3 => TriggerSource::FreeHz { hz: carried_hz },
        _ => TriggerSource::Transient {
            threshold: carried_threshold,
            hold_ms: carried_hold_ms,
        },
    }
}

/// The dropdown item index for a `TriggerSource`.
pub fn trigger_to_item(src: TriggerSource) -> usize {
    match src {
        TriggerSource::Free => 0,
        TriggerSource::CellLight => 1,
        TriggerSource::CellStep => 2,
        TriggerSource::FreeHz { .. } => 3,
        TriggerSource::Transient { .. } => 4,
    }
}

/// The trigger-rate dial range (Hz).
pub const TRIGGER_RATE_MIN_HZ: f32 = 0.05;
pub const TRIGGER_RATE_MAX_HZ: f32 = 20.0;

/// User-facing labels for the Transient trigger's two dials. The threshold
/// dial reads as "Sensitivity" because users think in terms of "how easy is
/// it to trigger" — sensitivity is the inverse of the underlying ratio.
pub const TRIGGER_TRANSIENT_SENS_LABEL: &str = "Sens";
pub const TRIGGER_TRANSIENT_HOLD_LABEL: &str = "Hold";

/// Length slider range when the active MSEG is Time-synced (seconds per cycle).
pub const MSEG_LENGTH_TIME_MIN: f32 = 0.05;
pub const MSEG_LENGTH_TIME_MAX: f32 = 32.0;

/// Discrete musical subdivisions the Beat-synced length slider snaps to,
/// in units of beats with their display label. Each entry doubles the
/// previous: `1/16` note (0.25 beats) up to `64 bars` (256 beats) in 4/4.
/// Sub-bar values keep their note-fraction notation (`1/16`, `1/4`);
/// whole-bar values switch to `N bar(s)`.
pub const BEAT_LADDER: &[(f32, &str)] = &[
    (0.25, "1/16"),
    (0.5, "1/8"),
    (1.0, "1/4"),
    (2.0, "1/2"),
    (4.0, "1 bar"),
    (8.0, "2 bars"),
    (16.0, "4 bars"),
    (32.0, "8 bars"),
    (64.0, "16 bars"),
    (128.0, "32 bars"),
    (256.0, "64 bars"),
];

/// The Beat-synced length floor and ceiling, kept as `pub` for callers that
/// want the absolute bounds (e.g. clamping a deserialized value). Equal to
/// `BEAT_LADDER`'s endpoints.
pub const MSEG_LENGTH_BEATS_MIN: f32 = 0.25;
pub const MSEG_LENGTH_BEATS_MAX: f32 = 256.0;

/// Map a normalized 0..1 slider position to a Beat-synced length value by
/// quantizing to the nearest `BEAT_LADDER` index.
pub fn beats_norm_to_value(norm: f32) -> f32 {
    let max_idx = (BEAT_LADDER.len() - 1) as f32;
    let idx = (norm.clamp(0.0, 1.0) * max_idx).round() as usize;
    BEAT_LADDER[idx.min(BEAT_LADDER.len() - 1)].0
}

/// Inverse of `beats_norm_to_value`: find the nearest ladder entry (by log
/// distance, since the ladder doubles each step) and return its normalized
/// 0..1 slider position.
pub fn beats_value_to_norm(v: f32) -> f32 {
    let log_v = v.max(1e-6).ln();
    let mut best = 0usize;
    let mut best_d = f32::INFINITY;
    for (i, (b, _)) in BEAT_LADDER.iter().enumerate() {
        let d = (b.ln() - log_v).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best as f32 / (BEAT_LADDER.len() - 1) as f32
}

/// Display label for a Beat-synced length value: the nearest ladder entry's
/// name. The slider snaps on drag so off-grid values are only ever seen if
/// the persisted state was set by an older build.
fn format_beats_label(v: f32) -> &'static str {
    let log_v = v.max(1e-6).ln();
    let mut best = 0usize;
    let mut best_d = f32::INFINITY;
    for (i, (b, _)) in BEAT_LADDER.iter().enumerate() {
        let d = (b.ln() - log_v).abs();
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    BEAT_LADDER[best].1
}

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
            (beats_value_to_norm(v), format_beats_label(v).to_string())
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

/// Draw the per-track trigger dropdown trigger and the trigger-source-specific
/// dials: a `Rate` dial for `FreeHz`; a `Sens` + `Hold` pair for `Transient`.
/// `Free`/`CellLight`/`CellStep` leave the dial slots empty.
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
        TriggerSource::CellStep => "Cell step",
        TriggerSource::FreeHz { .. } => "Free Hz",
        TriggerSource::Transient { .. } => "Transient",
    };
    widgets::dropdown::draw_dropdown_trigger(pixmap, tr, lay.trigger, label, trigger_dropdown_open);
    match trigger {
        TriggerSource::FreeHz { hz } => {
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
        TriggerSource::Transient { threshold, hold_ms } => {
            let (rx, ry, rw, rh) = lay.trigger_rate;
            // Sensitivity is the inverse of the stored threshold ratio, so
            // the dial sweep reads as "more triggers" → right.
            let sens_norm = 1.0
                - crate::effects::value_to_norm(
                    threshold,
                    crate::modulation::TRANSIENT_THRESHOLD_MIN,
                    crate::modulation::TRANSIENT_THRESHOLD_MAX,
                    crate::effects::ParamScaling::Log,
                );
            widgets::param_dial::draw_dial(
                pixmap,
                tr,
                rx + rw / 2.0,
                ry + rh / 2.0,
                (rw.min(rh) / 2.0) - 6.0 * scale,
                TRIGGER_TRANSIENT_SENS_LABEL,
                &format!("{:.0}%", sens_norm * 100.0),
                sens_norm,
            );
            let (ax, ay, aw, ah) = lay.trigger_aux;
            widgets::param_dial::draw_dial(
                pixmap,
                tr,
                ax + aw / 2.0,
                ay + ah / 2.0,
                (aw.min(ah) / 2.0) - 6.0 * scale,
                TRIGGER_TRANSIENT_HOLD_LABEL,
                &format!("{hold_ms:.0} ms"),
                crate::effects::value_to_norm(
                    hold_ms,
                    crate::modulation::TRANSIENT_HOLD_MS_MIN,
                    crate::modulation::TRANSIENT_HOLD_MS_MAX,
                    crate::effects::ParamScaling::Log,
                ),
            );
        }
        _ => {}
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
    fn section_headers_are_disjoint_from_their_section_controls() {
        let lay = effect_layout(1.0);
        // EFFECT header clears the editor bar above and the EFFECT controls below.
        assert!(!rects_overlap(lay.effect_header, lay.back));
        assert!(!rects_overlap(lay.effect_header, lay.kind));
        for d in lay.dials {
            assert!(!rects_overlap(lay.effect_header, d));
        }
        assert!(!rects_overlap(lay.effect_header, lay.mix));
        // MODULATION header clears the EFFECT dials above and the MODULATION
        // controls below.
        for d in lay.dials {
            assert!(!rects_overlap(lay.modulation_header, d));
        }
        assert!(!rects_overlap(lay.modulation_header, lay.trigger));
        assert!(!rects_overlap(lay.modulation_header, lay.depth));
        assert!(!rects_overlap(lay.modulation_header, lay.mseg_pane));
        // The two headers do not overlap each other.
        assert!(!rects_overlap(lay.effect_header, lay.modulation_header));
        // The MSEG pane still sits below both sections.
        assert!(lay.mseg_pane.1 >= lay.modulation_header.1 + lay.modulation_header.3);
        assert!(lay.mseg_pane.1 >= lay.depth.1 + lay.depth.3);
    }

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
    fn format_beats_label_names_every_ladder_entry() {
        // Each ladder entry maps to exactly its registered label.
        for &(v, name) in BEAT_LADDER {
            assert_eq!(format_beats_label(v), name, "v = {v}");
        }
    }

    #[test]
    fn format_beats_label_snaps_off_grid_to_nearest() {
        // An off-grid value falls to whichever ladder entry is closest in log
        // space. 0.32 sits between 1/16 (0.25) and 1/8 (0.5) and is closer
        // (geometrically) to 1/16.
        assert_eq!(format_beats_label(0.32), "1/16");
        // 1.7 sits between 1/2 (2.0) and 1/4 (1.0); the geometric midpoint is
        // sqrt(2) ≈ 1.414, so 1.7 lands on 1/2.
        assert_eq!(format_beats_label(1.7), "1/2");
    }

    #[test]
    fn beats_norm_and_value_round_trip_through_the_ladder() {
        let n = BEAT_LADDER.len();
        for (i, &(v, _)) in BEAT_LADDER.iter().enumerate() {
            let norm = i as f32 / (n - 1) as f32;
            // norm at the ladder position maps back to exactly that beats value.
            assert_eq!(beats_norm_to_value(norm), v, "ladder index {i}");
            // The reverse also lands on the same normalized position.
            assert!(
                (beats_value_to_norm(v) - norm).abs() < 1e-5,
                "value_to_norm({v}) gave {} not {norm}",
                beats_value_to_norm(v)
            );
        }
    }

    #[test]
    fn beats_norm_to_value_snaps_continuous_norm_to_nearest_step() {
        // A norm just past the boundary between two ladder steps lands on the
        // upper one; just before, on the lower one.
        let n = BEAT_LADDER.len();
        let step = 1.0 / (n - 1) as f32;
        let mid = 0.5 * step;
        assert_eq!(beats_norm_to_value(mid - 0.01), BEAT_LADDER[0].0);
        assert_eq!(beats_norm_to_value(mid + 0.01), BEAT_LADDER[1].0);
    }

    #[test]
    fn target_items_lists_none_then_each_parameter() {
        let items = target_items(crate::effects::EffectKind::Lowpass);
        assert_eq!(items[0], "(none)");
        assert_eq!(items[1], "Cutoff");
        assert_eq!(items[2], "Resonance");
        assert_eq!(items[3], "Poles");
        assert_eq!(items.len(), 4);
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
    fn trigger_items_lists_the_five_sources() {
        let items = trigger_items();
        assert_eq!(
            items,
            [
                "Free run",
                "Cell light",
                "Cell step",
                "Free Hz",
                "Transient"
            ]
        );
    }

    #[test]
    fn trigger_from_and_to_item_round_trip() {
        // 0 -> Free, 1 -> CellLight, 2 -> CellStep, 3 -> FreeHz, 4 -> Transient.
        assert_eq!(trigger_from_item(0, 1.0, 1.5, 50.0), TriggerSource::Free);
        assert_eq!(
            trigger_from_item(1, 1.0, 1.5, 50.0),
            TriggerSource::CellLight
        );
        assert_eq!(
            trigger_from_item(2, 1.0, 1.5, 50.0),
            TriggerSource::CellStep
        );
        assert_eq!(
            trigger_from_item(3, 3.5, 1.5, 50.0),
            TriggerSource::FreeHz { hz: 3.5 }
        );
        assert_eq!(
            trigger_from_item(4, 1.0, 2.0, 75.0),
            TriggerSource::Transient {
                threshold: 2.0,
                hold_ms: 75.0,
            }
        );
        assert_eq!(trigger_to_item(TriggerSource::Free), 0);
        assert_eq!(trigger_to_item(TriggerSource::CellLight), 1);
        assert_eq!(trigger_to_item(TriggerSource::CellStep), 2);
        assert_eq!(trigger_to_item(TriggerSource::FreeHz { hz: 99.0 }), 3);
        assert_eq!(
            trigger_to_item(TriggerSource::Transient {
                threshold: 2.0,
                hold_ms: 75.0,
            }),
            4
        );
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
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 0, TriggerSource::Free),
            Some(EffectHit::Trigger)
        );
        assert_eq!(
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 1, TriggerSource::Free),
            Some(EffectHit::Trigger)
        );
    }

    #[test]
    fn mix_dial_is_hit_and_disjoint_from_other_effect_controls() {
        let lay = effect_layout(1.0);
        assert!(!rects_overlap(lay.mix, lay.kind));
        for d in lay.dials {
            assert!(!rects_overlap(lay.mix, d));
        }
        let (mx, my, mw, mh) = lay.mix;
        let hit = effect_hit(mx + mw / 2.0, my + mh / 2.0, 1.0, 2, 0, TriggerSource::Free);
        assert_eq!(hit, Some(EffectHit::Mix));
    }

    #[test]
    fn effect_hit_returns_trigger_rate_for_free_hz_and_transient() {
        let lay = effect_layout(1.0);
        let (rx, ry, rw, rh) = lay.trigger_rate;
        // FreeHz: rate dial is hot.
        assert_eq!(
            effect_hit(
                rx + rw / 2.0,
                ry + rh / 2.0,
                1.0,
                2,
                0,
                TriggerSource::FreeHz { hz: 1.0 },
            ),
            Some(EffectHit::TriggerRate)
        );
        // Transient: rate-dial slot hosts Sens — also hot.
        assert_eq!(
            effect_hit(
                rx + rw / 2.0,
                ry + rh / 2.0,
                1.0,
                2,
                0,
                TriggerSource::Transient {
                    threshold: 1.5,
                    hold_ms: 50.0,
                },
            ),
            Some(EffectHit::TriggerRate)
        );
        // Not FreeHz nor Transient: rate dial falls through (not TriggerRate).
        let other = effect_hit(rx + rw / 2.0, ry + rh / 2.0, 1.0, 2, 0, TriggerSource::Free);
        assert_ne!(other, Some(EffectHit::TriggerRate));
    }

    #[test]
    fn effect_hit_returns_trigger_aux_only_for_transient() {
        let lay = effect_layout(1.0);
        let (ax, ay, aw, ah) = lay.trigger_aux;
        let cx = ax + aw / 2.0;
        let cy = ay + ah / 2.0;
        // Transient: aux dial (Hold) is hot.
        assert_eq!(
            effect_hit(
                cx,
                cy,
                1.0,
                2,
                0,
                TriggerSource::Transient {
                    threshold: 1.5,
                    hold_ms: 50.0,
                },
            ),
            Some(EffectHit::TriggerAux)
        );
        // FreeHz: aux dial NOT hot (only Transient uses it).
        let free_hz_hit = effect_hit(cx, cy, 1.0, 2, 0, TriggerSource::FreeHz { hz: 1.0 });
        assert_ne!(free_hz_hit, Some(EffectHit::TriggerAux));
    }

    #[test]
    fn layout_trigger_aux_sits_disjoint_from_trigger_rate_and_mseg_controls() {
        let lay = effect_layout(1.0);
        assert!(!rects_overlap(lay.trigger_rate, lay.trigger_aux));
        assert!(!rects_overlap(lay.trigger_aux, lay.mseg_selector));
        assert!(!rects_overlap(lay.trigger_aux, lay.target));
        assert!(!rects_overlap(lay.trigger_aux, lay.depth));
        // Aux sits to the RIGHT of the rate dial.
        assert!(lay.trigger_rate.0 < lay.trigger_aux.0);
    }
}
