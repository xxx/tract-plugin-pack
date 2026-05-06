//! Softbuffer-based editor for Imagine. CPU rendering via tiny-skia.
//!
//! Layout B:
//! ```text
//! +-----------------------------------------------------------+
//! |                          |   spectrum view (top right)    |
//! |                          +--------------------------------+
//! |     vectorscope panel    |   per-band strip (mid right)   |
//! |     (left ~40%)          +--------------------------------+
//! |                          |   coherence spectrum (bot rt)  |
//! +-----------------------------------------------------------+
//! | global strip: Recover Sides / Link / Quality              |
//! +-----------------------------------------------------------+
//! ```
//!
//! Coordinates are in physical pixels; `scale_factor` is derived from
//! `physical_width / WINDOW_WIDTH`. Mouse handling uses the shared
//! [`tiny_skia_widgets::DragState`] and [`tiny_skia_widgets::TextEditState`]
//! patterns from six-pack.

mod band_strip;
mod global_strip;
mod spectrum_view;
mod vectorscope_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::editor::vectorscope_view::VectorMode;
use crate::vectorscope::VectorConsumer;
use crate::{ImagineParams, Quality, StereoizeModeParam, NUM_BANDS};
use tiny_skia_widgets as widgets;

pub const WINDOW_WIDTH: u32 = 960;
pub const WINDOW_HEIGHT: u32 = 640;
pub const MIN_WIDTH: u32 = 720;
pub const MIN_HEIGHT: u32 = 580;

/// Bottom strip height in logical pixels (scaled by `scale_factor`).
/// 52 px is enough room for a 14 px header label row plus the 30+ px control
/// bodies (Recover bar, Link toggle, Quality selector) and a couple of pixels
/// of padding above and below.
const BOTTOM_STRIP_H: f32 = 52.0;

/// Hit/drag tolerance around split lines, in physical pixels.
const SPLIT_HIT_TOL_PX: f32 = 5.0;

/// Pixels of vertical drag for a full-range Stereoize change.
const STZ_DRAG_PIXELS_PER_FULL: f32 = 200.0;

// ── Hit-action enum ─────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitAction {
    /// Crossover split handle in the spectrum view (idx ∈ {0, 1, 2}).
    Split { idx: usize },
    /// Per-band Width slider (vertical bar). Drag = continuous.
    BandWidth { band: usize },
    /// Per-band Stereoize knob. Drag = vertical anchored.
    BandStz { band: usize },
    /// Per-band Mode I/II toggle (click only).
    BandMode { band: usize },
    /// Per-band Solo button (click only, radio behavior).
    BandSolo { band: usize },
    /// Recover-Sides slider (horizontal). Drag.
    Recover,
    /// Link Bands toggle (click only).
    LinkBands,
    /// Quality selector (click only). `seg`: 0 = Linear, 1 = IIR.
    QualitySeg { seg: usize },
    /// Vectorscope mode toggle area (click only). Toggles Polar ↔ Lissajous.
    VectorMode,
}

/// Snapshot of all per-band Width / Stereoize values at drag start. Used
/// to apply Link-Bands deltas with smallest-headroom clamping.
#[derive(Default, Clone, Copy)]
struct LinkBaseline {
    /// Normalized Width values (0..1) for each band at drag start.
    widths: [f32; NUM_BANDS],
    /// Normalized Stereoize values (0..1) for each band at drag start.
    stzs: [f32; NUM_BANDS],
    /// Normalized value at drag start of the band actually being dragged.
    dragged_baseline: f32,
    /// Anchor mouse-y captured when a Stereoize knob drag began.
    anchor_y: f32,
}

// ── Window handler ──────────────────────────────────────────────────────

struct ImagineWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    pending_resize: Arc<AtomicU64>,

    params: Arc<ImagineParams>,
    vectorscope: Arc<VectorConsumer>,

    /// Scratch buffers reused each frame so the vectorscope drain doesn't
    /// allocate per-frame. The audio thread fills the SPSC ring; the editor
    /// drains into these on the GUI thread.
    vec_l: Vec<f32>,
    vec_r: Vec<f32>,

    /// Glyph cache for CPU text rendering. Owned by the window; passed by
    /// `&mut` reference into each view's draw function.
    text_renderer: widgets::TextRenderer,

    // ── Mouse / keyboard state ───────────────────────────────────────
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    /// Snapshot for Link-Bands delta application during a Width or Stereoize
    /// drag. Captured at button-press time.
    link_baseline: LinkBaseline,

    // ── Cached layout rectangles, refreshed by `compute_layout()` ───
    /// Spectrum panel: (x, y, w, h) in physical pixels.
    spectrum_rect: (i32, i32, i32, i32),
    /// Band strip outer rect.
    band_strip_rect: (i32, i32, i32, i32),
    /// Vectorscope panel rect.
    vectorscope_rect: (i32, i32, i32, i32),
    /// Global strip rect.
    global_strip_rect: (i32, i32, i32, i32),
}

impl ImagineWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<ImagineParams>,
        vectorscope: Arc<VectorConsumer>,
        pending_resize: Arc<AtomicU64>,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Self {
        let surface = widgets::SoftbufferSurface::new(window, physical_width, physical_height);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            surface,
            physical_width,
            physical_height,
            scale_factor,
            pending_resize,
            params,
            vectorscope,
            vec_l: Vec::with_capacity(2048),
            vec_r: Vec::with_capacity(2048),
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            link_baseline: LinkBaseline::default(),
            spectrum_rect: (0, 0, 0, 0),
            band_strip_rect: (0, 0, 0, 0),
            vectorscope_rect: (0, 0, 0, 0),
            global_strip_rect: (0, 0, 0, 0),
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    /// Compute the panel layout (mirrors the geometry used by `draw`) and
    /// cache rectangles for hit-testing. Returns the per-section rects in the
    /// order (vectorscope, spectrum, band, coherence, global).
    fn compute_layout(&mut self) -> Layout {
        let s = self.scale_factor;
        let w = self.physical_width as i32;
        let h = self.physical_height as i32;

        let bottom_h = (BOTTOM_STRIP_H * s).round() as i32;
        let main_h = (h - bottom_h).max(1);

        let left_w = ((w as f32) * 0.4).round() as i32;
        let right_x = left_w;
        let right_w = (w - left_w).max(1);

        let spectrum_h = ((main_h as f32) * 0.50).round() as i32;
        let band_h = ((main_h as f32) * 0.35).round() as i32;
        let coherence_h = (main_h - spectrum_h - band_h).max(1);

        let spectrum_y = 0;
        let band_y = spectrum_y + spectrum_h;
        let coherence_y = band_y + band_h;
        let bottom_y = main_h;

        self.spectrum_rect = (right_x, spectrum_y, right_w, spectrum_h);
        self.band_strip_rect = (right_x, band_y, right_w, band_h);
        self.vectorscope_rect = (0, 0, left_w, main_h);
        self.global_strip_rect = (0, bottom_y, w, bottom_h);

        Layout {
            vectorscope: (0, 0, left_w, main_h),
            spectrum: (right_x, spectrum_y, right_w, spectrum_h),
            band: (right_x, band_y, right_w, band_h),
            coherence: (right_x, coherence_y, right_w, coherence_h),
            global: (0, bottom_y, w, bottom_h),
        }
    }

    fn draw(&mut self) {
        // Clear background.
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, crate::theme::bg());

        let layout = self.compute_layout();

        // Dispatch to view modules. Each view receives `&mut Pixmap` (so it
        // can call into TextRenderer::draw_text) plus `&mut TextRenderer`.
        let pm = &mut self.surface.pixmap;
        vectorscope_view::draw(
            pm,
            layout.vectorscope.0,
            layout.vectorscope.1,
            layout.vectorscope.2,
            layout.vectorscope.3,
            &self.params,
            &self.vectorscope,
            &mut self.vec_l,
            &mut self.vec_r,
            &mut self.text_renderer,
        );
        spectrum_view::draw(
            pm,
            layout.spectrum.0,
            layout.spectrum.1,
            layout.spectrum.2,
            layout.spectrum.3,
            &self.params,
            &mut self.text_renderer,
        );
        band_strip::draw(
            pm,
            layout.band.0,
            layout.band.1,
            layout.band.2,
            layout.band.3,
            &self.params,
            &mut self.text_renderer,
        );
        spectrum_view::draw_coherence(
            pm,
            layout.coherence.0,
            layout.coherence.1,
            layout.coherence.2,
            layout.coherence.3,
            &self.params,
            &mut self.text_renderer,
        );
        global_strip::draw(
            pm,
            layout.global.0,
            layout.global.1,
            layout.global.2,
            layout.global.3,
            &self.params,
            &mut self.text_renderer,
        );
    }

    // ── Param helpers ───────────────────────────────────────────────────

    fn float_for_action(&self, action: HitAction) -> Option<&FloatParam> {
        match action {
            HitAction::Split { idx } => Some(match idx {
                0 => &self.params.xover_1,
                1 => &self.params.xover_2,
                _ => &self.params.xover_3,
            }),
            HitAction::BandWidth { band } => Some(&self.params.bands[band].width),
            HitAction::BandStz { band } => Some(&self.params.bands[band].stz),
            HitAction::Recover => Some(&self.params.recover_sides),
            _ => None,
        }
    }

    fn formatted_value_without_unit(&self, action: HitAction) -> String {
        let Some(p) = self.float_for_action(action) else {
            return String::new();
        };
        let v = p.modulated_normalized_value();
        p.normalized_value_to_string(v, false)
    }

    /// Snapshot all band Width and Stereoize values for delta-clamped Link
    /// behavior. `dragged_baseline` is the value of the field actually being
    /// dragged (chosen by the caller).
    fn snapshot_link_baseline(&mut self, dragged_baseline: f32, anchor_y: f32) {
        let mut widths = [0.0_f32; NUM_BANDS];
        let mut stzs = [0.0_f32; NUM_BANDS];
        for b in 0..NUM_BANDS {
            widths[b] = self.params.bands[b].width.unmodulated_normalized_value();
            stzs[b] = self.params.bands[b].stz.unmodulated_normalized_value();
        }
        self.link_baseline = LinkBaseline {
            widths,
            stzs,
            dragged_baseline,
            anchor_y,
        };
    }

    /// Apply a delta to all four bands' Width values, with smallest-headroom
    /// clamping so no band's `baseline + clamped_delta` leaves [0, 1].
    fn apply_link_widths(&self, setter: &ParamSetter, requested_delta: f32) {
        let mut clamped_delta = requested_delta;
        for b in 0..NUM_BANDS {
            let baseline = self.link_baseline.widths[b];
            let max_up = 1.0 - baseline;
            let max_down = -baseline;
            clamped_delta = clamped_delta.clamp(max_down, max_up);
        }
        for b in 0..NUM_BANDS {
            let target = (self.link_baseline.widths[b] + clamped_delta).clamp(0.0, 1.0);
            setter.set_parameter_normalized(&self.params.bands[b].width, target);
        }
    }

    /// Apply a delta to all four bands' Stereoize values with smallest-headroom
    /// clamping.
    fn apply_link_stzs(&self, setter: &ParamSetter, requested_delta: f32) {
        let mut clamped_delta = requested_delta;
        for b in 0..NUM_BANDS {
            let baseline = self.link_baseline.stzs[b];
            let max_up = 1.0 - baseline;
            let max_down = -baseline;
            clamped_delta = clamped_delta.clamp(max_down, max_up);
        }
        for b in 0..NUM_BANDS {
            let target = (self.link_baseline.stzs[b] + clamped_delta).clamp(0.0, 1.0);
            setter.set_parameter_normalized(&self.params.bands[b].stz, target);
        }
    }

    fn commit_text_edit(&mut self) {
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let Some(p) = self.float_for_action(action) else {
            return;
        };
        if let Some(norm) = p.string_to_normalized_value(&text) {
            let setter = ParamSetter::new(self.gui_context.as_ref());
            setter.begin_set_parameter(p);
            setter.set_parameter_normalized(p, norm);
            setter.end_set_parameter(p);
        }
    }

    // ── Hit testing ─────────────────────────────────────────────────

    /// Hit-test in priority order. Returns the first matching action under
    /// the current mouse position.
    fn hit_test_at(&self, mx: f32, my: f32) -> Option<HitAction> {
        // 1. Spectrum split handles (±SPLIT_HIT_TOL_PX around each split).
        let (sx, sy, sw, sh) = self.spectrum_rect;
        let in_spectrum = sw > 0
            && sh > 0
            && mx >= sx as f32
            && mx < (sx + sw) as f32
            && my >= sy as f32
            && my < (sy + sh) as f32;
        if in_spectrum {
            let freqs = [
                self.params.xover_1.value(),
                self.params.xover_2.value(),
                self.params.xover_3.value(),
            ];
            let mut best: Option<(usize, f32)> = None;
            for (i, &hz) in freqs.iter().enumerate() {
                let lx = spectrum_view::split_pixel_x(sx, sw, hz);
                let dx = (mx - lx as f32).abs();
                if dx <= SPLIT_HIT_TOL_PX {
                    match best {
                        Some((_, best_dx)) if dx >= best_dx => {}
                        _ => best = Some((i, dx)),
                    }
                }
            }
            if let Some((i, _)) = best {
                return Some(HitAction::Split { idx: i });
            }
        }

        // 2. Band strip — Width slider, Stereoize knob, Mode toggle, Solo button.
        let (bx, by, bw, bh) = self.band_strip_rect;
        let in_band_strip = bw > 0
            && bh > 0
            && mx >= bx as f32
            && mx < (bx + bw) as f32
            && my >= by as f32
            && my < (by + bh) as f32;
        if in_band_strip {
            let layout = band_strip::compute_layout(bx, by, bw, bh);
            for i in 0..NUM_BANDS {
                let band_left = layout.band_x[i] as f32;
                let band_right = band_left + layout.band_w as f32;
                if mx < band_left || mx >= band_right {
                    continue;
                }
                let local_x = mx - band_left;
                let local_y = my - layout.y as f32;

                // Width slider rect (relative to band's top-left).
                let (wxr, wyr, wwr, whr) = layout.width_rect;
                if local_x >= wxr as f32
                    && local_x < (wxr + wwr) as f32
                    && local_y >= wyr as f32
                    && local_y < (wyr + whr) as f32
                {
                    return Some(HitAction::BandWidth { band: i });
                }

                // Stereoize knob: round hit area.
                let (cxr, cyr) = layout.stz_center;
                let dx = local_x - cxr as f32;
                let dy = local_y - cyr as f32;
                let r = layout.stz_radius as f32;
                if r > 0.0 && dx * dx + dy * dy <= r * r {
                    return Some(HitAction::BandStz { band: i });
                }

                // Mode toggle.
                let (mxr, myr, mwr, mhr) = layout.mode_rect;
                if local_x >= mxr as f32
                    && local_x < (mxr + mwr) as f32
                    && local_y >= myr as f32
                    && local_y < (myr + mhr) as f32
                {
                    return Some(HitAction::BandMode { band: i });
                }

                // Solo button.
                let (sxr, syr, swr, shr) = layout.solo_rect;
                if local_x >= sxr as f32
                    && local_x < (sxr + swr) as f32
                    && local_y >= syr as f32
                    && local_y < (syr + shr) as f32
                {
                    return Some(HitAction::BandSolo { band: i });
                }
            }
        }

        // 3. Global strip — Recover / Link / Quality.
        let (gx, gy, gw, gh) = self.global_strip_rect;
        let in_global = gw > 0
            && gh > 0
            && mx >= gx as f32
            && mx < (gx + gw) as f32
            && my >= gy as f32
            && my < (gy + gh) as f32;
        if in_global {
            let layout = global_strip::compute_layout(gx, gy, gw, gh);
            let (rx, ry, rw, rh) = layout.recover_rect;
            if mx >= rx as f32 && mx < (rx + rw) as f32 && my >= ry as f32 && my < (ry + rh) as f32
            {
                return Some(HitAction::Recover);
            }
            let (lx, ly, lw, lh) = layout.link_rect;
            if mx >= lx as f32 && mx < (lx + lw) as f32 && my >= ly as f32 && my < (ly + lh) as f32
            {
                return Some(HitAction::LinkBands);
            }
            let (qx, qy, qw, qh) = layout.quality_rect;
            if mx >= qx as f32 && mx < (qx + qw) as f32 && my >= qy as f32 && my < (qy + qh) as f32
            {
                let half_w = qw / 2;
                let seg = if (mx - qx as f32) < half_w as f32 {
                    0
                } else {
                    1
                };
                return Some(HitAction::QualitySeg { seg });
            }
        }

        // 4. Vectorscope mode toggle (bottom-left corner of the vec panel,
        //    above the correlation/balance bars).
        let (vx, vy, vw, vh) = self.vectorscope_rect;
        let in_vec = vw > 0
            && vh > 0
            && mx >= vx as f32
            && mx < (vx + vw) as f32
            && my >= vy as f32
            && my < (vy + vh) as f32;
        if in_vec {
            // Toggle area: bottom-left ~80×16 px just above the meter bars.
            let toggle_x0 = vx as f32 + 6.0;
            let toggle_y1 = (vy + vh) as f32 - 36.0;
            let toggle_y0 = toggle_y1 - 16.0;
            let toggle_x1 = toggle_x0 + 80.0;
            if mx >= toggle_x0 && mx < toggle_x1 && my >= toggle_y0 && my < toggle_y1 {
                return Some(HitAction::VectorMode);
            }
        }

        None
    }

    // ── Drag handling ───────────────────────────────────────────────

    fn handle_drag(&mut self, action: HitAction) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let (mx, my) = self.drag.mouse_pos();

        match action {
            HitAction::Split { idx } => {
                let (sx, _sy, sw, _sh) = self.spectrum_rect;
                if sw <= 0 {
                    return;
                }
                let xn = ((mx - sx as f32) / sw as f32).clamp(0.0, 1.0);
                let hz = spectrum_view::x_to_hz(xn);
                let p = match idx {
                    0 => &self.params.xover_1,
                    1 => &self.params.xover_2,
                    _ => &self.params.xover_3,
                };
                let norm = p.preview_normalized(hz);
                setter.set_parameter_normalized(p, norm);
            }
            HitAction::BandWidth { band } => {
                let (bx, by, bw, bh) = self.band_strip_rect;
                if bw <= 0 || bh <= 0 {
                    return;
                }
                let layout = band_strip::compute_layout(bx, by, bw, bh);
                let (_wxr, wyr, _wwr, whr) = layout.width_rect;
                if whr <= 0 {
                    return;
                }
                let slot_y_top = (layout.y + wyr) as f32;
                let local_y = (my - slot_y_top).clamp(0.0, whr as f32);
                let target = (1.0 - local_y / whr as f32).clamp(0.0, 1.0);
                if self.params.link_bands.value() {
                    let delta = target - self.link_baseline.dragged_baseline;
                    self.apply_link_widths(&setter, delta);
                } else {
                    setter.set_parameter_normalized(&self.params.bands[band].width, target);
                }
            }
            HitAction::BandStz { band } => {
                // Anchored vertical drag.
                let dy = self.link_baseline.anchor_y - my;
                let delta_norm = dy / STZ_DRAG_PIXELS_PER_FULL;
                let target = (self.link_baseline.dragged_baseline + delta_norm).clamp(0.0, 1.0);
                if self.params.link_bands.value() {
                    let actual_delta = target - self.link_baseline.dragged_baseline;
                    self.apply_link_stzs(&setter, actual_delta);
                } else {
                    setter.set_parameter_normalized(&self.params.bands[band].stz, target);
                }
            }
            HitAction::Recover => {
                let (gx, gy, gw, gh) = self.global_strip_rect;
                if gw <= 0 || gh <= 0 {
                    return;
                }
                let layout = global_strip::compute_layout(gx, gy, gw, gh);
                let (rx, _ry, rw, _rh) = layout.recover_rect;
                if rw <= 0 {
                    return;
                }
                let target = ((mx - rx as f32) / rw as f32).clamp(0.0, 1.0);
                setter.set_parameter_normalized(&self.params.recover_sides, target);
            }
            _ => {}
        }
    }

    // ── Press handlers ──────────────────────────────────────────────

    fn handle_left_press(&mut self, action: HitAction) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let (_mx, my) = self.drag.mouse_pos();

        match action {
            HitAction::Split { idx } => {
                let p = match idx {
                    0 => &self.params.xover_1,
                    1 => &self.params.xover_2,
                    _ => &self.params.xover_3,
                };
                setter.begin_set_parameter(p);
                self.drag
                    .begin_drag(action, p.unmodulated_normalized_value(), false);
                // Immediately drag to current mouse position.
                self.handle_drag(action);
            }
            HitAction::BandWidth { band } => {
                let p = &self.params.bands[band].width;
                let norm = p.unmodulated_normalized_value();
                setter.begin_set_parameter(p);
                if self.params.link_bands.value() {
                    // Begin set on all 4 bands' widths so all get end_set_parameter at end.
                    for b in 0..NUM_BANDS {
                        if b != band {
                            setter.begin_set_parameter(&self.params.bands[b].width);
                        }
                    }
                }
                self.snapshot_link_baseline(norm, my);
                self.drag.begin_drag(action, norm, false);
                self.handle_drag(action);
            }
            HitAction::BandStz { band } => {
                let p = &self.params.bands[band].stz;
                let norm = p.unmodulated_normalized_value();
                setter.begin_set_parameter(p);
                if self.params.link_bands.value() {
                    for b in 0..NUM_BANDS {
                        if b != band {
                            setter.begin_set_parameter(&self.params.bands[b].stz);
                        }
                    }
                }
                self.snapshot_link_baseline(norm, my);
                self.drag.begin_drag(action, norm, false);
                // No immediate drag-update (anchored drag — value only changes
                // once the mouse moves away from the anchor).
            }
            HitAction::BandMode { band } => {
                let bp = &self.params.bands[band];
                let cur = bp.mode.value();
                let next = match cur {
                    StereoizeModeParam::I => StereoizeModeParam::Ii,
                    StereoizeModeParam::Ii => StereoizeModeParam::I,
                };
                setter.begin_set_parameter(&bp.mode);
                let next_norm = match next {
                    StereoizeModeParam::I => 0.0,
                    StereoizeModeParam::Ii => 1.0,
                };
                setter.set_parameter_normalized(&bp.mode, next_norm);
                setter.end_set_parameter(&bp.mode);
            }
            HitAction::BandSolo { band } => {
                // Radio: clicking a solo'd band turns it off; clicking an
                // un-solo'd band turns it on and clears all others.
                let cur = self.params.bands[band].solo.value();
                if cur {
                    let p = &self.params.bands[band].solo;
                    setter.begin_set_parameter(p);
                    setter.set_parameter(p, false);
                    setter.end_set_parameter(p);
                } else {
                    for b in 0..NUM_BANDS {
                        let p = &self.params.bands[b].solo;
                        let target = b == band;
                        if self.params.bands[b].solo.value() != target {
                            setter.begin_set_parameter(p);
                            setter.set_parameter(p, target);
                            setter.end_set_parameter(p);
                        }
                    }
                }
            }
            HitAction::Recover => {
                let p = &self.params.recover_sides;
                setter.begin_set_parameter(p);
                self.drag
                    .begin_drag(action, p.unmodulated_normalized_value(), false);
                self.handle_drag(action);
            }
            HitAction::LinkBands => {
                let p = &self.params.link_bands;
                let cur = p.value();
                setter.begin_set_parameter(p);
                setter.set_parameter(p, !cur);
                setter.end_set_parameter(p);
            }
            HitAction::QualitySeg { seg } => {
                let p = &self.params.quality;
                let target = match seg {
                    0 => Quality::Linear,
                    _ => Quality::Iir,
                };
                if p.value() != target {
                    setter.begin_set_parameter(p);
                    let norm = match target {
                        Quality::Linear => 0.0,
                        Quality::Iir => 1.0,
                    };
                    setter.set_parameter_normalized(p, norm);
                    setter.end_set_parameter(p);
                }
            }
            HitAction::VectorMode => {
                // Direct AtomicU32 toggle — vector_mode is not a Param.
                let cur = VectorMode::from_u32(self.params.vector_mode.load(Ordering::Relaxed));
                let next = match cur {
                    VectorMode::Polar => VectorMode::Lissajous as u32,
                    VectorMode::Lissajous => VectorMode::Polar as u32,
                };
                self.params.vector_mode.store(next, Ordering::Release);
            }
        }
    }

    fn handle_right_press(&mut self, action: HitAction) {
        // Only continuous controls open text-entry. Stepped/toggle controls
        // are no-ops on right-click.
        match action {
            HitAction::Split { .. }
            | HitAction::BandWidth { .. }
            | HitAction::BandStz { .. }
            | HitAction::Recover => {
                let initial = self.formatted_value_without_unit(action);
                self.text_edit.begin(action, &initial);
            }
            _ => {}
        }
    }

    fn end_drag_for_action(&self, setter: &ParamSetter, action: HitAction) {
        match action {
            HitAction::Split { idx } => {
                let p = match idx {
                    0 => &self.params.xover_1,
                    1 => &self.params.xover_2,
                    _ => &self.params.xover_3,
                };
                setter.end_set_parameter(p);
            }
            HitAction::BandWidth { band } => {
                setter.end_set_parameter(&self.params.bands[band].width);
                if self.params.link_bands.value() {
                    for b in 0..NUM_BANDS {
                        if b != band {
                            setter.end_set_parameter(&self.params.bands[b].width);
                        }
                    }
                }
            }
            HitAction::BandStz { band } => {
                setter.end_set_parameter(&self.params.bands[band].stz);
                if self.params.link_bands.value() {
                    for b in 0..NUM_BANDS {
                        if b != band {
                            setter.end_set_parameter(&self.params.bands[b].stz);
                        }
                    }
                }
            }
            HitAction::Recover => {
                setter.end_set_parameter(&self.params.recover_sides);
            }
            _ => {}
        }
    }
}

struct Layout {
    vectorscope: (i32, i32, i32, i32),
    spectrum: (i32, i32, i32, i32),
    band: (i32, i32, i32, i32),
    coherence: (i32, i32, i32, i32),
    global: (i32, i32, i32, i32),
}

impl baseview::WindowHandler for ImagineWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        let packed = self.pending_resize.swap(0, Ordering::Relaxed);
        if packed != 0 {
            let new_w = (packed >> 32) as u32;
            let new_h = (packed & 0xFFFF_FFFF) as u32;
            if new_w > 0
                && new_h > 0
                && (new_w != self.physical_width || new_h != self.physical_height)
            {
                window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            }
        }
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if let Some(active) = self.drag.active_action().copied() {
                    self.handle_drag(active);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorLeft) => {
                self.drag.on_cursor_left();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorEntered) => {
                self.drag.on_cursor_entered();
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                // Click-outside auto-commits any in-flight text edit.
                self.commit_text_edit();
                let (mx, my) = self.drag.mouse_pos();
                if let Some(action) = self.hit_test_at(mx, my) {
                    // End any prior drag (defensive — shouldn't normally happen).
                    if let Some(prev) = self.drag.end_drag() {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.end_drag_for_action(&setter, prev);
                    }
                    self.handle_left_press(action);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(active) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_drag_for_action(&setter, active);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                // Ignore right-click during an active drag.
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }
                let (mx, my) = self.drag.mouse_pos();
                if let Some(action) = self.hit_test_at(mx, my) {
                    self.commit_text_edit();
                    self.handle_right_press(action);
                }
            }
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                if ev.state != keyboard_types::KeyState::Down {
                    // Swallow key-up events while editing so the host DAW
                    // doesn't fire shortcuts on release.
                    return baseview::EventStatus::Captured;
                }
                match &ev.key {
                    keyboard_types::Key::Character(s) => {
                        for c in s.chars() {
                            self.text_edit.insert_char(c);
                        }
                    }
                    keyboard_types::Key::Backspace => self.text_edit.backspace(),
                    keyboard_types::Key::Escape => self.text_edit.cancel(),
                    keyboard_types::Key::Enter => self.commit_text_edit(),
                    _ => return baseview::EventStatus::Ignored,
                }
                return baseview::EventStatus::Captured;
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

pub struct ImagineEditor {
    pub params: Arc<ImagineParams>,
    pub vectorscope: Arc<VectorConsumer>,
    pub pending_resize: Arc<AtomicU64>,
}

impl Editor for ImagineEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let vectorscope = Arc::clone(&self.vectorscope);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Imagine"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                ImagineWindow::new(
                    window,
                    gui_context,
                    params,
                    vectorscope,
                    pending_resize,
                    persisted_w,
                    persisted_h,
                    sf,
                )
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let packed = ((width as u64) << 32) | (height as u64);
        self.pending_resize.store(packed, Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_dimensions_within_default() {
        assert!(MIN_WIDTH <= WINDOW_WIDTH);
        assert!(MIN_HEIGHT <= WINDOW_HEIGHT);
    }

    #[test]
    fn pending_resize_packs_round_trip() {
        let r = Arc::new(AtomicU64::new(0));
        let editor = ImagineEditor {
            params: Arc::new(ImagineParams::default()),
            vectorscope: Arc::new(crate::vectorscope::ring_pair().1),
            pending_resize: Arc::clone(&r),
        };
        assert!(editor.set_size(1024, 768));
        let packed = r.load(Ordering::Relaxed);
        assert_eq!((packed >> 32) as u32, 1024);
        assert_eq!((packed & 0xFFFF_FFFF) as u32, 768);
    }

    #[test]
    fn set_size_rejects_zero() {
        let r = Arc::new(AtomicU64::new(0));
        let editor = ImagineEditor {
            params: Arc::new(ImagineParams::default()),
            vectorscope: Arc::new(crate::vectorscope::ring_pair().1),
            pending_resize: Arc::clone(&r),
        };
        assert!(!editor.set_size(0, 768));
        assert!(!editor.set_size(1024, 0));
        assert_eq!(r.load(Ordering::Relaxed), 0);
    }
}
