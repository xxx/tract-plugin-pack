//! Softbuffer + tiny-skia editor for HD26. CPU rendering, freely resizable.
//! Two labeled sections (Hyper / Dimension), a global row, and a Retrig
//! activity indicator (input-level bar + trigger LED) fed by lock-free
//! telemetry from the audio thread.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
// Brings the `Param` trait into scope for `.modulated_normalized_value()` etc.
// (mirrors gain-brain). If the compiler flags this as a redundant import under
// `-D warnings` because the prelude glob already provides it, delete this line.
use nih_plug::prelude::Param;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::{DimensionMode, Hd26Params, Telemetry};
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 560;
const WINDOW_HEIGHT: u32 = 280;
const LED_FLASH_FRAMES: u32 = 8;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ParamId {
    Unison,
    Detune,
    Rate,
    Width,
    Sensitivity,
    HyperMix,
    Size,
    Hpf,
    DimMix,
    Output,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HitAction {
    Dial(ParamId),
    ModeSegment { index: i32 },
    ToggleRetrig,
    ToggleBypass,
}

/// Bind a `ParamId` to its concrete param field and run `$body` with `$p`.
/// Works for both `IntParam` (Unison) and `FloatParam` via the `Param` trait.
macro_rules! dispatch {
    ($self:expr, $id:expr, $p:ident => $body:expr) => {
        match $id {
            ParamId::Unison => {
                let $p = &$self.params.hyper_unison;
                $body
            }
            ParamId::Detune => {
                let $p = &$self.params.hyper_detune;
                $body
            }
            ParamId::Rate => {
                let $p = &$self.params.hyper_rate;
                $body
            }
            ParamId::Width => {
                let $p = &$self.params.hyper_width;
                $body
            }
            ParamId::Sensitivity => {
                let $p = &$self.params.hyper_sensitivity;
                $body
            }
            ParamId::HyperMix => {
                let $p = &$self.params.hyper_mix;
                $body
            }
            ParamId::Size => {
                let $p = &$self.params.dim_size;
                $body
            }
            ParamId::Hpf => {
                let $p = &$self.params.dim_hpf;
                $body
            }
            ParamId::DimMix => {
                let $p = &$self.params.dim_mix;
                $body
            }
            ParamId::Output => {
                let $p = &$self.params.output;
                $body
            }
        }
    };
}

struct Hd26Window {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    pending_resize: Arc<AtomicU64>,
    params: Arc<Hd26Params>,
    telemetry: Arc<Telemetry>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,
    last_trigger_count: u32,
    led_flash: u32,
}

impl Hd26Window {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<Hd26Params>,
        telemetry: Arc<Telemetry>,
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
            telemetry,
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            last_trigger_count: 0,
            led_flash: 0,
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;
        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let r = 28.0 * s;
        let title = 18.0 * s;
        let lbl = 13.0 * s;

        // Section titles.
        self.text_renderer.draw_text(
            &mut self.surface.pixmap,
            16.0 * s,
            22.0 * s,
            "HYPER",
            title,
            widgets::color_text(),
        );
        self.text_renderer.draw_text(
            &mut self.surface.pixmap,
            400.0 * s,
            22.0 * s,
            "DIMENSION",
            title,
            widgets::color_text(),
        );

        // Vertical divider.
        widgets::draw_rect(
            &mut self.surface.pixmap,
            378.0 * s,
            12.0 * s,
            1.0 * s,
            210.0 * s,
            widgets::color_muted(),
        );

        // Dial grid: (id, label, cx, cy) in logical px.
        let hx = [56.0, 140.0, 224.0, 308.0];
        let row0 = 64.0;
        let row1 = 148.0;
        let dials: [(ParamId, &str, f32, f32); 9] = [
            (ParamId::Unison, "Voices", hx[0], row0),
            (ParamId::Detune, "Detune", hx[1], row0),
            (ParamId::Rate, "Rate", hx[2], row0),
            (ParamId::Width, "Width", hx[3], row0),
            (ParamId::Sensitivity, "Sens", hx[0], row1),
            (ParamId::HyperMix, "Mix", hx[1], row1),
            (ParamId::Size, "Size", 432.0, row0),
            (ParamId::Hpf, "Wet HPF", 516.0, row0),
            (ParamId::DimMix, "Mix", 432.0, row1),
        ];

        let accent = widgets::color_accent();
        for &(id, label, lcx, lcy) in dials.iter() {
            let cx = lcx * s;
            let cy = lcy * s;
            let normalized = dispatch!(self, id, p => p.modulated_normalized_value());
            let value_text = dispatch!(self, id, p => p.normalized_value_to_string(p.modulated_normalized_value(), true));
            let editing = self
                .text_edit
                .active_for(&HitAction::Dial(id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                &mut self.text_renderer,
                cx,
                cy,
                r,
                label,
                &value_text,
                normalized,
                None,
                editing.as_deref(),
                caret,
                accent,
            );
            self.drag.push_region(
                cx - r - 6.0 * s,
                cy - r - 14.0 * s,
                2.0 * r + 12.0 * s,
                2.0 * r + 40.0 * s,
                HitAction::Dial(id),
            );
        }

        // Retrig toggle (Hyper, row1, col2).
        let retrig_on = self.params.hyper_retrig.value();
        let btn_w = 56.0 * s;
        let btn_h = 22.0 * s;
        let retrig_x = (224.0 - 28.0) * s;
        let retrig_y = (row1 - 8.0) * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            retrig_x,
            retrig_y,
            btn_w,
            btn_h,
            "Retrig",
            retrig_on,
            false,
        );
        self.drag
            .push_region(retrig_x, retrig_y, btn_w, btn_h, HitAction::ToggleRetrig);

        // ── Retrig activity indicator (level bar + LED) ──
        let level = f32::from_bits(self.telemetry.level.load(Ordering::Relaxed)).clamp(0.0, 1.0);
        let trig = self.telemetry.trigger_count.load(Ordering::Relaxed);
        if trig != self.last_trigger_count {
            self.last_trigger_count = trig;
            self.led_flash = LED_FLASH_FRAMES;
        }
        let meter_x = (224.0 - 28.0) * s;
        let meter_y = (row1 + 22.0) * s;
        let meter_w = 140.0 * s;
        let meter_h = 12.0 * s;
        self.text_renderer.draw_text(
            &mut self.surface.pixmap,
            meter_x,
            meter_y - 4.0 * s,
            "input",
            lbl,
            widgets::color_muted(),
        );
        widgets::draw_rect(
            &mut self.surface.pixmap,
            meter_x,
            meter_y,
            meter_w,
            meter_h,
            widgets::color_control_bg(),
        );
        if level > 0.0 {
            widgets::draw_rect(
                &mut self.surface.pixmap,
                meter_x,
                meter_y,
                meter_w * level,
                meter_h,
                accent,
            );
        }
        let led_x = meter_x + meter_w + 8.0 * s;
        let led_c = if self.led_flash > 0 {
            tiny_skia::Color::from_rgba8(255, 80, 60, 255)
        } else {
            widgets::color_control_bg()
        };
        widgets::draw_rect(
            &mut self.surface.pixmap,
            led_x,
            meter_y,
            meter_h,
            meter_h,
            led_c,
        );
        if self.led_flash > 0 {
            self.led_flash -= 1;
        }

        // Dimension Mode selector (2-segment).
        let mode_idx = match self.params.dim_mode.value() {
            DimensionMode::Am => 0,
            DimensionMode::Pitch => 1,
        };
        let mode_x = 474.0 * s;
        let mode_y = (row1 - 8.0) * s;
        let mode_w = 84.0 * s;
        let mode_h = 22.0 * s;
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mode_x,
            mode_y,
            mode_w,
            mode_h,
            &["AM", "Pitch"],
            mode_idx,
            None,
        );
        let seg_w = mode_w / 2.0;
        for i in 0..2_i32 {
            self.drag.push_region(
                mode_x + i as f32 * seg_w,
                mode_y,
                seg_w,
                mode_h,
                HitAction::ModeSegment { index: i },
            );
        }
        self.text_renderer.draw_text(
            &mut self.surface.pixmap,
            mode_x,
            mode_y - 6.0 * s,
            "Mode",
            lbl,
            widgets::color_muted(),
        );

        // ── Global row: Output dial + Bypass ──
        let out_cx = 200.0 * s;
        let out_cy = 236.0 * s;
        let normalized = self.params.output.modulated_normalized_value();
        let value_text = self
            .params
            .output
            .normalized_value_to_string(normalized, true);
        let editing = self
            .text_edit
            .active_for(&HitAction::Dial(ParamId::Output))
            .map(str::to_owned);
        let caret = self.text_edit.caret_visible();
        widgets::draw_dial_ex(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            out_cx,
            out_cy,
            r * 0.8,
            "Output",
            &value_text,
            normalized,
            None,
            editing.as_deref(),
            caret,
            accent,
        );
        self.drag.push_region(
            out_cx - r - 6.0 * s,
            out_cy - r,
            2.0 * r + 12.0 * s,
            2.0 * r + 30.0 * s,
            HitAction::Dial(ParamId::Output),
        );

        let bypass_on = self.params.bypass.value();
        let by_x = 300.0 * s;
        let by_y = 228.0 * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            by_x,
            by_y,
            70.0 * s,
            24.0 * s,
            "Bypass",
            bypass_on,
            false,
        );
        self.drag
            .push_region(by_x, by_y, 70.0 * s, 24.0 * s, HitAction::ToggleBypass);
    }

    fn formatted_value_without_unit(&self, id: ParamId) -> String {
        dispatch!(self, id, p => p.normalized_value_to_string(p.modulated_normalized_value(), false))
    }

    fn string_to_norm(&self, id: ParamId, text: &str) -> Option<f32> {
        dispatch!(self, id, p => p.string_to_normalized_value(text))
    }

    fn begin_set(&self, setter: &ParamSetter, id: ParamId) {
        dispatch!(self, id, p => setter.begin_set_parameter(p));
    }

    fn set_norm(&self, setter: &ParamSetter, id: ParamId, norm: f32) {
        dispatch!(self, id, p => setter.set_parameter_normalized(p, norm));
    }

    fn end_set(&self, setter: &ParamSetter, id: ParamId) {
        dispatch!(self, id, p => setter.end_set_parameter(p));
    }

    fn reset_default(&self, setter: &ParamSetter, id: ParamId) {
        dispatch!(self, id, p => {
            setter.begin_set_parameter(p);
            setter.set_parameter_normalized(p, p.default_normalized_value());
            setter.end_set_parameter(p);
        });
    }

    fn commit_text_edit(&mut self) {
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let HitAction::Dial(id) = action else {
            return;
        };
        let Some(norm) = self.string_to_norm(id, &text) else {
            return;
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set(&setter, id);
        self.set_norm(&setter, id, norm);
        self.end_set(&setter, id);
    }

    fn resize_buffers(&mut self) {
        self.surface.resize_and_persist(
            self.physical_width,
            self.physical_height,
            &self.params.editor_state,
        );
    }
}

impl baseview::WindowHandler for Hd26Window {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        widgets::consume_pending_resize(
            &self.pending_resize,
            (self.physical_width, self.physical_height),
            window,
        );
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
                self.scale_factor =
                    (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if let Some(HitAction::Dial(id)) = self.drag.active_action().copied() {
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    let current = dispatch!(self, id, p => p.modulated_normalized_value());
                    if let Some(norm) = self.drag.update_drag(shift, current) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_norm(&setter, id, norm);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                self.commit_text_edit();
                if let Some(region) = self.drag.hit_test().cloned() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set(&setter, id);
                    }
                    let is_double = self.drag.check_double_click(&region.action);
                    match region.action {
                        HitAction::Dial(id) => {
                            if is_double {
                                self.reset_default(&setter, id);
                            } else {
                                let norm = dispatch!(self, id, p => p.modulated_normalized_value());
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(id), norm, shift);
                                self.begin_set(&setter, id);
                            }
                        }
                        HitAction::ModeSegment { index } => {
                            let mode = if index == 0 {
                                DimensionMode::Am
                            } else {
                                DimensionMode::Pitch
                            };
                            setter.begin_set_parameter(&self.params.dim_mode);
                            setter.set_parameter(&self.params.dim_mode, mode);
                            setter.end_set_parameter(&self.params.dim_mode);
                        }
                        HitAction::ToggleRetrig => {
                            let v = self.params.hyper_retrig.value();
                            setter.begin_set_parameter(&self.params.hyper_retrig);
                            setter.set_parameter(&self.params.hyper_retrig, !v);
                            setter.end_set_parameter(&self.params.hyper_retrig);
                        }
                        HitAction::ToggleBypass => {
                            let v = self.params.bypass.value();
                            setter.begin_set_parameter(&self.params.bypass);
                            setter.set_parameter(&self.params.bypass, !v);
                            setter.end_set_parameter(&self.params.bypass);
                        }
                    }
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
                    if let HitAction::Dial(id) = region.action {
                        let initial = self.formatted_value_without_unit(id);
                        self.text_edit.begin(HitAction::Dial(id), &initial);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set(&setter, id);
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

pub(crate) struct Hd26Editor {
    params: Arc<Hd26Params>,
    telemetry: Arc<Telemetry>,
    pending_resize: Arc<AtomicU64>,
}

pub(crate) fn create(
    params: Arc<Hd26Params>,
    telemetry: Arc<Telemetry>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(Hd26Editor {
        params,
        telemetry,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for Hd26Editor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let telemetry = Arc::clone(&self.telemetry);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("HD26"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                Hd26Window::new(window, gui_context, params, telemetry, pending_resize, sf)
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
    use tiny_skia_widgets::TextEditState;

    #[test]
    fn text_edit_roundtrip_for_dial() {
        let mut s: TextEditState<HitAction> = TextEditState::new();
        s.begin(HitAction::Dial(ParamId::Detune), "");
        for c in "42".chars() {
            s.insert_char(c);
        }
        assert_eq!(
            s.commit(),
            Some((HitAction::Dial(ParamId::Detune), "42".to_string()))
        );
    }
}
