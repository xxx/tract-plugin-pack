//! Softbuffer-based editor for nap. CPU rendering via tiny-skia.
//!
//! Layout (880×720, freely resizable):
//! - Three stacked curve-only MSEG editors (top→bottom: Decay, Width, Tone),
//!   each editing the matching `Arc<Mutex<MsegData>>` from `NapParams`.
//! - A fixed-height bottom strip with a mode selector (Zero Latency / Efficient)
//!   plus dials (Size / Density / Width / Pre-Delay / Mix / Output) and a Seed stepper.
//!
//! Any curve edit, or any change to a design-time param (Size/Density/Width/
//! Seed), regenerates the velvet sequence and publishes it through the
//! `SequenceHandoff` — the exact GUI→audio pattern miff uses for `rebake()`.
//! When the mode is Efficient, curve/dial edits also bake and publish the IR
//! via the `IrHandoff`; the bake is deferred during continuous MSEG node drags
//! (fast sequence regen still runs) and fires on drag-release.

pub mod tail_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia;
use tiny_skia_widgets as widgets;
use tiny_skia_widgets::mseg::{MsegData, MsegEdit, MsegEditState};

use crate::handoff::{IrHandoff, SequenceHandoff};
use crate::ir::{IrBaker, IrSpectra};
use crate::sequence::VelvetSequence;
use crate::theme::pane_colors;
use crate::{NapMode, NapParams};

// 880 matches miff (the other curve-only MSEG consumer): each full-width pane
// gives the shared MSEG control strip enough room (it needs ≥ 676 px at scale
// 1.0) so Polarity/Style/Randomize don't overlap. Every strip control scales
// with the pane, so this holds at any resize.
pub const WINDOW_WIDTH: u32 = 880;
pub const WINDOW_HEIGHT: u32 = 720;
const MIN_WIDTH: u32 = 420;
const MIN_HEIGHT: u32 = 520;

/// Fixed bottom-strip height (logical pixels, scaled by `scale`).
const STRIP_H: f32 = 90.0;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ───────────────────────────────────────────────────────────

/// Which bottom-strip parameter a dial drag / text-edit targets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DialId {
    Size,
    Density,
    Width,
    PreDelay,
    Input,
    Mix,
    Output,
    Seed,
}

impl DialId {
    /// Left→right order of the dials in the strip.
    const ALL: [DialId; 8] = [
        DialId::Size,
        DialId::Density,
        DialId::Width,
        DialId::PreDelay,
        DialId::Input,
        DialId::Mix,
        DialId::Output,
        DialId::Seed,
    ];

    fn label(self) -> &'static str {
        match self {
            DialId::Size => "Size",
            DialId::Density => "Density",
            DialId::Width => "Width",
            DialId::PreDelay => "Pre-Delay",
            DialId::Input => "Input",
            DialId::Mix => "Mix",
            DialId::Output => "Output",
            DialId::Seed => "Seed",
        }
    }

    /// Whether a change to this param requires regenerating the velvet
    /// sequence (the design-time params).
    fn affects_sequence(self) -> bool {
        matches!(
            self,
            DialId::Size | DialId::Density | DialId::Width | DialId::Seed
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HitAction {
    ModeSelector,
    Dial(DialId),
}

// ── Layout ─────────────────────────────────────────────────────────────────

/// The three equal MSEG panes stacked above the fixed bottom strip.
/// Returns each pane as `(x, y, w, h)`, top→bottom: Decay, Width, Tone.
pub(crate) fn pane_rects(w: f32, h: f32, scale: f32) -> [(f32, f32, f32, f32); 3] {
    let strip_h = STRIP_H * scale;
    let pane_h = ((h - strip_h) / 3.0).max(0.0);
    [
        (0.0, 0.0, w, pane_h),
        (0.0, pane_h, w, pane_h),
        (0.0, 2.0 * pane_h, w, pane_h),
    ]
}

/// Distinct randomizer seed per MSEG pane (Decay/Width/Tone). Widely separated
/// so the three curves randomize as independent streams, never in lockstep,
/// for any realistic number of Randomize clicks.
pub(crate) fn pane_seed(pane: usize) -> u32 {
    const SEEDS: [u32; 3] = [0x0000_0000, 0x5555_5555, 0xAAAA_AAAA];
    SEEDS[pane.min(SEEDS.len() - 1)]
}

/// The bottom strip rect (below the three panes).
pub(crate) fn strip_rect(w: f32, h: f32, scale: f32) -> (f32, f32, f32, f32) {
    let strip_h = STRIP_H * scale;
    (0.0, h - strip_h, w, strip_h)
}

/// Which pane (0..3) contains the vertical coordinate `y`, or `None` if `y`
/// falls in the bottom strip / outside the panes. Width is irrelevant — the
/// panes are full-width and stacked vertically — so only `h`/`scale` matter.
pub(crate) fn pane_at(y: f32, h: f32, scale: f32) -> Option<usize> {
    let r = pane_rects(0.0, h, scale);
    r.iter()
        .position(|&(_, py, _, ph)| ph > 0.0 && y >= py && y < py + ph)
}

/// Width reserved for the mode selector in the bottom strip (logical pixels).
const MODE_W: f32 = 180.0;
/// Height of the mode selector (logical pixels).
const MODE_H: f32 = 34.0;

/// Mode selector rect within the bottom strip — left-aligned, vertically centred.
/// Pure — unit-testable without a window.
pub(crate) fn mode_selector_rect(strip: (f32, f32, f32, f32), scale: f32) -> (f32, f32, f32, f32) {
    let (sx, sy, _sw, sh) = strip;
    let pad = 6.0 * scale;
    let mode_w = MODE_W * scale;
    let mode_h = MODE_H * scale;
    let mode_x = sx + pad;
    let mode_y = sy + (sh - mode_h) * 0.5;
    (mode_x, mode_y, mode_w, mode_h)
}

/// Per-dial center + hit rect within the bottom strip, left→right.
/// The mode selector occupies the leftmost portion; the dials fill the rest.
/// Pure — unit-testable without a window.
pub(crate) fn dial_regions(
    strip: (f32, f32, f32, f32),
    scale: f32,
) -> Vec<((f32, f32, f32, f32), DialId)> {
    let (sx, sy, sw, sh) = strip;
    let pad = 6.0 * scale;
    let mode_w = MODE_W * scale;
    // Dials start after the mode selector.
    let dials_x = sx + pad + mode_w + pad;
    let dials_w = (sx + sw - dials_x - pad).max(0.0);
    let n = DialId::ALL.len() as f32;
    let slot_w = (dials_w / n).max(0.0);
    let mut out = Vec::with_capacity(DialId::ALL.len());
    for (i, &id) in DialId::ALL.iter().enumerate() {
        let rx = dials_x + slot_w * i as f32;
        out.push(((rx, sy, slot_w, sh), id));
    }
    out
}

// ── Window handler ──────────────────────────────────────────────────────────

struct NapWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    /// Shared with the audio thread for publishing baked IR spectra.
    ir_handoff: Arc<IrHandoff>,
    sample_rate: f32,
    /// Scratch buffer reused by every `regenerate()` (GUI-thread alloc only).
    scratch: VelvetSequence,
    /// Snapshot of the last generated sequence, kept for the tail overlay.
    snapshot: VelvetSequence,
    /// Reusable column buffer for `tail_view::decimate` (no per-frame alloc).
    columns: Vec<tail_view::Column>,

    /// Owns the FFT planner + IR scratch buffers for GUI-thread IR baking.
    baker: IrBaker,
    /// Scratch L/R spectra for `bake_ir()` (pre-allocated, GUI-thread only).
    scratch_ir_l: IrSpectra,
    scratch_ir_r: IrSpectra,

    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    /// Three curve-only MSEG editors: [Decay, Width, Tone].
    states: [MsegEditState; 3],
    /// The matching `Arc<Mutex<MsegData>>` from `params` — the SAME Arcs the
    /// audio thread reads through `Nap::regenerate`, so edits persist + take.
    curves: [Arc<Mutex<MsegData>>; 3],

    alt_held: bool,
    shift_held: bool,
    /// Per-pane last-click time/pos for double-click detection.
    last_click_time: std::time::Instant,
    last_click_pos: (f32, f32),

    // ── Per-frame param-change guard ────────────────────────────────────────
    /// Last-seen design-time param values; if any drift (host/preset
    /// automation while the editor is open) we regenerate at the top of frame.
    last_design: (f32, f32, f32, i32),
    /// Last-seen curve documents; a curve changed underneath us (preset load)
    /// also triggers a regenerate.
    last_curves: [MsegData; 3],

    // ── Drag-deferred IR bake ───────────────────────────────────────────────
    /// `true` while a left-button press is being held down inside an MSEG pane
    /// (i.e. a continuous node drag may be in progress). During this window the
    /// IR bake is deferred so dragging nodes stays smooth; it fires on release.
    mseg_dragging: bool,
    /// `true` if a curve changed during an ongoing drag, so we know to bake
    /// the IR on mouse-up even when mode is Efficient.
    ir_needs_bake: bool,
}

impl NapWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<NapParams>,
        handoff: Arc<SequenceHandoff>,
        ir_handoff: Arc<IrHandoff>,
        sample_rate: f32,
        shared_scale: Arc<AtomicCell<f32>>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        let curves = [
            params.decay_curve.clone(),
            params.width_curve.clone(),
            params.tone_curve.clone(),
        ];
        let mut states = [
            MsegEditState::new_curve_only(),
            MsegEditState::new_curve_only(),
            MsegEditState::new_curve_only(),
        ];
        // Give each pane a distinct randomizer stream so Randomize never moves
        // the three curves in lockstep (they'd otherwise all start at seed 0).
        for (i, s) in states.iter_mut().enumerate() {
            s.set_randomize_seed(pane_seed(i));
        }

        // Seed the guard caches with the current state so the first frame does
        // not spuriously regenerate.
        let last_design = current_design(&params);
        let last_curves = read_curves(&curves);

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            pending_resize,
            params: params.clone(),
            handoff,
            ir_handoff,
            sample_rate,
            scratch: VelvetSequence::new(),
            snapshot: VelvetSequence::new(),
            columns: Vec::new(),
            baker: IrBaker::new(sample_rate),
            scratch_ir_l: IrSpectra::new(sample_rate),
            scratch_ir_r: IrSpectra::new(sample_rate),
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            states,
            curves,
            alt_held: false,
            shift_held: false,
            last_click_time: std::time::Instant::now(),
            last_click_pos: (-999.0, -999.0),
            last_design,
            last_curves,
            mseg_dragging: false,
            ir_needs_bake: false,
        }
    }

    /// Regenerate the velvet sequence from the current params + curves and
    /// publish it to the audio thread. GUI-thread only. Mirrors miff's
    /// `rebake()`. Refreshes the per-frame guard caches so they stay in sync,
    /// and copies the freshly-generated sequence into `snapshot` for the tail
    /// overlay (so the overlay always matches what was published).
    fn regenerate(&mut self) {
        crate::Nap::regenerate(
            &self.handoff,
            &self.params,
            self.sample_rate,
            &mut self.scratch,
        );
        self.snapshot.copy_from(&self.scratch);
        self.last_design = current_design(&self.params);
        self.last_curves = read_curves(&self.curves);
    }

    /// Bake and publish the IR if the mode is Efficient. GUI-thread only.
    /// No-op in Zero Latency mode. Designed to be called after discrete edits
    /// (node add/delete, dial change, mode switch) and on drag-release.
    fn bake_ir_if_efficient(&mut self) {
        if self.params.mode.value() == NapMode::Efficient {
            crate::Nap::bake_ir(
                &self.ir_handoff,
                &self.params,
                self.sample_rate,
                &mut self.scratch,
                &mut self.baker,
                &mut self.scratch_ir_l,
                &mut self.scratch_ir_r,
            );
        }
    }

    /// Apply a mode change by variant index (0 = Zero Latency, 1 = Efficient).
    /// When switching to Efficient, immediately bakes the IR so audio updates
    /// without waiting for the next edit.
    fn set_mode(&mut self, variant: usize) {
        let target = match variant {
            0 => NapMode::ZeroLatency,
            _ => NapMode::Efficient,
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let norm = self.params.mode.preview_normalized(target);
        setter.begin_set_parameter(&self.params.mode);
        setter.set_parameter_normalized(&self.params.mode, norm);
        setter.end_set_parameter(&self.params.mode);
        // Bake immediately when switching to Efficient so the audio thread has
        // the IR ready without waiting for the next design-time edit.
        if target == NapMode::Efficient {
            self.bake_ir_if_efficient();
        }
    }

    /// Top-of-frame guard: if a design-time param or a curve changed under us
    /// (host/preset automation while the editor is open), regenerate and (if
    /// Efficient mode) bake the IR.
    fn check_external_changes(&mut self) {
        let design = current_design(&self.params);
        let curves = read_curves(&self.curves);
        if design != self.last_design || curves != self.last_curves {
            self.regenerate();
            // External change (preset load, host automation): bake immediately.
            self.bake_ir_if_efficient();
        }
    }

    fn float_param(&self, id: DialId) -> Option<&FloatParam> {
        match id {
            DialId::Size => Some(&self.params.size),
            DialId::Density => Some(&self.params.density),
            DialId::Width => Some(&self.params.width),
            DialId::PreDelay => Some(&self.params.predelay),
            DialId::Input => Some(&self.params.input),
            DialId::Mix => Some(&self.params.mix),
            DialId::Output => Some(&self.params.output),
            DialId::Seed => None,
        }
    }

    /// Current normalized value for any dial (float or the int Seed).
    fn dial_normalized(&self, id: DialId) -> f32 {
        use nih_plug::prelude::Param;
        match self.float_param(id) {
            Some(p) => p.unmodulated_normalized_value(),
            None => self.params.seed.unmodulated_normalized_value(),
        }
    }

    /// Formatted display value (with unit) for any dial.
    fn dial_value_text(&self, id: DialId) -> String {
        use nih_plug::prelude::Param;
        match self.float_param(id) {
            Some(p) => p.normalized_value_to_string(p.modulated_normalized_value(), true),
            None => {
                let p = &self.params.seed;
                p.normalized_value_to_string(p.modulated_normalized_value(), true)
            }
        }
    }

    /// Formatted value without unit (for seeding text entry).
    fn dial_value_without_unit(&self, id: DialId) -> String {
        use nih_plug::prelude::Param;
        match self.float_param(id) {
            Some(p) => p.normalized_value_to_string(p.modulated_normalized_value(), false),
            None => {
                let p = &self.params.seed;
                p.normalized_value_to_string(p.modulated_normalized_value(), false)
            }
        }
    }

    /// Set a dial to a normalized value within an open gesture. Float params
    /// route through their own setter; Seed routes through the int param.
    fn set_dial_normalized(&self, setter: &ParamSetter, id: DialId, norm: f32) {
        match self.float_param(id) {
            Some(p) => setter.set_parameter_normalized(p, norm),
            None => setter.set_parameter_normalized(&self.params.seed, norm),
        }
    }

    fn begin_dial(&self, setter: &ParamSetter, id: DialId) {
        match self.float_param(id) {
            Some(p) => setter.begin_set_parameter(p),
            None => setter.begin_set_parameter(&self.params.seed),
        }
    }

    fn end_dial(&self, setter: &ParamSetter, id: DialId) {
        match self.float_param(id) {
            Some(p) => setter.end_set_parameter(p),
            None => setter.end_set_parameter(&self.params.seed),
        }
    }

    fn reset_dial_to_default(&self, setter: &ParamSetter, id: DialId) {
        use nih_plug::prelude::Param;
        match self.float_param(id) {
            Some(p) => {
                setter.begin_set_parameter(p);
                setter.set_parameter_normalized(p, p.default_normalized_value());
                setter.end_set_parameter(p);
            }
            None => {
                let p = &self.params.seed;
                setter.begin_set_parameter(p);
                setter.set_parameter_normalized(p, p.default_normalized_value());
                setter.end_set_parameter(p);
            }
        }
    }

    fn commit_text_edit(&mut self) {
        use nih_plug::prelude::Param;
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        match action {
            HitAction::ModeSelector => {
                // Mode selector is not a text-entry control — no-op.
            }
            HitAction::Dial(id) => {
                let norm = match self.float_param(id) {
                    Some(p) => p.string_to_normalized_value(&text),
                    None => self.params.seed.string_to_normalized_value(&text),
                };
                let Some(norm) = norm else { return };
                let setter = ParamSetter::new(self.gui_context.as_ref());
                self.begin_dial(&setter, id);
                self.set_dial_normalized(&setter, id, norm);
                self.end_dial(&setter, id);
                if id.affects_sequence() {
                    self.regenerate();
                    // Discrete text-edit: bake IR immediately (no drag in flight).
                    self.bake_ir_if_efficient();
                }
            }
        }
    }

    fn resize_buffers(&mut self) {
        self.surface.resize_and_persist(
            self.physical_width,
            self.physical_height,
            &self.params.editor_state,
        );
    }

    /// Records this click and returns `true` if it forms a double-click
    /// (within 400 ms and 8 px of the previous one).
    fn double_click_check(&mut self, x: f32, y: f32) -> bool {
        let now = std::time::Instant::now();
        let elapsed_ms = now.duration_since(self.last_click_time).as_millis();
        let (px, py) = self.last_click_pos;
        let dist_sq = (x - px) * (x - px) + (y - py) * (y - py);
        let is_double = elapsed_ms < 400 && dist_sq < 64.0;
        self.last_click_time = now;
        self.last_click_pos = (x, y);
        is_double
    }

    fn update_modifiers(&mut self, modifiers: &keyboard_types::Modifiers) {
        let new_alt = modifiers.contains(keyboard_types::Modifiers::ALT);
        let new_shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
        if new_alt != self.alt_held {
            self.alt_held = new_alt;
            for s in &mut self.states {
                s.set_stepped_draw(new_alt);
            }
        }
        self.shift_held = new_shift;
    }

    // ── Drawing ──────────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let s = self.scale_factor;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let rects = pane_rects(w, h, s);
        let colors = pane_colors();
        let labels = ["Decay", "Width", "Tone"];

        for p in 0..3 {
            let curve = self.curves[p].lock().map(|c| *c).unwrap_or_default();
            widgets::mseg::draw_mseg(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                rects[p],
                &curve,
                &self.states[p],
                s,
                colors[p],
                None,
            );
            // Per-pane label in the top-left of the pane.
            let label_size = 13.0 * s;
            self.text_renderer.draw_text(
                &mut self.surface.pixmap,
                rects[p].0 + 8.0 * s,
                rects[p].1 + label_size + 4.0 * s,
                labels[p],
                label_size,
                colors[p],
            );
        }
        // Tail overlay: draw the decimated pulse field over the Decay pane
        // (pane 0). Cost is O(plot_width) regardless of pulse count because
        // `decimate` caps to `cols` columns.
        self.draw_tail_overlay(rects[0], s);

        // Dropdown popups for the panes (drawn after every pane so an open
        // grid/style popup overlays the curve beneath it).
        for (state, rect) in self.states.iter().zip(rects.iter()) {
            widgets::mseg::draw_mseg_dropdown(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                state,
                *rect,
            );
        }

        self.draw_strip(strip_rect(w, h, s), s);
    }

    /// Draw the tail pulse-field overlay over the Decay pane (`rect`).
    ///
    /// Decimates `self.snapshot` to the plot's pixel width, then for each
    /// populated column draws a vertical stick whose height is proportional to
    /// `coeff_abs` (value axis), tinted by dictionary index, with a tiny
    /// horizontal offset that encodes the L/R split. All drawing uses
    /// `widgets::draw_rect` so no unsafe code is needed.
    fn draw_tail_overlay(&mut self, rect: (f32, f32, f32, f32), scale: f32) {
        use widgets::mseg::{mseg_layout, phase_to_x, value_to_y};

        let layout = mseg_layout(rect, true, scale);
        let plot_w = layout.plot.2.max(1.0) as usize;

        // Decimate into the reusable column buffer.
        tail_view::decimate(&self.snapshot, plot_w, &mut self.columns);

        // Per-column rendering constants.
        let stick_w = 1.0_f32.max(scale * 0.75);
        let bottom_y = value_to_y(&layout, 0.0);
        let pane_colors = crate::theme::pane_colors();
        // Filter-index brightness ramp: 6 filters, index 0 = darkest, 5 = brightest.
        // Blend between the decay pane accent (warm amber) dimmed vs bright.
        let num_filters = crate::coloration::Q as f32;

        for (col_idx, col) in self.columns.iter().enumerate() {
            if !col.present || col.coeff_abs < 1e-6 {
                continue;
            }

            // Phase for this column (centre of the column bucket).
            let phase = col_idx as f32 / (plot_w - 1).max(1) as f32;
            let cx = phase_to_x(&layout, phase);

            // Stick top: coeff_abs mapped via value_to_y (value 0 = bottom,
            // coeff_abs = height above bottom). Clamp to the plot.
            let top_y = value_to_y(&layout, col.coeff_abs.clamp(0.0, 1.0));
            if top_y >= bottom_y {
                continue; // degenerate / zero height
            }

            // Color: tint the decay accent by filter index brightness.
            // Brighter (higher) filter_idx → more vivid, less dimmed.
            let brightness = (col.filter_idx as f32 + 1.0) / num_filters;
            let base = pane_colors[0]; // amber accent for the Decay pane
            let r = (base.red() * brightness).clamp(0.0, 1.0);
            let g = (base.green() * brightness).clamp(0.0, 1.0);
            let b_ch = (base.blue() * brightness).clamp(0.0, 1.0);
            let color = tiny_skia::Color::from_rgba(r, g, b_ch, 0.75).unwrap_or(base);

            // Horizontal L/R split offset: scale lr_split to ±a few pixels
            // (capped at ±3 px regardless of the raw sample delta).
            let split_px = if self.snapshot.tail_len > 0 {
                let split_phase =
                    (col.lr_split as f32 / self.snapshot.tail_len as f32).clamp(-1.0, 1.0);
                (split_phase * 3.0 * scale).round()
            } else {
                0.0
            };

            // Left stick (no offset, full coeff height).
            widgets::draw_rect(
                &mut self.surface.pixmap,
                cx - stick_w * 0.5,
                top_y,
                stick_w,
                bottom_y - top_y,
                color,
            );

            // Right-channel indicator: a small horizontal notch offset by
            // split_px, drawn at the midpoint of the stick height.
            if split_px.abs() >= 1.0 {
                let mid_y = (top_y + bottom_y) * 0.5;
                widgets::draw_rect(
                    &mut self.surface.pixmap,
                    cx + split_px - stick_w * 0.5,
                    mid_y - scale,
                    stick_w,
                    2.0 * scale,
                    color,
                );
            }
        }
    }

    /// Draw the bottom strip: mode selector + dials. Registers all hit regions.
    fn draw_strip(&mut self, strip: (f32, f32, f32, f32), scale: f32) {
        let (sx, sy, sw, _sh) = strip;

        // Top separator rule.
        widgets::draw_rect(
            &mut self.surface.pixmap,
            sx,
            sy,
            sw,
            1.0,
            widgets::color_border(),
        );

        // ── Mode selector ────────────────────────────────────────────────────
        let (mx, my, mw, mh) = mode_selector_rect(strip, scale);
        self.drag
            .push_region(mx, my, mw, mh, HitAction::ModeSelector);
        let mode_idx = if self.params.mode.value() == NapMode::ZeroLatency {
            0usize
        } else {
            1
        };
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mx,
            my,
            mw,
            mh,
            &["Zero Latency", "Efficient"],
            mode_idx,
            None,
        );

        // ── Dials ────────────────────────────────────────────────────────────
        let regions = dial_regions(strip, scale);
        let radius = 24.0 * scale;

        for ((rx, ry, rw, rh), id) in regions {
            self.drag.push_region(rx, ry, rw, rh, HitAction::Dial(id));

            let cx = rx + rw * 0.5;
            let cy = ry + rh * 0.5;
            let normalized = self.dial_normalized(id);
            let value_text = self.dial_value_text(id);
            let editing_buf = self
                .text_edit
                .active_for(&HitAction::Dial(id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();

            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                cx,
                cy,
                radius,
                id.label(),
                &value_text,
                normalized,
                None,
                editing_buf.as_deref(),
                caret,
                crate::theme::color_strip_accent(),
            );
        }
    }
}

/// Snapshot of the design-time params for the change guard. Seed is stored as
/// its raw int (exact compare); the floats are exact since they don't change
/// except via the setter.
fn current_design(params: &NapParams) -> (f32, f32, f32, i32) {
    (
        params.size.value(),
        params.density.value(),
        params.width.value(),
        params.seed.value(),
    )
}

fn read_curves(curves: &[Arc<Mutex<MsegData>>; 3]) -> [MsegData; 3] {
    [
        curves[0].lock().map(|c| *c).unwrap_or_default(),
        curves[1].lock().map(|c| *c).unwrap_or_default(),
        curves[2].lock().map(|c| *c).unwrap_or_default(),
    ]
}

impl baseview::WindowHandler for NapWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        widgets::consume_pending_resize(
            &self.pending_resize,
            (self.physical_width, self.physical_height),
            window,
        );
        // Pick up host/preset-driven changes to params or curves while open.
        self.check_external_changes();
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;
        let s = self.scale_factor;

        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width.max(MIN_WIDTH);
                self.physical_height = info.physical_size().height.max(MIN_HEIGHT);
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }

            baseview::Event::Mouse(baseview::MouseEvent::CursorEntered) => {
                self.drag.on_cursor_entered();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorLeft) => {
                self.drag.on_cursor_left();
            }

            // ── CursorMoved ──────────────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                let x = position.x as f32;
                let y = position.y as f32;
                self.drag.set_mouse(x, y);
                self.update_modifiers(modifiers);

                // A live dial drag takes priority over MSEG hover/drag.
                if let Some(HitAction::Dial(id)) = self.drag.active_action().copied() {
                    let shift = self.shift_held;
                    let current = self.dial_normalized(id);
                    if let Some(norm) = self.drag.update_drag(shift, current) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_dial_normalized(&setter, id, norm);
                        if id.affects_sequence() {
                            self.regenerate();
                            // Dial drag: defer IR bake to drag-end (on_mouse_up).
                            self.ir_needs_bake = true;
                        }
                    }
                } else if let Some(p) = pane_at(y, h, s) {
                    // Route the move to whichever pane the cursor is in.
                    let rect = pane_rects(w, h, s)[p];
                    let changed = {
                        if let Ok(mut curve) = self.curves[p].lock() {
                            self.states[p].on_mouse_move(x, y, &mut curve, rect, s, self.shift_held)
                        } else {
                            None
                        }
                    };
                    if changed == Some(MsegEdit::Changed) {
                        self.regenerate();
                        // During a node drag, defer IR bake to mouse-up.
                        if self.mseg_dragging {
                            self.ir_needs_bake = true;
                        } else {
                            self.bake_ir_if_efficient();
                        }
                    }
                } else {
                    // Cursor is in the bottom strip with no dial drag active.
                    // A node drag started in a pane must keep tracking even when
                    // the pointer strays below it, so forward the move to every
                    // pane — only the pane with the active drag mutates its
                    // curve (the rest no-op). Mirrors miff's cross-boundary
                    // forwarding (miff/src/editor.rs ~666–683).
                    for p in 0..3 {
                        let rect = pane_rects(w, h, s)[p];
                        let changed = {
                            if let Ok(mut curve) = self.curves[p].lock() {
                                self.states[p].on_mouse_move(
                                    x,
                                    y,
                                    &mut curve,
                                    rect,
                                    s,
                                    self.shift_held,
                                )
                            } else {
                                None
                            }
                        };
                        if changed == Some(MsegEdit::Changed) {
                            self.regenerate();
                            // During a node drag, defer IR bake to mouse-up.
                            if self.mseg_dragging {
                                self.ir_needs_bake = true;
                            } else {
                                self.bake_ir_if_efficient();
                            }
                        }
                    }
                }
            }

            // ── Left button pressed ──────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let (x, y) = self.drag.mouse_pos();
                self.update_modifiers(modifiers);
                self.commit_text_edit();

                if let Some(p) = pane_at(y, h, s) {
                    let rect = pane_rects(w, h, s)[p];
                    // Only a double-click on the canvas deletes/inserts a node;
                    // strip clicks (snap/grid/style/randomize) are immediate.
                    let strip = widgets::mseg::mseg_layout(rect, true, s).strip;
                    let on_strip = x >= strip.0
                        && x < strip.0 + strip.2
                        && y >= strip.1
                        && y < strip.1 + strip.3;
                    // MSEG double-click is POSITION-based (`double_click_check`):
                    // two clicks close in time AND space on the canvas insert /
                    // delete a node. (The dial reset below uses an ACTION-based
                    // double-click on the same widget — see `check_double_click`.)
                    let is_double = !on_strip && self.double_click_check(x, y);
                    let changed = {
                        if let Ok(mut curve) = self.curves[p].lock() {
                            if is_double {
                                self.states[p].on_double_click(x, y, &mut curve, rect, s)
                            } else {
                                let ctrl = modifiers.contains(keyboard_types::Modifiers::CONTROL);
                                self.states[p].on_mouse_down(x, y, &mut curve, rect, s, ctrl)
                            }
                        } else {
                            None
                        }
                    };
                    if changed == Some(MsegEdit::Changed) {
                        self.regenerate();
                        if is_double {
                            // Double-click node add/delete is a discrete edit — bake immediately.
                            self.bake_ir_if_efficient();
                        } else {
                            // Mouse-down with potential drag starting: mark dragging,
                            // defer bake. We'll bake on mouse-up if the curve changed.
                            self.mseg_dragging = true;
                            self.ir_needs_bake = true;
                        }
                    } else {
                        // Mouse-down in pane: a drag may start (even if no immediate change).
                        self.mseg_dragging = true;
                    }
                } else {
                    // Bottom strip: mode selector, dial drag / double-click reset.
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_dial(&setter, id);
                    }
                    if let Some(region) = self.drag.hit_test().cloned() {
                        let is_double = self.drag.check_double_click(&region.action);
                        match region.action {
                            HitAction::ModeSelector => {
                                // Determine which half of the selector was clicked.
                                let (mx, _, mw, _) = mode_selector_rect(strip_rect(w, h, s), s);
                                let variant = if x - mx < mw * 0.5 { 0 } else { 1 };
                                self.set_mode(variant);
                            }
                            HitAction::Dial(id) => {
                                // Dial double-click is ACTION-based (`check_double_click`):
                                // two clicks on the SAME dial within the window reset it
                                // to its default (vs the MSEG's position-based check above).
                                if is_double {
                                    self.reset_dial_to_default(&setter, id);
                                    if id.affects_sequence() {
                                        self.regenerate();
                                        // Discrete reset: bake immediately.
                                        self.bake_ir_if_efficient();
                                    }
                                } else {
                                    let norm = self.dial_normalized(id);
                                    self.drag.begin_drag(
                                        HitAction::Dial(id),
                                        norm,
                                        self.shift_held,
                                    );
                                    self.begin_dial(&setter, id);
                                }
                            }
                        }
                    }
                }
            }

            // ── Left button released ─────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                // A release always ends any in-flight MSEG drag in every pane.
                for p in 0..3 {
                    let rect = pane_rects(w, h, s)[p];
                    let changed = {
                        if let Ok(mut curve) = self.curves[p].lock() {
                            self.states[p].on_mouse_up(&mut curve, rect, s)
                        } else {
                            None
                        }
                    };
                    if changed == Some(MsegEdit::Changed) {
                        self.regenerate();
                        self.ir_needs_bake = true;
                    }
                }

                // Clear MSEG drag tracking; bake on release if deferred.
                self.mseg_dragging = false;
                if self.ir_needs_bake {
                    self.ir_needs_bake = false;
                    self.bake_ir_if_efficient();
                }

                // End any dial gesture.
                if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_dial(&setter, id);
                    if id.affects_sequence() {
                        self.regenerate();
                        // Dial drag end: discrete bake.
                        self.bake_ir_if_efficient();
                    }
                }
            }

            // ── Right button pressed ─────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                let (x, y) = self.drag.mouse_pos();
                // Right-click during an active dial drag: ignore.
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }
                if let Some(p) = pane_at(y, h, s) {
                    let rect = pane_rects(w, h, s)[p];
                    let changed = {
                        if let Ok(mut curve) = self.curves[p].lock() {
                            self.states[p].on_right_click(x, y, &mut curve, rect, s)
                        } else {
                            None
                        }
                    };
                    if changed == Some(MsegEdit::Changed) {
                        self.regenerate();
                        // Right-click curve edit is discrete — bake immediately.
                        self.bake_ir_if_efficient();
                    }
                } else {
                    // Right-click on a dial opens text entry; mode selector has none.
                    self.commit_text_edit();
                    if let Some(region) = self.drag.hit_test().cloned() {
                        if let HitAction::Dial(id) = region.action {
                            let initial = self.dial_value_without_unit(id);
                            self.text_edit.begin(HitAction::Dial(id), &initial);
                        }
                        // HitAction::ModeSelector: no text entry on the mode selector.
                    }
                }
            }

            // ── Keyboard ─────────────────────────────────────────────────────
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                if ev.state != keyboard_types::KeyState::Down {
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
            baseview::Event::Keyboard(ev) => {
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Ignored;
                }
                match &ev.key {
                    keyboard_types::Key::Delete | keyboard_types::Key::Backspace => {
                        // Delete the selection in whichever pane has the cursor.
                        let (_, y) = self.drag.mouse_pos();
                        if let Some(p) = pane_at(y, h, s) {
                            let changed = if let Ok(mut curve) = self.curves[p].lock() {
                                self.states[p].delete_selection(&mut curve)
                            } else {
                                None
                            };
                            if changed == Some(MsegEdit::Changed) {
                                self.regenerate();
                                // Node deletion is discrete — bake IR immediately.
                                self.bake_ir_if_efficient();
                                return baseview::EventStatus::Captured;
                            }
                        }
                        return baseview::EventStatus::Ignored;
                    }
                    _ => return baseview::EventStatus::Ignored,
                }
            }

            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────────

pub(crate) struct NapEditor {
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    ir_handoff: Arc<IrHandoff>,
    sample_rate: f32,
    scaling_factor: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,
}

pub fn create(
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    ir_handoff: Arc<IrHandoff>,
    sample_rate: f32,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(NapEditor {
        params,
        handoff,
        ir_handoff,
        sample_rate,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for NapEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let handoff = Arc::clone(&self.handoff);
        let ir_handoff = Arc::clone(&self.ir_handoff);
        let sample_rate = self.sample_rate;
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Nap"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                NapWindow::new(
                    window,
                    gui_context,
                    params,
                    handoff,
                    ir_handoff,
                    sample_rate,
                    shared_scale,
                    pending_resize,
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

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
        true
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

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_panes_partition_the_area_above_the_strip() {
        let (w, h, scale) = (880.0, 720.0, 1.0);
        let r = pane_rects(w, h, scale);
        assert!((r[0].1 - 0.0).abs() < 1e-3);
        assert!(
            (r[1].1 - r[0].3).abs() < 1e-3,
            "pane 1 starts where pane 0 ends"
        );
        assert!(
            r[2].1 + r[2].3 <= h - STRIP_H + 1e-3,
            "panes sit above the {STRIP_H}px strip"
        );
    }

    #[test]
    fn pane_at_maps_y_to_pane_index() {
        let (h, scale) = (720.0, 1.0);
        assert_eq!(pane_at(5.0, h, scale), Some(0));
        assert_eq!(pane_at(h - 95.0, h, scale), Some(2));
        assert_eq!(pane_at(h - 10.0, h, scale), None); // in the strip
    }

    #[test]
    fn pane_seeds_are_distinct() {
        // The three MSEG panes must seed their randomizers differently, else
        // Randomize moves all three curves in lockstep.
        let s: Vec<u32> = (0..3).map(pane_seed).collect();
        assert_eq!(s.len(), 3);
        assert_ne!(s[0], s[1]);
        assert_ne!(s[1], s[2]);
        assert_ne!(s[0], s[2]);
    }

    #[test]
    fn dial_regions_cover_all_dials_within_the_strip() {
        let (w, h, scale) = (880.0_f32, 720.0_f32, 1.0_f32);
        let strip = strip_rect(w, h, scale);
        let regions = dial_regions(strip, scale);
        // All eight dials (Size, Density, Width, Pre-Delay, Input, Mix,
        // Output, Seed) are laid out.
        assert_eq!(regions.len(), 8);
        assert_eq!(regions.len(), DialId::ALL.len());
        for ((rx, ry, rw, rh), _) in &regions {
            assert!(*rx >= strip.0 - 0.5 && rx + rw <= strip.0 + strip.2 + 0.5);
            assert!(*ry >= strip.1 - 0.5 && ry + rh <= strip.1 + strip.3 + 0.5);
        }
    }

    #[test]
    fn mode_selector_is_within_strip_and_does_not_overlap_dials() {
        // The mode selector must lie inside the strip and must not overlap any
        // dial region. Both are pure layout functions, no window needed.
        let (w, h, scale) = (880.0_f32, 720.0_f32, 1.0_f32);
        let strip = strip_rect(w, h, scale);
        let (sx, sy, sw, sh) = strip;

        let (mx, my, mw, mh) = mode_selector_rect(strip, scale);

        // Must be inside the strip (with small float tolerance).
        assert!(
            mx >= sx - 0.5 && mx + mw <= sx + sw + 0.5,
            "mode selector x-range [{mx}, {}] outside strip x-range [{sx}, {}]",
            mx + mw,
            sx + sw
        );
        assert!(
            my >= sy - 0.5 && my + mh <= sy + sh + 0.5,
            "mode selector y-range [{my}, {}] outside strip y-range [{sy}, {}]",
            my + mh,
            sy + sh
        );

        // Must not overlap any dial region.
        let dial_regs = dial_regions(strip, scale);
        for ((rx, ry, rw, rh), id) in &dial_regs {
            let overlap = mx < rx + rw && mx + mw > *rx && my < ry + rh && my + mh > *ry;
            assert!(
                !overlap,
                "mode selector overlaps dial {:?}: selector ({mx},{my},{mw},{mh}) vs dial ({rx},{ry},{rw},{rh})",
                id
            );
        }
    }
}
