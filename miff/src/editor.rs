//! Softbuffer-based editor for miff. CPU rendering via tiny-skia.
//!
//! Layout (880×620, freely resizable):
//! - Top region (~55%): MSEG editor (curve-only mode)
//! - Middle region (~29%): Frequency-response view (Task 12)
//! - Bottom strip (~16%): Controls placeholder (Task 10)

pub mod response_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia_widgets as widgets;
use tiny_skia_widgets::mseg::{MsegEdit, MsegEditState};

use crate::MiffParams;

// ── Event target ─────────────────────────────────────────────────────────

/// Where an input event should be dispatched.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EventTarget {
    /// The event is inside the MSEG editor region.
    Mseg,
    /// The event is outside the MSEG region (controls strip, response area).
    Controls,
}

/// Pure routing helper: `Mseg` if `(x, y)` is inside `mseg_rect`, else `Controls`.
/// Unit-testable without a window.
pub(crate) fn event_target(mseg_rect: (f32, f32, f32, f32), x: f32, y: f32) -> EventTarget {
    let (rx, ry, rw, rh) = mseg_rect;
    if x >= rx && x < rx + rw && y >= ry && y < ry + rh {
        EventTarget::Mseg
    } else {
        EventTarget::Controls
    }
}

pub const WINDOW_WIDTH: u32 = 880;
pub const WINDOW_HEIGHT: u32 = 620;
// Used in the Resized event handler.
const MIN_WIDTH: u32 = 680;
const MIN_HEIGHT: u32 = 480;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

/// Hit actions for miff's editor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HitAction {
    ModeSelector,
    MixDial,
    GainDial,
    LengthDial,
}

// ── Layout ───────────────────────────────────────────────────────────────

/// Compute the three editor regions from physical pixel dimensions.
/// Returns `(mseg_rect, response_rect, strip_rect)` each as `(x, y, w, h)`.
#[allow(clippy::type_complexity)]
pub(crate) fn layout(
    w: f32,
    h: f32,
) -> (
    (f32, f32, f32, f32),
    (f32, f32, f32, f32),
    (f32, f32, f32, f32),
) {
    // full-width regions; sub-widgets have their own internal padding
    let mseg_h = h * 0.55;
    let response_h = h * 0.29;
    let strip_h = h - mseg_h - response_h;

    let mseg_rect = (0.0, 0.0, w, mseg_h);
    let response_rect = (0.0, mseg_h, w, response_h);
    let strip_rect = (0.0, mseg_h + response_h, w, strip_h);

    (mseg_rect, response_rect, strip_rect)
}

// ── Strip layout helper ──────────────────────────────────────────────────

/// Sizing constants for the bottom control strip.
const STRIP_PAD: f32 = 8.0;
const MODE_W: f32 = 160.0;
const MODE_H: f32 = 28.0;
const DIAL_RADIUS: f32 = 26.0;

/// Compute the four control rects within `strip_rect` at `scale`.
///
/// Returns `Vec<((x, y, w, h), HitAction)>` for each control, left→right:
/// Mode selector, Mix dial, Gain dial, Length dial.
/// This is a pure function and is unit-testable without a window.
pub(crate) fn strip_regions(
    strip_rect: (f32, f32, f32, f32),
    scale: f32,
) -> Vec<((f32, f32, f32, f32), HitAction)> {
    let (sx, sy, sw, sh) = strip_rect;
    let pad = STRIP_PAD * scale;
    let mode_w = MODE_W * scale;
    let mode_h = MODE_H * scale;

    // Mode selector: left-aligned, vertically centred in the strip.
    let mode_x = sx + pad;
    let mode_y = sy + (sh - mode_h) * 0.5;

    // Three dials share the remaining horizontal space to the right.
    let dials_x_start = mode_x + mode_w + pad;
    let dials_region_w = (sx + sw) - dials_x_start - pad;
    let dial_hit = DIAL_RADIUS * 3.2 * scale;
    let dial_cy = sy + sh * 0.5;

    // Place three dials evenly across dials_region_w.
    let dial_labels = [
        (HitAction::MixDial, 0usize),
        (HitAction::GainDial, 1),
        (HitAction::LengthDial, 2),
    ];
    let slot_w = dials_region_w / dial_labels.len() as f32;

    let mut regions = Vec::with_capacity(4);

    regions.push(((mode_x, mode_y, mode_w, mode_h), HitAction::ModeSelector));

    for (action, idx) in dial_labels {
        let cx = dials_x_start + slot_w * (idx as f32 + 0.5);
        let rx = cx - dial_hit * 0.5;
        let ry = dial_cy - dial_hit * 0.5;
        regions.push(((rx, ry, dial_hit, dial_hit), action));
    }

    regions
}

// ── Window handler ──────────────────────────────────────────────────────

struct MiffWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<MiffParams>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    /// GUI → audio kernel handoff (used when re-baking in Task 11).
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    /// Curve-only MSEG editor state.
    mseg_state: MsegEditState,
    /// Input spectrum shadow for the response view (Task 12).
    input_spectrum: Arc<Mutex<Vec<f32>>>,
    /// The most recently baked kernel — used to draw the response curve.
    last_kernel: crate::kernel::Kernel,

    /// Whether the Alt modifier is currently held (enables stepped-draw in the MSEG).
    alt_held: bool,
    /// Whether the Shift modifier is currently held (enables fine-drag in the MSEG).
    shift_held: bool,
    /// Timestamp of the last left-click in the MSEG region (for double-click detection).
    mseg_last_click_time: std::time::Instant,
    /// Position of the last left-click in the MSEG region.
    mseg_last_click_pos: (f32, f32),
}

impl MiffWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<MiffParams>,
        kernel_handoff: Arc<crate::kernel::KernelHandoff>,
        input_spectrum: Arc<Mutex<Vec<f32>>>,
        shared_scale: Arc<AtomicCell<f32>>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        // Bake the initial kernel so the response view has something to draw.
        let initial_kernel = {
            params
                .curve
                .lock()
                .map(|c| {
                    let len = params.length.value() as usize;
                    crate::kernel::bake(&c, len)
                })
                .unwrap_or_default()
        };

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            pending_resize,
            params,
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            kernel_handoff,
            mseg_state: MsegEditState::new_curve_only(),
            input_spectrum,
            last_kernel: initial_kernel,
            alt_held: false,
            shift_held: false,
            mseg_last_click_time: std::time::Instant::now(),
            mseg_last_click_pos: (-999.0, -999.0),
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let (mseg_rect, response_rect, strip_rect) = layout(w, h);

        // ── MSEG region (top ~55%) ──────────────────────────────────────────
        // Lock the curve, copy the `Copy` value out, then release before drawing.
        let curve = self.params.curve.lock().map(|c| *c).unwrap_or_default();

        widgets::mseg::draw_mseg(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mseg_rect,
            &curve,
            &self.mseg_state,
            s,
        );

        // ── Response region (middle ~29%) — frequency-response view ────────
        {
            // Read the latest input spectrum from the audio thread (non-blocking).
            let input_mags: Vec<f32> = self
                .input_spectrum
                .try_lock()
                .map(|guard| guard.clone())
                .unwrap_or_default();

            response_view::draw_response(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                response_rect,
                &self.last_kernel.mags,
                &input_mags,
                s,
            );
        }

        // ── Bottom strip (~16%) ────────────────────────────────────────────
        self.draw_strip(strip_rect, s);
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    /// Check if `(x, y)` is a double-click in the MSEG region.
    /// Records the click position/time and returns `true` if the previous click
    /// was within 400 ms and within 8 pixels.
    fn mseg_double_click_check(&mut self, x: f32, y: f32) -> bool {
        let now = std::time::Instant::now();
        let elapsed_ms = now.duration_since(self.mseg_last_click_time).as_millis();
        let (px, py) = self.mseg_last_click_pos;
        let dist_sq = (x - px) * (x - px) + (y - py) * (y - py);
        let is_double = elapsed_ms < 400 && dist_sq < 64.0; // 8px radius
        self.mseg_last_click_time = now;
        self.mseg_last_click_pos = (x, y);
        is_double
    }

    /// Refresh `alt_held` / `shift_held` from a baseview event's modifier set.
    /// Propagates an Alt change into the MSEG editor's stepped-draw flag.
    fn update_modifiers(&mut self, modifiers: &keyboard_types::Modifiers) {
        let new_alt = modifiers.contains(keyboard_types::Modifiers::ALT);
        let new_shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
        if new_alt != self.alt_held {
            self.alt_held = new_alt;
            self.mseg_state.set_stepped_draw(new_alt);
        }
        self.shift_held = new_shift;
    }

    /// Re-bake the kernel from the current curve + Length, publish it to the
    /// audio thread, and cache it in `self.last_kernel` for the response view.
    /// GUI-thread only; called after a curve or Length edit.
    fn rebake(&mut self) {
        if let Ok(curve) = self.params.curve.lock() {
            let len = self.params.length.value() as usize;
            let kernel = crate::kernel::bake(&curve, len);
            self.kernel_handoff.publish(kernel);
            self.last_kernel = kernel;
        }
    }

    /// Commit any active text-edit, applying the value to the appropriate param.
    fn commit_text_edit(&mut self) {
        use nih_plug::prelude::Param;
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match action {
            HitAction::MixDial => {
                let p = &self.params.mix;
                if let Some(norm) = p.string_to_normalized_value(&text) {
                    setter.begin_set_parameter(p);
                    setter.set_parameter_normalized(p, norm);
                    setter.end_set_parameter(p);
                }
            }
            HitAction::GainDial => {
                let p = &self.params.gain;
                if let Some(norm) = p.string_to_normalized_value(&text) {
                    setter.begin_set_parameter(p);
                    setter.set_parameter_normalized(p, norm);
                    setter.end_set_parameter(p);
                }
            }
            HitAction::LengthDial => {
                let p = &self.params.length;
                if let Some(norm) = p.string_to_normalized_value(&text) {
                    setter.begin_set_parameter(p);
                    setter.set_parameter_normalized(p, norm);
                    setter.end_set_parameter(p);
                    self.rebake();
                }
            }
            HitAction::ModeSelector => {
                // Mode selector is not a text-entry control — no-op.
            }
        }
    }

    /// Apply a mode change (0 = Raw, 1 = Phaseless).
    fn set_mode(&self, variant: usize) {
        let target = match variant {
            0 => crate::MiffMode::Raw,
            _ => crate::MiffMode::Phaseless,
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let norm = self.params.mode.preview_normalized(target);
        setter.begin_set_parameter(&self.params.mode);
        setter.set_parameter_normalized(&self.params.mode, norm);
        setter.end_set_parameter(&self.params.mode);
    }

    /// Formatted value for a float param without its unit suffix (for text-edit seed).
    fn dial_value_without_unit(&self, action: HitAction) -> String {
        use nih_plug::prelude::Param;
        match action {
            HitAction::MixDial => {
                let p = &self.params.mix;
                p.normalized_value_to_string(p.modulated_normalized_value(), false)
            }
            HitAction::GainDial => {
                let p = &self.params.gain;
                p.normalized_value_to_string(p.modulated_normalized_value(), false)
            }
            HitAction::LengthDial => {
                let p = &self.params.length;
                p.normalized_value_to_string(p.modulated_normalized_value(), false)
            }
            HitAction::ModeSelector => String::new(),
        }
    }

    /// Draw the bottom control strip: Mode stepped selector + Mix/Gain/Length dials.
    /// Also registers hit regions into `self.drag` for all four controls.
    fn draw_strip(&mut self, strip_rect: (f32, f32, f32, f32), scale: f32) {
        use nih_plug::prelude::Param;

        let regions = strip_regions(strip_rect, scale);

        // Mode selector active index: Raw = 0, Phaseless = 1.
        let mode_idx = if self.params.mode.value() == crate::MiffMode::Raw {
            0usize
        } else {
            1
        };

        // 1px separator rule along the strip's top edge, visually dividing
        // the response region from the control strip.
        let (sx, sy, sw, _sh) = strip_rect;
        widgets::draw_rect(
            &mut self.surface.pixmap,
            sx,
            sy,
            sw,
            1.0,
            widgets::color_border(),
        );

        for ((rx, ry, rw, rh), action) in regions {
            // Register the hit region before drawing the control (matches
            // wavetable-filter's editor ordering).
            self.drag.push_region(rx, ry, rw, rh, action);
            match action {
                HitAction::ModeSelector => {
                    widgets::draw_stepped_selector(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        rx,
                        ry,
                        rw,
                        rh,
                        &["Raw", "Phaseless"],
                        mode_idx,
                    );
                }
                HitAction::MixDial => {
                    let p = &self.params.mix;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::MixDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Mix",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
                HitAction::GainDial => {
                    let p = &self.params.gain;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::GainDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Gain",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
                HitAction::LengthDial => {
                    let p = &self.params.length;
                    let unmod = p.unmodulated_normalized_value();
                    let modulated = p.modulated_normalized_value();
                    let value_text = p.normalized_value_to_string(modulated, true);
                    let editing_buf = self
                        .text_edit
                        .active_for(&HitAction::LengthDial)
                        .map(str::to_owned);
                    let caret = self.text_edit.caret_visible();
                    let cx = rx + rw * 0.5;
                    let cy = ry + rh * 0.5;
                    let radius = DIAL_RADIUS * scale;
                    widgets::draw_dial_ex(
                        &mut self.surface.pixmap,
                        &mut self.text_renderer,
                        cx,
                        cy,
                        radius,
                        "Length",
                        &value_text,
                        unmod,
                        Some(modulated),
                        editing_buf.as_deref(),
                        caret,
                    );
                }
            }
        }
    }
}

impl baseview::WindowHandler for MiffWindow {
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
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;
        let s = self.scale_factor;
        let (mseg_rect, _response_rect, strip_rect) = layout(w, h);

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

            // ── CursorMoved ─────────────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                let x = position.x as f32;
                let y = position.y as f32;
                self.drag.set_mouse(x, y);
                self.update_modifiers(modifiers);

                match event_target(mseg_rect, x, y) {
                    EventTarget::Mseg => {
                        // MSEG drag-move (if a drag is active the MSEG tracks it
                        // even outside the rect, which matches the node-drag
                        // intention; for controls there's no drag into MSEG).
                        let changed = {
                            if let Ok(mut curve) = self.params.curve.lock() {
                                self.mseg_state.on_mouse_move(
                                    x,
                                    y,
                                    &mut curve,
                                    mseg_rect,
                                    s,
                                    self.shift_held,
                                )
                            } else {
                                None
                            }
                        };
                        if changed == Some(MsegEdit::Changed) {
                            self.rebake();
                        }
                    }
                    EventTarget::Controls => {
                        // Dial drag — only if a drag is active.
                        if let Some(action) = self.drag.active_action().copied() {
                            let shift = self.shift_held;
                            let current = match action {
                                HitAction::MixDial => {
                                    self.params.mix.unmodulated_normalized_value()
                                }
                                HitAction::GainDial => {
                                    self.params.gain.unmodulated_normalized_value()
                                }
                                HitAction::LengthDial => {
                                    self.params.length.unmodulated_normalized_value()
                                }
                                HitAction::ModeSelector => 0.0,
                            };
                            if let Some(norm) = self.drag.update_drag(shift, current) {
                                let setter = ParamSetter::new(self.gui_context.as_ref());
                                match action {
                                    HitAction::MixDial => {
                                        setter.set_parameter_normalized(&self.params.mix, norm);
                                    }
                                    HitAction::GainDial => {
                                        setter.set_parameter_normalized(&self.params.gain, norm);
                                    }
                                    HitAction::LengthDial => {
                                        setter.set_parameter_normalized(&self.params.length, norm);
                                        self.rebake();
                                    }
                                    HitAction::ModeSelector => {}
                                }
                            }
                        }
                    }
                }

                // Also allow MSEG drags that started inside the rect to follow
                // the pointer even when it leaves the rect — deliver the move
                // even when the pointer is in the Controls zone if there is an
                // active MSEG drag.  (The branch above covers Mseg; we handle
                // the cross-boundary case by re-delivering for Controls zone
                // when no Controls drag is active but a curve edit might be pending.)
                // NOTE: The move is already dispatched above for Mseg zone.
                // For Controls zone during an *active MSEG drag*, we need to
                // also forward the move. We do this by checking whether a
                // Controls drag is active; if not and Controls is the target,
                // we still pass the move to the MSEG state.
                if event_target(mseg_rect, x, y) == EventTarget::Controls
                    && self.drag.active_action().is_none()
                {
                    // No controls drag active — forward pointer move to MSEG state
                    // so node drags that stray outside the MSEG rect keep tracking.
                    let changed = {
                        if let Ok(mut curve) = self.params.curve.lock() {
                            self.mseg_state.on_mouse_move(
                                x,
                                y,
                                &mut curve,
                                mseg_rect,
                                s,
                                self.shift_held,
                            )
                        } else {
                            None
                        }
                    };
                    if changed == Some(MsegEdit::Changed) {
                        self.rebake();
                    }
                }
            }

            // ── Left button pressed ─────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let (x, y) = self.drag.mouse_pos();
                self.update_modifiers(modifiers);

                match event_target(mseg_rect, x, y) {
                    EventTarget::Mseg => {
                        // Commit any open text-edit before a new click.
                        self.commit_text_edit();

                        // MSEG clicks use a per-window timestamp/position double-click heuristic.
                        let is_double = self.mseg_double_click_check(x, y);
                        if is_double {
                            let changed = {
                                if let Ok(mut curve) = self.params.curve.lock() {
                                    self.mseg_state
                                        .on_double_click(x, y, &mut curve, mseg_rect, s)
                                } else {
                                    None
                                }
                            };
                            if changed == Some(MsegEdit::Changed) {
                                self.rebake();
                            }
                        } else {
                            let changed = {
                                if let Ok(mut curve) = self.params.curve.lock() {
                                    self.mseg_state.on_mouse_down(
                                        x,
                                        y,
                                        &mut curve,
                                        mseg_rect,
                                        s,
                                        self.shift_held,
                                    )
                                } else {
                                    None
                                }
                            };
                            if changed == Some(MsegEdit::Changed) {
                                self.rebake();
                            }
                        }
                    }
                    EventTarget::Controls => {
                        // Commit any open text-edit first.
                        self.commit_text_edit();

                        // End any previous drag.
                        if let Some(ended) = self.drag.end_drag() {
                            let setter = ParamSetter::new(self.gui_context.as_ref());
                            match ended {
                                HitAction::MixDial => {
                                    setter.end_set_parameter(&self.params.mix);
                                }
                                HitAction::GainDial => {
                                    setter.end_set_parameter(&self.params.gain);
                                }
                                HitAction::LengthDial => {
                                    setter.end_set_parameter(&self.params.length);
                                }
                                HitAction::ModeSelector => {}
                            }
                        }

                        if let Some(region) = self.drag.hit_test().cloned() {
                            let is_double = self.drag.check_double_click(&region.action);
                            match region.action {
                                HitAction::MixDial => {
                                    let setter = ParamSetter::new(self.gui_context.as_ref());
                                    if is_double {
                                        // Double-click resets to default.
                                        use nih_plug::prelude::Param;
                                        let p = &self.params.mix;
                                        setter.begin_set_parameter(p);
                                        setter.set_parameter_normalized(
                                            p,
                                            p.default_normalized_value(),
                                        );
                                        setter.end_set_parameter(p);
                                    } else {
                                        use nih_plug::prelude::Param;
                                        let norm = self.params.mix.unmodulated_normalized_value();
                                        self.drag.begin_drag(
                                            HitAction::MixDial,
                                            norm,
                                            self.shift_held,
                                        );
                                        setter.begin_set_parameter(&self.params.mix);
                                    }
                                }
                                HitAction::GainDial => {
                                    let setter = ParamSetter::new(self.gui_context.as_ref());
                                    if is_double {
                                        use nih_plug::prelude::Param;
                                        let p = &self.params.gain;
                                        setter.begin_set_parameter(p);
                                        setter.set_parameter_normalized(
                                            p,
                                            p.default_normalized_value(),
                                        );
                                        setter.end_set_parameter(p);
                                    } else {
                                        use nih_plug::prelude::Param;
                                        let norm = self.params.gain.unmodulated_normalized_value();
                                        self.drag.begin_drag(
                                            HitAction::GainDial,
                                            norm,
                                            self.shift_held,
                                        );
                                        setter.begin_set_parameter(&self.params.gain);
                                    }
                                }
                                HitAction::LengthDial => {
                                    let setter = ParamSetter::new(self.gui_context.as_ref());
                                    if is_double {
                                        use nih_plug::prelude::Param;
                                        let p = &self.params.length;
                                        setter.begin_set_parameter(p);
                                        setter.set_parameter_normalized(
                                            p,
                                            p.default_normalized_value(),
                                        );
                                        setter.end_set_parameter(p);
                                        self.rebake();
                                    } else {
                                        use nih_plug::prelude::Param;
                                        let norm =
                                            self.params.length.unmodulated_normalized_value();
                                        self.drag.begin_drag(
                                            HitAction::LengthDial,
                                            norm,
                                            self.shift_held,
                                        );
                                        setter.begin_set_parameter(&self.params.length);
                                    }
                                }
                                HitAction::ModeSelector => {
                                    // Determine which segment was hit.
                                    let strip_regions = strip_regions(strip_rect, s);
                                    // The ModeSelector region covers the whole selector.
                                    // Find which half was clicked by x position.
                                    if let Some(((rx, _, rw, _), _)) = strip_regions
                                        .iter()
                                        .find(|(_, a)| *a == HitAction::ModeSelector)
                                    {
                                        let local = x - rx;
                                        let variant = if local < rw * 0.5 { 0 } else { 1 };
                                        self.set_mode(variant);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // ── Left button released ────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                // A release ALWAYS terminates any in-flight drag, regardless of
                // where the cursor is — a controls drag started on a dial can be
                // released over the MSEG region (~55% of the window), and the
                // automation gesture must still be closed out.

                // End any MSEG drag.
                let changed = {
                    if let Ok(mut curve) = self.params.curve.lock() {
                        self.mseg_state.on_mouse_up(&mut curve)
                    } else {
                        None
                    }
                };
                if changed == Some(MsegEdit::Changed) {
                    self.rebake();
                }

                // End any controls drag and close the automation gesture.
                if let Some(ended) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    match ended {
                        HitAction::MixDial => {
                            setter.end_set_parameter(&self.params.mix);
                        }
                        HitAction::GainDial => {
                            setter.end_set_parameter(&self.params.gain);
                        }
                        HitAction::LengthDial => {
                            setter.end_set_parameter(&self.params.length);
                        }
                        HitAction::ModeSelector => {}
                    }
                }
            }

            // ── Right button pressed ────────────────────────────────────────
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                let (x, y) = self.drag.mouse_pos();
                // Right-click during a controls drag: ignore (as wavetable-filter does).
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }

                match event_target(mseg_rect, x, y) {
                    EventTarget::Mseg => {
                        // Right-click in MSEG: toggle segment stepped flag.
                        let changed = {
                            if let Ok(mut curve) = self.params.curve.lock() {
                                self.mseg_state
                                    .on_right_click(x, y, &mut curve, mseg_rect, s)
                            } else {
                                None
                            }
                        };
                        if changed == Some(MsegEdit::Changed) {
                            self.rebake();
                        }
                    }
                    EventTarget::Controls => {
                        // Right-click on a dial opens text entry.
                        self.commit_text_edit();
                        if let Some(region) = self.drag.hit_test().cloned() {
                            match region.action {
                                HitAction::MixDial
                                | HitAction::GainDial
                                | HitAction::LengthDial => {
                                    let initial = self.dial_value_without_unit(region.action);
                                    self.text_edit.begin(region.action, &initial);
                                }
                                HitAction::ModeSelector => {
                                    // No text entry on the mode selector.
                                }
                            }
                        }
                    }
                }
            }

            // ── Keyboard ────────────────────────────────────────────────────
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                // Key-up events are swallowed while editing so host shortcuts
                // don't fire on release.
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
                    keyboard_types::Key::Enter => {
                        self.commit_text_edit();
                    }
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

pub(crate) struct MiffEditor {
    params: Arc<MiffParams>,
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    input_spectrum: Arc<Mutex<Vec<f32>>>,
    scaling_factor: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,
}

pub(crate) fn create(
    params: Arc<MiffParams>,
    kernel_handoff: Arc<crate::kernel::KernelHandoff>,
    input_spectrum: Arc<Mutex<Vec<f32>>>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MiffEditor {
        params,
        kernel_handoff,
        input_spectrum,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for MiffEditor {
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
        let kernel_handoff = Arc::clone(&self.kernel_handoff);
        let input_spectrum = Arc::clone(&self.input_spectrum);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("miff"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                MiffWindow::new(
                    window,
                    gui_context,
                    params,
                    kernel_handoff,
                    input_spectrum,
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

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_target_routes_by_region() {
        let (mseg, _response, strip) = layout(880.0, 620.0);
        // A point inside the MSEG region routes to Mseg.
        let mx = mseg.0 + mseg.2 * 0.5;
        let my = mseg.1 + mseg.3 * 0.5;
        assert_eq!(event_target(mseg, mx, my), EventTarget::Mseg);
        // A point in the strip routes to Controls.
        let sx = strip.0 + strip.2 * 0.5;
        let sy = strip.1 + strip.3 * 0.5;
        assert_eq!(event_target(mseg, sx, sy), EventTarget::Controls);
    }

    #[test]
    fn layout_regions_stack_without_overlap() {
        let (mseg, response, strip) = layout(880.0, 620.0);
        // mseg above response above strip, no overlap.
        // mseg bottom <= response top (within float tolerance)
        assert!(
            mseg.1 + mseg.3 <= response.1 + 0.01,
            "mseg bottom ({}) > response top ({})",
            mseg.1 + mseg.3,
            response.1
        );
        // response bottom <= strip top
        assert!(
            response.1 + response.3 <= strip.1 + 0.01,
            "response bottom ({}) > strip top ({})",
            response.1 + response.3,
            strip.1
        );
    }

    #[test]
    fn layout_regions_cover_full_height() {
        let h = 620.0_f32;
        let (mseg, response, strip) = layout(880.0, h);
        let total = mseg.3 + response.3 + strip.3;
        assert!(
            (total - h).abs() < 0.1,
            "regions don't sum to window height: {total} vs {h}"
        );
    }

    #[test]
    fn layout_regions_share_full_width() {
        let w = 880.0_f32;
        let (mseg, response, strip) = layout(w, 620.0);
        assert!((mseg.2 - w).abs() < 0.01);
        assert!((response.2 - w).abs() < 0.01);
        assert!((strip.2 - w).abs() < 0.01);
    }

    #[test]
    fn strip_regions_are_within_strip_and_disjoint() {
        let w = 880.0_f32;
        let h = 620.0_f32;
        let (_, _, strip_rect) = layout(w, h);
        let (sx, sy, sw, sh) = strip_rect;

        let regions = strip_regions(strip_rect, 1.0);
        assert_eq!(regions.len(), 4, "expected exactly 4 control regions");

        // All rects must lie inside the strip.
        for ((rx, ry, rw, rh), action) in &regions {
            assert!(
                rx >= &sx && ry >= &sy && rx + rw <= sx + sw + 0.5 && ry + rh <= sy + sh + 0.5,
                "{action:?} rect ({rx},{ry},{rw},{rh}) falls outside strip ({sx},{sy},{sw},{sh})"
            );
        }

        // All rects must be pairwise non-overlapping.
        for i in 0..regions.len() {
            for j in (i + 1)..regions.len() {
                let ((ax, ay, aw, ah), a_action) = regions[i];
                let ((bx, by, bw, bh), b_action) = regions[j];
                let overlap = ax < bx + bw && ax + aw > bx && ay < by + bh && ay + ah > by;
                assert!(!overlap, "{a_action:?} and {b_action:?} rects overlap");
            }
        }
    }

    #[test]
    fn draw_smoke_headless() {
        // Draw into a Pixmap directly to verify the draw path doesn't panic and
        // leaves a non-zero-alpha pixel inside the MSEG region.
        use tiny_skia::Pixmap;
        use tiny_skia_widgets::mseg::{draw_mseg, MsegEditState};
        use tiny_skia_widgets::{color_bg, TextRenderer};

        // Use the embedded font (same as MiffWindow::new).
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let mut text_renderer = TextRenderer::new(font_data);

        let w = 880_u32;
        let h = 620_u32;
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(color_bg());

        let (mseg_rect, _response_rect, _strip_rect) = layout(w as f32, h as f32);

        let curve = crate::kernel::default_flat_curve();
        let state = MsegEditState::new_curve_only();

        draw_mseg(&mut pm, &mut text_renderer, mseg_rect, &curve, &state, 1.0);

        // Sample a pixel well inside the MSEG canvas (quarter-width, half-height).
        let sample_x = (mseg_rect.0 + mseg_rect.2 * 0.25) as u32;
        let sample_y = (mseg_rect.1 + mseg_rect.3 * 0.5) as u32;
        let alpha = pm.pixels()[(sample_y * w + sample_x) as usize].alpha();
        assert!(
            alpha > 0,
            "MSEG region not painted at ({sample_x}, {sample_y})"
        );
    }
}
