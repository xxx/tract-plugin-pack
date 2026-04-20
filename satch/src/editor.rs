//! Softbuffer-based editor for satch. CPU rendering via tiny-skia.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::SatchParams;
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 300;
const WINDOW_HEIGHT: u32 = 380;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum HitAction {
    Dial(ParamId),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ParamId {
    Gain,
    Threshold,
    Knee,
    Detail,
    Mix,
}

// ── Window handler ──────────────────────────────────────────────────────

struct SatchWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Packed (w << 32 | h) pending host-initiated resize, read on next frame.
    pending_resize: Arc<std::sync::atomic::AtomicU64>,

    params: Arc<SatchParams>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,
}

impl SatchWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<SatchParams>,
        pending_resize: Arc<std::sync::atomic::AtomicU64>,
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
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
        }
    }

    // ── Param access helpers ────────────────────────────────────────────

    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Gain => &self.params.gain,
            ParamId::Threshold => &self.params.threshold,
            ParamId::Knee => &self.params.knee,
            ParamId::Detail => &self.params.detail,
            ParamId::Mix => &self.params.mix,
        }
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
        let HitAction::Dial(param_id) = action;
        let p = self.float_param(param_id);
        let norm = p.string_to_normalized_value(&text);
        let Some(norm) = norm else { return };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set_param(&setter, param_id);
        self.set_param_normalized(&setter, param_id, norm);
        self.end_set_param(&setter, param_id);
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

    // ── Drawing ─────────────────────────────────────────────────────────

    fn draw(&mut self) {
        let s = self.scale_factor;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let pad = 20.0 * s;
        let title_size = 20.0 * s;
        let w = self.physical_width as f32;
        let h = self.physical_height as f32;

        let mut y = 12.0 * s;

        // Pre-collect dial data before taking &mut tr to avoid borrow conflicts.
        // Row 1: Gain, Threshold
        // Row 2: Detail, Knee
        // Bottom-right: Mix
        let row1: [(ParamId, &str, f32, String); 2] = [
            (
                ParamId::Gain,
                "Gain",
                self.params.gain.unmodulated_normalized_value(),
                self.format_value(ParamId::Gain),
            ),
            (
                ParamId::Threshold,
                "Threshold",
                self.params.threshold.unmodulated_normalized_value(),
                self.format_value(ParamId::Threshold),
            ),
        ];
        let row2: [(ParamId, &str, f32, String); 2] = [
            (
                ParamId::Detail,
                "Detail",
                self.params.detail.unmodulated_normalized_value(),
                self.format_value(ParamId::Detail),
            ),
            (
                ParamId::Knee,
                "Knee",
                self.params.knee.unmodulated_normalized_value(),
                self.format_value(ParamId::Knee),
            ),
        ];
        let mix_data = (
            ParamId::Mix,
            "Mix",
            self.params.mix.unmodulated_normalized_value(),
            self.format_value(ParamId::Mix),
        );
        let tr = &mut self.text_renderer;

        // ── Title ──
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + title_size,
            "satch",
            title_size,
            widgets::color_text(),
        );

        y += 36.0 * s; // title row height

        // Layout: 2 rows of 2 dials + Mix bottom-right
        let available_h = h - y - pad;
        let dial_radius = 28.0 * s;
        let col_spacing = (w - 2.0 * pad) / 2.0;
        let row_h = available_h / 3.0;
        let row1_cy = y + row_h * 0.5;
        let row2_cy = y + row_h * 1.5;

        // Row 1: Gain, Threshold
        for (i, (param_id, label, normalized, value_text)) in row1.iter().enumerate() {
            let cx = pad + col_spacing * (i as f32 + 0.5);
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(*param_id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                cx,
                row1_cy,
                dial_radius,
                label,
                value_text,
                *normalized,
                None,
                editing_buf.as_deref(),
                caret,
            );
            self.drag.push_region(cx - col_spacing / 2.0, y, col_spacing, row_h, HitAction::Dial(*param_id));
        }

        // Row 2: Detail, Knee
        for (i, (param_id, label, normalized, value_text)) in row2.iter().enumerate() {
            let cx = pad + col_spacing * (i as f32 + 0.5);
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
                dial_radius,
                label,
                value_text,
                *normalized,
                None,
                editing_buf.as_deref(),
                caret,
            );
            self.drag.push_region(cx - col_spacing / 2.0, y + row_h, col_spacing, row_h, HitAction::Dial(*param_id));
        }

        // Mix: bottom-right corner, smaller
        {
            let (param_id, label, normalized, ref value_text) = mix_data;
            let mix_radius = 22.0 * s;
            let mix_cx = w - pad - mix_radius - 10.0 * s;
            let mix_cy = y + row_h * 2.5;
            let editing_buf: Option<String> = self
                .text_edit
                .active_for(&HitAction::Dial(param_id))
                .map(str::to_owned);
            let caret = self.text_edit.caret_visible();
            widgets::draw_dial_ex(
                &mut self.surface.pixmap,
                tr,
                mix_cx,
                mix_cy,
                mix_radius,
                label,
                value_text,
                normalized,
                None,
                editing_buf.as_deref(),
                caret,
            );
            let hit_w = col_spacing;
            self.drag.push_region(mix_cx - hit_w / 2.0, y + row_h * 2.0, hit_w, row_h, HitAction::Dial(param_id));
        }
    }

    fn format_value(&self, id: ParamId) -> String {
        match id {
            ParamId::Gain => {
                let db = nih_plug::util::gain_to_db(self.params.gain.value());
                format!("{:.1} dB", db)
            }
            ParamId::Threshold => {
                let db = nih_plug::util::gain_to_db(self.params.threshold.value());
                format!("{:.1} dB", db)
            }
            ParamId::Knee => {
                format!("{:.0} %", self.params.knee.value())
            }
            ParamId::Detail => {
                format!("{:.0} %", self.params.detail.value())
            }
            ParamId::Mix => {
                format!("{:.0} %", self.params.mix.value())
            }
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for SatchWindow {
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

                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set_param(&setter, id);
                    }

                    let is_double = self.drag.check_double_click(&region.action);

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let norm = self.float_param(param_id).unmodulated_normalized_value();
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(param_id), norm, shift);
                                self.begin_set_param(&setter, param_id);
                            }
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
                    let HitAction::Dial(param_id) = region.action;
                    let initial = self.formatted_value_without_unit(param_id);
                    self.text_edit.begin(HitAction::Dial(param_id), &initial);
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

pub(crate) struct SatchEditor {
    params: Arc<SatchParams>,
    pending_resize: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) fn create(params: Arc<SatchParams>) -> Option<Box<dyn Editor>> {
    Some(Box::new(SatchEditor {
        params,
        pending_resize: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    }))
}

impl Editor for SatchEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("satch"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| SatchWindow::new(window, gui_context, params, pending_resize, sf),
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
mod text_entry_tests {
    use super::*;

    #[test]
    fn text_edit_roundtrip_for_threshold_action() {
        let mut text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Threshold)).is_none());

        text_edit.begin(HitAction::Dial(ParamId::Threshold), "5.0");
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Threshold)),
            Some("5.0")
        );

        text_edit.insert_char('0');
        assert_eq!(
            text_edit.active_for(&HitAction::Dial(ParamId::Threshold)),
            Some("5.00")
        );

        let (action, buffer) = text_edit.commit().unwrap();
        assert_eq!(action, HitAction::Dial(ParamId::Threshold));
        assert_eq!(buffer, "5.00");
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Threshold)).is_none());
    }

    #[test]
    fn state_starts_inactive() {
        let text_edit: widgets::TextEditState<HitAction> = widgets::TextEditState::new();
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Gain)).is_none());
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Threshold)).is_none());
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Detail)).is_none());
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Knee)).is_none());
        assert!(text_edit.active_for(&HitAction::Dial(ParamId::Mix)).is_none());
    }
}
