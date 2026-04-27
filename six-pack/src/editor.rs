//! Softbuffer-based editor for Six Pack. CPU rendering via tiny-skia.
//!
//! Layout:
//! ```text
//! +-----------------------------------------------------------+
//! | filter icons (6 across)                                   |
//! +-----------------------------------------------------------+
//! | EQ curve view + spectrum overlay + 6 draggable band dots  |
//! +-----------------------------------------------------------+
//! | per-band labels grid (Freq / Gain / Q / Algo / Mode)      |
//! +-----------------------------------------------------------+
//! | bottom strip: Input / [link] / Output / Mix / steppers    |
//! +-----------------------------------------------------------+
//! ```
//!
//! Coordinates are in physical pixels; `scale_factor` is derived from
//! `physical_width / WINDOW_WIDTH`.

mod band_labels;
mod bottom_strip;
mod curve_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crate::spectrum::N_BINS;
use crate::SixPackParams;
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 720;
const WINDOW_HEIGHT: u32 = 500;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit testing ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum HitAction {
    /// One of the global float dials (Input / Output / Mix).
    GlobalDial(GlobalDialId),
    /// Band dot in the curve view: drag = freq+gain, scroll = Q,
    /// double-click = toggle enable.
    BandDot(usize),
    /// Per-band label cell in the labels grid (right-click → text edit
    /// for Freq/Gain/Q; left-click cycles Algo/Mode).
    BandLabel(usize, BandLabelField),
    /// Filter icon at the top — toggle band enable.
    BandIcon(usize),
    /// Toggle the I/O link.
    IoLink,
    /// Stepped selector segments in the bottom strip.
    QualitySeg(usize),
    DriveSeg(usize),
    /// Toggle de-emphasis.
    DeEmphasis,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum GlobalDialId {
    Input,
    Output,
    Mix,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum BandLabelField {
    Freq,
    Gain,
    Q,
    Algo,
    Mode,
}

// ── Per-band palette ────────────────────────────────────────────────────

/// Six band colors — magenta-to-cyan gradient on a deep navy background.
pub(crate) const BAND_COLORS: [(u8, u8, u8); 6] = [
    (0xff, 0x4a, 0xa8), // band 0 — magenta
    (0xff, 0x7a, 0x6e), // band 1 — coral
    (0xff, 0xc1, 0x4a), // band 2 — amber
    (0x9f, 0xe2, 0x4a), // band 3 — lime
    (0x4a, 0xd5, 0xc8), // band 4 — teal
    (0x6a, 0x9c, 0xff), // band 5 — periwinkle
];

#[inline]
pub(crate) fn band_color(idx: usize) -> tiny_skia::Color {
    let (r, g, b) = BAND_COLORS[idx.min(5)];
    tiny_skia::Color::from_rgba8(r, g, b, 0xff)
}

#[inline]
pub(crate) fn band_color_alpha(idx: usize, a: u8) -> tiny_skia::Color {
    let (r, g, b) = BAND_COLORS[idx.min(5)];
    tiny_skia::Color::from_rgba8(r, g, b, a)
}

// ── Window handler ──────────────────────────────────────────────────────

struct SixPackWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    pending_resize: Arc<AtomicU64>,

    params: Arc<SixPackParams>,
    /// Cloned from `SixPack::spectrum.bins`. Read each frame for the overlay.
    spectrum_bins: Arc<[AtomicU32; N_BINS]>,
    /// EMA-smoothed display magnitudes, GUI-thread-only.
    display_bins: [f32; N_BINS],

    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    /// Layout rectangles cached from last `draw()` for hit-testing band dots
    /// (which need to convert pixel coords to (freq, gain) parameter space).
    curve_rect: (f32, f32, f32, f32),
}

impl SixPackWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<SixPackParams>,
        spectrum_bins: Arc<[AtomicU32; N_BINS]>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            pending_resize,
            params,
            spectrum_bins,
            display_bins: [0.0; N_BINS],
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            curve_rect: (0.0, 0.0, 0.0, 0.0),
        }
    }

    // ── Spectrum read + smoothing ───────────────────────────────────────

    fn update_spectrum_smoothing(&mut self) {
        // Simple per-bin EMA smoothing for the display overlay.
        const ATTACK: f32 = 0.6;
        const RELEASE: f32 = 0.15;
        for (i, slot) in self.spectrum_bins.iter().enumerate() {
            let raw = f32::from_bits(slot.load(Ordering::Relaxed));
            let prev = self.display_bins[i];
            let alpha = if raw > prev { ATTACK } else { RELEASE };
            self.display_bins[i] = prev + alpha * (raw - prev);
        }
    }

    // ── Param helpers ───────────────────────────────────────────────────

    fn float_for_global(&self, id: GlobalDialId) -> &FloatParam {
        match id {
            GlobalDialId::Input => &self.params.input_gain,
            GlobalDialId::Output => &self.params.output_gain,
            GlobalDialId::Mix => &self.params.mix,
        }
    }

    fn float_for_band(&self, band: usize, field: BandLabelField) -> Option<&FloatParam> {
        let bp = &self.params.bands[band];
        match field {
            BandLabelField::Freq => Some(&bp.freq),
            BandLabelField::Gain => Some(&bp.gain),
            BandLabelField::Q => Some(&bp.q),
            _ => None,
        }
    }

    fn formatted_value_without_unit_global(&self, id: GlobalDialId) -> String {
        let p = self.float_for_global(id);
        let v = p.modulated_normalized_value();
        p.normalized_value_to_string(v, false)
    }

    fn formatted_value_without_unit_band(&self, band: usize, field: BandLabelField) -> String {
        let Some(p) = self.float_for_band(band, field) else {
            return String::new();
        };
        let v = p.modulated_normalized_value();
        p.normalized_value_to_string(v, false)
    }

    fn commit_text_edit(&mut self) {
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        match action {
            HitAction::GlobalDial(id) => {
                let p = self.float_for_global(id);
                if let Some(norm) = p.string_to_normalized_value(&text) {
                    setter.begin_set_parameter(p);
                    setter.set_parameter_normalized(p, norm);
                    setter.end_set_parameter(p);
                }
            }
            HitAction::BandLabel(band, field) => {
                if let Some(p) = self.float_for_band(band, field) {
                    if let Some(norm) = p.string_to_normalized_value(&text) {
                        setter.begin_set_parameter(p);
                        setter.set_parameter_normalized(p, norm);
                        setter.end_set_parameter(p);
                    }
                }
            }
            _ => {}
        }
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let s = self.scale_factor;

        self.drag.clear_regions();
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg());

        let pad = 12.0 * s;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        // Vertical band heights — proportional layout so all sections rescale
        // together when the window grows.
        let icons_h = 36.0 * s;
        let bottom_h = 92.0 * s;
        let labels_h = 110.0 * s;
        let curve_y0 = pad + icons_h + 4.0 * s;
        let curve_h = (h - bottom_h - labels_h - icons_h - pad - 4.0 * s).max(80.0 * s);
        let labels_y0 = curve_y0 + curve_h + 4.0 * s;
        let bottom_y0 = labels_y0 + labels_h + 4.0 * s;

        // Pre-update display bins (lock-free atomic reads + EMA).
        self.update_spectrum_smoothing();

        // ── Filter icons row ────────────────────────────────────────────
        self.draw_filter_icons(pad, pad, w - 2.0 * pad, icons_h);

        // ── Curve view ──────────────────────────────────────────────────
        let curve_x = pad;
        let curve_w = w - 2.0 * pad;
        self.curve_rect = (curve_x, curve_y0, curve_w, curve_h);
        curve_view::draw(self, curve_x, curve_y0, curve_w, curve_h);

        // ── Per-band label grid ─────────────────────────────────────────
        band_labels::draw(self, pad, labels_y0, w - 2.0 * pad, labels_h);

        // ── Bottom strip ────────────────────────────────────────────────
        bottom_strip::draw(self, pad, bottom_y0, w - 2.0 * pad, bottom_h - pad);
    }

    fn draw_filter_icons(&mut self, x: f32, y: f32, w: f32, h: f32) {
        // 6 equal columns. Each shows a stylized filter glyph plus the band
        // index, and a click toggles the band enable.
        let col_w = w / 6.0;
        for i in 0..6 {
            let cx = x + (i as f32 + 0.5) * col_w;
            let cy = y + h * 0.5;
            let enabled = self.params.bands[i].enable.value();
            let color = if enabled {
                band_color(i)
            } else {
                widgets::color_muted()
            };
            // Draw a stylized "filter shape" hint: low-shelf, peak, peak,
            // peak, peak, high-shelf.
            let icon_w = (col_w * 0.7).min(40.0 * self.scale_factor);
            let icon_h = h * 0.55;
            let ix = cx - icon_w * 0.5;
            let iy = cy - icon_h * 0.5;
            self.draw_filter_glyph(i, ix, iy, icon_w, icon_h, color);
            // Hit region across the column.
            self.drag
                .push_region(x + i as f32 * col_w, y, col_w, h, HitAction::BandIcon(i));
        }
    }

    fn draw_filter_glyph(
        &mut self,
        idx: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: tiny_skia::Color,
    ) {
        // Plot a synthetic magnitude curve for the shape: low-shelf rises
        // toward the left, high-shelf rises toward the right, peaks bump
        // around the center but at different positions.
        let n = 40usize;
        let points: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32; // 0..1 left-to-right
                let mag = match idx {
                    0 => 1.0 - smoothstep(0.0, 0.5, t), // low-shelf
                    5 => smoothstep(0.5, 1.0, t),       // high-shelf
                    _ => peak_curve(t, 0.1 + idx as f32 * 0.16),
                };
                let xx = x + t * w;
                let yy = y + h - mag * h * 0.85 - h * 0.075;
                (xx, yy)
            })
            .collect();
        // Draw as a polyline of short rects — keeps us off the AA path
        // pipeline for these tiny icons.
        for pair in points.windows(2) {
            let (x0, y0) = pair[0];
            let (x1, y1) = pair[1];
            // approximate line segment as a filled rect connecting the two
            // pixels — for tiny icons this is sufficient.
            let dx = (x1 - x0).abs().max(1.0);
            let dy = (y1 - y0).abs().max(1.0);
            // Short hop — just plot a couple of pixels.
            let steps = (dx.max(dy)).ceil() as i32;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                let xx = x0 + (x1 - x0) * t;
                let yy = y0 + (y1 - y0) * t;
                widgets::draw_rect(&mut self.surface.pixmap, xx, yy, 1.5, 1.5, color);
            }
        }
    }

    // ── Resize ──────────────────────────────────────────────────────────

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn peak_curve(t: f32, center: f32) -> f32 {
    let d = (t - center).abs() * 6.0;
    (1.0 - d).clamp(0.0, 1.0)
}

impl baseview::WindowHandler for SixPackWindow {
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
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if let Some(active) = self.drag.active_action().copied() {
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    self.handle_drag(active, shift);
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
                modifiers,
            }) => {
                self.commit_text_edit();
                if let Some(region) = self.drag.hit_test().cloned() {
                    if let Some(active) = self.drag.end_drag() {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.end_drag_for_action(&setter, active);
                    }

                    let is_double = self.drag.check_double_click(&region.action);
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    self.handle_left_press(region.action, is_double, shift);
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
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }
                if let Some(region) = self.drag.hit_test().cloned() {
                    self.commit_text_edit();
                    self.handle_right_press(region.action);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::WheelScrolled { delta, .. }) => {
                if let Some(region) = self.drag.hit_test().cloned() {
                    let dy = match delta {
                        baseview::ScrollDelta::Lines { y, .. } => *y,
                        baseview::ScrollDelta::Pixels { y, .. } => *y * 0.05,
                    };
                    self.handle_scroll(region.action, dy);
                }
            }
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
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

impl SixPackWindow {
    fn handle_drag(&mut self, action: HitAction, shift: bool) {
        match action {
            HitAction::GlobalDial(id) => {
                let current = self.float_for_global(id).unmodulated_normalized_value();
                if let Some(norm) = self.drag.update_drag(shift, current) {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    setter.set_parameter_normalized(self.float_for_global(id), norm);
                }
            }
            HitAction::BandDot(idx) => {
                // Two-axis drag: x = freq (log), y = gain (dB linear).
                let (cx, cy, cw, ch) = self.curve_rect;
                let (mx, my) = self.drag.mouse_pos();
                let mx_clamped = mx.clamp(cx + 4.0, cx + cw - 4.0);
                let my_clamped = my.clamp(cy + 4.0, cy + ch - 4.0);
                let xnorm = ((mx_clamped - cx) / cw).clamp(0.0, 1.0);
                let freq = curve_view::norm_x_to_freq(xnorm);
                let ynorm = 1.0 - ((my_clamped - cy) / ch).clamp(0.0, 1.0);
                let gain_db = ynorm * 18.0; // params: 0..18
                let setter = ParamSetter::new(self.gui_context.as_ref());
                let bp = &self.params.bands[idx];
                setter.set_parameter(&bp.freq, freq);
                setter.set_parameter(&bp.gain, gain_db);
            }
            HitAction::BandLabel(idx, field) => {
                let current = match self.float_for_band(idx, field) {
                    Some(p) => p.unmodulated_normalized_value(),
                    None => return,
                };
                if let Some(norm) = self.drag.update_drag(shift, current) {
                    if let Some(p) = self.float_for_band(idx, field) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        setter.set_parameter_normalized(p, norm);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_left_press(&mut self, action: HitAction, is_double: bool, shift: bool) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let setter = &setter;
        match action {
            HitAction::GlobalDial(id) => {
                if is_double {
                    let p = self.float_for_global(id);
                    setter.begin_set_parameter(p);
                    setter.set_parameter_normalized(p, p.default_normalized_value());
                    setter.end_set_parameter(p);
                } else {
                    let norm = self.float_for_global(id).unmodulated_normalized_value();
                    self.drag.begin_drag(HitAction::GlobalDial(id), norm, shift);
                    setter.begin_set_parameter(self.float_for_global(id));
                }
            }
            HitAction::BandDot(idx) => {
                if is_double {
                    // toggle enable
                    let bp = &self.params.bands[idx];
                    let cur = bp.enable.value();
                    setter.begin_set_parameter(&bp.enable);
                    setter.set_parameter(&bp.enable, !cur);
                    setter.end_set_parameter(&bp.enable);
                } else {
                    self.drag.begin_drag(HitAction::BandDot(idx), 0.0, shift);
                    // Begin parameter edit for both freq and gain.
                    setter.begin_set_parameter(&self.params.bands[idx].freq);
                    setter.begin_set_parameter(&self.params.bands[idx].gain);
                }
            }
            HitAction::BandLabel(idx, field) => match field {
                BandLabelField::Freq | BandLabelField::Gain | BandLabelField::Q => {
                    if is_double {
                        if let Some(p) = self.float_for_band(idx, field) {
                            setter.begin_set_parameter(p);
                            setter.set_parameter_normalized(p, p.default_normalized_value());
                            setter.end_set_parameter(p);
                        }
                    } else {
                        let norm = match self.float_for_band(idx, field) {
                            Some(p) => p.unmodulated_normalized_value(),
                            None => return,
                        };
                        self.drag
                            .begin_drag(HitAction::BandLabel(idx, field), norm, shift);
                        if let Some(p) = self.float_for_band(idx, field) {
                            setter.begin_set_parameter(p);
                        }
                    }
                }
                BandLabelField::Algo => {
                    let bp = &self.params.bands[idx];
                    let cur = bp.algo.value();
                    let cur_idx = cur as usize;
                    let next_idx = (cur_idx + 1) % 6;
                    setter.begin_set_parameter(&bp.algo);
                    setter.set_parameter_normalized(&bp.algo, next_idx as f32 / 5.0);
                    setter.end_set_parameter(&bp.algo);
                }
                BandLabelField::Mode => {
                    let bp = &self.params.bands[idx];
                    let cur = bp.channel.value();
                    let cur_idx = cur as usize;
                    let next_idx = (cur_idx + 1) % 3;
                    setter.begin_set_parameter(&bp.channel);
                    setter.set_parameter_normalized(&bp.channel, next_idx as f32 / 2.0);
                    setter.end_set_parameter(&bp.channel);
                }
            },
            HitAction::BandIcon(idx) => {
                let bp = &self.params.bands[idx];
                let cur = bp.enable.value();
                setter.begin_set_parameter(&bp.enable);
                setter.set_parameter(&bp.enable, !cur);
                setter.end_set_parameter(&bp.enable);
            }
            HitAction::IoLink => {
                let cur = self.params.io_link.value();
                setter.begin_set_parameter(&self.params.io_link);
                setter.set_parameter(&self.params.io_link, !cur);
                setter.end_set_parameter(&self.params.io_link);
            }
            HitAction::QualitySeg(seg) => {
                let p = &self.params.quality;
                setter.begin_set_parameter(p);
                setter.set_parameter_normalized(p, seg as f32 / 3.0);
                setter.end_set_parameter(p);
            }
            HitAction::DriveSeg(seg) => {
                let p = &self.params.drive;
                setter.begin_set_parameter(p);
                setter.set_parameter_normalized(p, seg as f32 / 2.0);
                setter.end_set_parameter(p);
            }
            HitAction::DeEmphasis => {
                let cur = self.params.deemphasis.value();
                setter.begin_set_parameter(&self.params.deemphasis);
                setter.set_parameter(&self.params.deemphasis, !cur);
                setter.end_set_parameter(&self.params.deemphasis);
            }
        }
    }

    fn end_drag_for_action(&self, setter: &ParamSetter, action: HitAction) {
        match action {
            HitAction::GlobalDial(id) => {
                setter.end_set_parameter(self.float_for_global(id));
            }
            HitAction::BandDot(idx) => {
                setter.end_set_parameter(&self.params.bands[idx].freq);
                setter.end_set_parameter(&self.params.bands[idx].gain);
            }
            HitAction::BandLabel(idx, field) => {
                if let Some(p) = self.float_for_band(idx, field) {
                    setter.end_set_parameter(p);
                }
            }
            _ => {}
        }
    }

    fn handle_right_press(&mut self, action: HitAction) {
        match action {
            HitAction::GlobalDial(id) => {
                let initial = self.formatted_value_without_unit_global(id);
                self.text_edit.begin(HitAction::GlobalDial(id), &initial);
            }
            HitAction::BandLabel(idx, field) => match field {
                BandLabelField::Freq | BandLabelField::Gain | BandLabelField::Q => {
                    let initial = self.formatted_value_without_unit_band(idx, field);
                    self.text_edit
                        .begin(HitAction::BandLabel(idx, field), &initial);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn handle_scroll(&self, action: HitAction, dy: f32) {
        if let HitAction::BandDot(idx) = action {
            // Scroll wheel adjusts Q on band dot.
            let bp = &self.params.bands[idx];
            let cur = bp.q.unmodulated_normalized_value();
            let step = 0.03 * dy;
            let new = (cur + step).clamp(0.0, 1.0);
            let setter = ParamSetter::new(self.gui_context.as_ref());
            setter.begin_set_parameter(&bp.q);
            setter.set_parameter_normalized(&bp.q, new);
            setter.end_set_parameter(&bp.q);
        }
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

pub(crate) struct SixPackEditor {
    params: Arc<SixPackParams>,
    spectrum_bins: Arc<[AtomicU32; N_BINS]>,
    pending_resize: Arc<AtomicU64>,
}

pub(crate) fn create(
    params: Arc<SixPackParams>,
    spectrum_bins: Arc<[AtomicU32; N_BINS]>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(SixPackEditor {
        params,
        spectrum_bins,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for SixPackEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let spectrum_bins = Arc::clone(&self.spectrum_bins);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Six Pack"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                SixPackWindow::new(
                    window,
                    gui_context,
                    params,
                    spectrum_bins,
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
    fn band_color_index_is_safe() {
        for i in 0..6 {
            let c = band_color(i);
            assert!(c.alpha() >= 0.99);
        }
        // Out-of-range indices clamp instead of panicking.
        let _ = band_color(99);
    }

    #[test]
    fn smoothstep_endpoints() {
        assert_eq!(super::smoothstep(0.0, 1.0, 0.0), 0.0);
        assert_eq!(super::smoothstep(0.0, 1.0, 1.0), 1.0);
    }

    #[test]
    fn text_edit_state_inactive_by_default() {
        let te: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(te
            .active_for(&HitAction::GlobalDial(GlobalDialId::Input))
            .is_none());
        assert!(te
            .active_for(&HitAction::BandLabel(0, BandLabelField::Freq))
            .is_none());
    }
}
