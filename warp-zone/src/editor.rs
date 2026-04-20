//! Softbuffer-based editor for Warp Zone. CPU rendering via tiny-skia.
//!
//! Layout (600x400, freely resizable):
//! - Top strip (~60px): Title, bypass toggle, Shift/Stretch/Mix dials
//! - Main area: Scrolling spectral waterfall

use crate::{SpectralDisplay, WarpZoneParams, DISPLAY_BINS, DISPLAY_COLUMNS};
use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 600;
const WINDOW_HEIGHT: u32 = 400;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ParamId {
    Shift,
    Stretch,
    Mix,
    Feedback,
    LowFreq,
    HighFreq,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ButtonAction {
    Freeze,
}

// ── Waterfall color LUT ────────────────────────────────────────────────

const COLOR_LUT_SIZE: usize = 256;

/// Pre-computed color lookup table: quantized magnitude → premultiplied RGBA bytes [R,G,B,A].
/// Built once at init, eliminates log10/branching from the per-pixel hot path.
fn build_color_lut() -> [[u8; 4]; COLOR_LUT_SIZE] {
    let mut lut = [[0u8; 4]; COLOR_LUT_SIZE];

    fn lerp(a: u8, b: u8, t: f32) -> u8 {
        (a as f32 + (b as f32 - a as f32) * t).round() as u8
    }

    for (i, entry) in lut.iter_mut().enumerate() {
        let t = i as f32 / (COLOR_LUT_SIZE - 1) as f32;

        let (r, g, b) = if t < 0.15 {
            let s = t / 0.15;
            (lerp(0, 48, s), 0, lerp(0, 80, s))
        } else if t < 0.3 {
            let s = (t - 0.15) / 0.15;
            (lerp(48, 75, s), 0, lerp(80, 160, s))
        } else if t < 0.5 {
            let s = (t - 0.3) / 0.2;
            (lerp(75, 0, s), lerp(0, 220, s), lerp(160, 255, s))
        } else if t < 0.7 {
            let s = (t - 0.5) / 0.2;
            (lerp(0, 255, s), lerp(220, 0, s), 255)
        } else if t < 0.85 {
            let s = (t - 0.7) / 0.15;
            (255, lerp(0, 105, s), lerp(255, 180, s))
        } else {
            let s = (t - 0.85) / 0.15;
            (255, lerp(105, 255, s), lerp(180, 255, s))
        };
        *entry = [r, g, b, 255];
    }
    lut
}

/// Convert a magnitude to a LUT index (0..255) using dB scaling.
#[inline]
fn magnitude_to_lut_index(mag: f32) -> usize {
    let db = (20.0 * mag.max(1e-10).log10()).max(-80.0);
    let t = (db + 80.0) / 80.0; // 0..1
    let idx = (t * (COLOR_LUT_SIZE - 1) as f32) as usize;
    idx.min(COLOR_LUT_SIZE - 1)
}

// ── Window handler ──────────────────────────────────────────────────────

struct WarpZoneWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<WarpZoneParams>,
    display: Arc<SpectralDisplay>,
    text_renderer: widgets::TextRenderer,
    color_lut: [[u8; 4]; COLOR_LUT_SIZE],
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,
}

impl WarpZoneWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<WarpZoneParams>,
        display: Arc<SpectralDisplay>,
        shared_scale: Arc<AtomicCell<f32>>,
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
            shared_scale,
            pending_resize,
            params,
            display,
            text_renderer,
            color_lut: build_color_lut(),
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
        }
    }

    // ── Param access helpers ────────────────────────────────────────────

    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Shift => &self.params.shift,
            ParamId::Stretch => &self.params.stretch,
            ParamId::Mix => &self.params.mix,
            ParamId::Feedback => &self.params.feedback,
            ParamId::LowFreq => &self.params.low_freq,
            ParamId::HighFreq => &self.params.high_freq,
        }
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.begin_set_parameter(self.float_param(id));
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        setter.set_parameter_normalized(self.float_param(id), normalized);
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.end_set_parameter(self.float_param(id));
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, p.default_normalized_value());
        setter.end_set_parameter(p);
    }

    fn formatted_value_without_unit(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        let v = p.modulated_normalized_value();
        p.normalized_value_to_string(v, false)
    }

    fn commit_text_edit(&mut self) {
        use nih_plug::prelude::Param;
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let HitAction::Dial(param_id) = action else {
            return;
        };
        let p = self.float_param(param_id);
        let norm = p.string_to_normalized_value(&text);
        let Some(norm) = norm else { return };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set_param(&setter, param_id);
        self.set_param_normalized(&setter, param_id, norm);
        self.end_set_param(&setter, param_id);
    }

    // ── Drawing ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let s = self.scale_factor;

        self.drag.clear_regions();

        let pad = 16.0 * s;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        let mut y = 10.0 * s;

        // Clear the full pixmap each frame.
        self.surface.pixmap.fill(widgets::color_bg());

        // Timbre controls grouped left, Mix isolated right
        let timbre_dials: [(ParamId, &str, f32, f32, String); 3] = [
            (
                ParamId::Shift,
                "Shift",
                self.params.shift.unmodulated_normalized_value(),
                self.params.shift.modulated_normalized_value(),
                self.format_value(ParamId::Shift),
            ),
            (
                ParamId::Stretch,
                "Stretch",
                self.params.stretch.unmodulated_normalized_value(),
                self.params.stretch.modulated_normalized_value(),
                self.format_value(ParamId::Stretch),
            ),
            (
                ParamId::Feedback,
                "Feedback",
                self.params.feedback.unmodulated_normalized_value(),
                self.params.feedback.modulated_normalized_value(),
                self.format_value(ParamId::Feedback),
            ),
        ];
        let mix_dial = (
            ParamId::Mix,
            "Mix",
            self.params.mix.unmodulated_normalized_value(),
            self.params.mix.modulated_normalized_value(),
            self.format_value(ParamId::Mix),
        );
        let row2_dials: [(ParamId, &str, f32, f32, String); 2] = [
            (
                ParamId::LowFreq,
                "Low",
                self.params.low_freq.unmodulated_normalized_value(),
                self.params.low_freq.modulated_normalized_value(),
                self.format_value(ParamId::LowFreq),
            ),
            (
                ParamId::HighFreq,
                "High",
                self.params.high_freq.unmodulated_normalized_value(),
                self.params.high_freq.modulated_normalized_value(),
                self.format_value(ParamId::HighFreq),
            ),
        ];
        let freeze_on = self.params.freeze.value();

        let tr = &mut self.text_renderer;

        // ── Row 1: Freeze button + Shift, Stretch, Mix, Feedback dials ──
        let dial_row_h = 56.0 * s;
        let dial_radius = 20.0 * s;

        // Freeze toggle button
        let freeze_btn_w = 44.0 * s;
        let freeze_btn_h = 20.0 * s;
        let freeze_x = pad;
        let freeze_y = y + (dial_row_h - freeze_btn_h) * 0.5;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            freeze_x,
            freeze_y,
            freeze_btn_w,
            freeze_btn_h,
            "Freeze",
            freeze_on,
            false,
        );
        self.drag.push_region(
            freeze_x,
            freeze_y,
            freeze_btn_w,
            freeze_btn_h,
            HitAction::Button(ButtonAction::Freeze),
        );

        let dial_area_start = freeze_x + freeze_btn_w + 8.0 * s;
        let dial_cy = y + dial_row_h * 0.5;

        // Timbre dials grouped left (Shift, Stretch, Feedback)
        let timbre_area_w = (w - dial_area_start - pad) * 0.65;
        let timbre_spacing = timbre_area_w / 3.0;

        for (i, (param_id, label, normalized, modulated, value_text)) in
            timbre_dials.iter().enumerate()
        {
            let cx = dial_area_start + timbre_spacing * (i as f32 + 0.5);
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(*param_id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                cx,
                dial_cy,
                dial_radius,
                label,
                value_text,
                *normalized,
                Some(*modulated),
                editing_buf.as_deref(),
                caret,
            );
            self.drag.push_region(
                dial_area_start + timbre_spacing * i as f32,
                y,
                timbre_spacing,
                dial_row_h,
                HitAction::Dial(*param_id),
            );
        }

        // Mix dial on the right
        {
            let (param_id, label, normalized, modulated, ref value_text) = mix_dial;
            let mix_cx = w - pad - dial_radius - 10.0 * s;
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(param_id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                mix_cx,
                dial_cy,
                dial_radius,
                label,
                value_text,
                normalized,
                Some(modulated),
                editing_buf.as_deref(),
                caret,
            );
            let mix_hit_w = (w - dial_area_start - timbre_area_w) * 0.8;
            self.drag.push_region(
                mix_cx - mix_hit_w * 0.5,
                y,
                mix_hit_w,
                dial_row_h,
                HitAction::Dial(param_id),
            );
        }

        y += dial_row_h;

        // ── Row 2: Low Freq, High Freq dials (aligned with timbre group) ──
        let row2_h = 52.0 * s;
        let row2_radius = 18.0 * s;
        let row2_spacing = timbre_area_w / 2.0;
        let row2_cy = y + row2_h * 0.5;

        for (i, (param_id, label, normalized, modulated, value_text)) in
            row2_dials.iter().enumerate()
        {
            let cx = dial_area_start + row2_spacing * (i as f32 + 0.5);
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(*param_id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                cx,
                row2_cy,
                row2_radius,
                label,
                value_text,
                *normalized,
                Some(*modulated),
                editing_buf.as_deref(),
                caret,
            );
            self.drag.push_region(
                dial_area_start + row2_spacing * i as f32,
                y,
                row2_spacing,
                row2_h,
                HitAction::Dial(*param_id),
            );
        }

        y += row2_h;

        // ── Waterfall spectrogram ──
        let waterfall_y = y + 8.0 * s;
        let waterfall_h = h - waterfall_y - pad;
        if waterfall_h > 0.0 {
            self.draw_waterfall(pad, waterfall_y, w - 2.0 * pad, waterfall_h);
        }
    }

    /// Draw the spectral waterfall using direct pixel writes with the color LUT.
    /// Redraws all columns every frame — the LUT + direct writes make this fast enough.
    fn draw_waterfall(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let wp = self.display.write_pos.load(Ordering::Relaxed);
        let pixmap_w = self.surface.pixmap.width() as usize;

        let x_start = x as usize;
        let y_start = y as usize;
        let w_px = w as usize;
        let h_px = h as usize;

        if w_px == 0 || h_px == 0 {
            return;
        }

        let data = self.surface.pixmap.data_mut();
        let pixmap_h = data.len() / (pixmap_w * 4);

        for col_i in 0..DISPLAY_COLUMNS {
            let col_idx = (wp + col_i) % DISPLAY_COLUMNS;
            // Evenly distribute pixels: each column spans from
            // col_i * w_px / N  to  (col_i+1) * w_px / N
            let col_x_start = x_start + col_i * w_px / DISPLAY_COLUMNS;
            let col_x_end = x_start + (col_i + 1) * w_px / DISPLAY_COLUMNS;

            // Bulk-read all bins for this column into a local array
            let mut mags = [0u8; DISPLAY_BINS]; // LUT indices
            for (bin, lut_idx) in mags.iter_mut().enumerate() {
                let mag = self.display.read(col_idx, bin);
                *lut_idx = magnitude_to_lut_index(mag) as u8;
            }

            // Write pixels directly
            for (bin, &lut_idx) in mags.iter().enumerate() {
                let color = self.color_lut[lut_idx as usize];
                // Low frequencies at bottom, high at top
                let bin_y_start = y_start + h_px - (bin + 1) * h_px / DISPLAY_BINS;
                let bin_y_end = y_start + h_px - bin * h_px / DISPLAY_BINS;

                for py in bin_y_start..bin_y_end.min(pixmap_h) {
                    let row_offset = py * pixmap_w * 4;
                    for px in col_x_start..col_x_end {
                        if px < pixmap_w {
                            let idx = row_offset + px * 4;
                            if idx + 3 < data.len() {
                                data[idx] = color[0];
                                data[idx + 1] = color[1];
                                data[idx + 2] = color[2];
                                data[idx + 3] = color[3];
                            }
                        }
                    }
                }
            }
        }

        // Border
        widgets::draw_rect_outline(
            &mut self.surface.pixmap,
            x,
            y,
            w,
            h,
            widgets::color_border(),
            1.0,
        );
    }

    fn format_value(&self, id: ParamId) -> String {
        match id {
            ParamId::Shift => format!("{:.1} st", self.params.shift.value()),
            ParamId::Stretch => format!("{:.2}x", self.params.stretch.value()),
            ParamId::Mix => format!("{:.0} %", self.params.mix.value()),
            ParamId::Feedback => format!("{:.0} %", self.params.feedback.value()),
            ParamId::LowFreq => format!("{:.0} Hz", self.params.low_freq.value()),
            ParamId::HighFreq => format!("{:.0} Hz", self.params.high_freq.value()),
        }
    }

    fn toggle_bool_param(&self, param: &BoolParam) {
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let current = param.value();
        setter.begin_set_parameter(param);
        setter.set_parameter_normalized(param, if current { 0.0 } else { 1.0 });
        setter.end_set_parameter(param);
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for WarpZoneWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        // Check for pending host-initiated resize
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
                // Derive scale factor from the new size
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if let Some(HitAction::Dial(param_id)) = self.drag.active_action().copied() {
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    let current = self.float_param(param_id).unmodulated_normalized_value();
                    if let Some(norm) = self.drag.update_drag(shift, current) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, norm);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                // Auto-commit any in-flight edit before starting a drag
                self.commit_text_edit();

                if let Some(region) = self.drag.hit_test().cloned() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());

                    // End any pending drag
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set_param(&setter, id);
                    }

                    let is_double = self.drag.check_double_click(&region.action);

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let norm =
                                    self.float_param(param_id).unmodulated_normalized_value();
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(param_id), norm, shift);
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::Button(ButtonAction::Freeze) => {
                            self.toggle_bool_param(&self.params.freeze);
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
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
                if let Some(region) = self.drag.hit_test().cloned() {
                    // Auto-commit any in-flight edit on a different widget.
                    self.commit_text_edit();
                    if let HitAction::Dial(param_id) = region.action {
                        let initial = self.formatted_value_without_unit(param_id);
                        self.text_edit.begin(HitAction::Dial(param_id), &initial);
                    }
                }
            }
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                if ev.state != keyboard_types::KeyState::Down {
                    // Swallow key-up events while editing so the host DAW doesn't
                    // process Enter/Escape releases as its own shortcuts.
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

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct WarpZoneEditor {
    params: Arc<WarpZoneParams>,
    display: Arc<SpectralDisplay>,
    scaling_factor: Arc<AtomicCell<f32>>,
    /// Pending host-initiated resize. Packed as (width << 32 | height).
    pending_resize: Arc<AtomicU64>,
}

pub fn create(
    params: Arc<WarpZoneParams>,
    display: Arc<SpectralDisplay>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(WarpZoneEditor {
        params,
        display,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for WarpZoneEditor {
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
        let display = Arc::clone(&self.display);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Warp Zone"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                WarpZoneWindow::new(
                    window,
                    gui_context,
                    params,
                    display,
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

#[cfg(test)]
mod text_entry_tests {
    use super::*;

    #[test]
    fn text_edit_roundtrip_for_shift_action() {
        let mut text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Shift))
            .is_none());

        text_edit.begin(HitAction::Dial(ParamId::Shift), "-12");
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Shift)),
            Some("-12")
        );

        text_edit.insert_char('5');
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Shift)),
            Some("-125")
        );

        let (action, buffer) = text_edit.commit().unwrap();
        assert_eq!(action, HitAction::Dial(ParamId::Shift));
        assert_eq!(buffer, "-125");
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Shift))
            .is_none());
    }

    #[test]
    fn state_starts_inactive() {
        let text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Shift))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Stretch))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Mix))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::Feedback))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::LowFreq))
            .is_none());
        assert!(text_edit
            .active_for(&HitAction::Dial(ParamId::HighFreq))
            .is_none());
    }
}
