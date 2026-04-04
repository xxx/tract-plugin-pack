//! Softbuffer-based editor for gain-brain. CPU rendering via tiny-skia.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::GainBrainParams;
use tiny_skia_widgets as widgets;

const WINDOW_WIDTH: u32 = 300;
const WINDOW_HEIGHT: u32 = 250;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Window Handler ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum HitAction {
    Dial(ParamId),
    SteppedSegment { param: ParamId, index: i32 },
    GroupDecrement,
    GroupIncrement,
    ToggleInvert,
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Gain,
    LinkMode,
}

struct GainBrainWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Packed (w << 32 | h) pending host-initiated resize, read on next frame.
    pending_resize: Arc<std::sync::atomic::AtomicU64>,

    params: Arc<GainBrainParams>,
    /// Effective gain in millibels. Written by the audio thread (group sync)
    /// and by the editor (user drag). Read by draw() for dial arc + text.
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
    /// Shared holder for the GuiContext — populated on spawn so the audio
    /// thread's task_executor can update the host param even after the
    /// editor window closes. Stored here to keep the Arc alive; the actual
    /// usage is in `task_executor()` via the shared Mutex.
    #[allow(dead_code)]
    gui_context_holder: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    /// User gain override for the audio thread. Written here when the user
    /// drags or double-clicks the gain knob. The audio thread reads this
    /// to reliably detect user intent, bypassing the SyncGainParam race.
    user_gain_override: Arc<std::sync::atomic::AtomicI32>,
    text_renderer: widgets::TextRenderer,

    drag: widgets::DragState<HitAction>,
}

impl GainBrainWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<GainBrainParams>,
        display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
        gui_context_holder: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
        user_gain_override: Arc<std::sync::atomic::AtomicI32>,
        pending_resize: Arc<std::sync::atomic::AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        // Load embedded font
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
            display_gain_millibels,
            gui_context_holder,
            user_gain_override,
            text_renderer,
            drag: widgets::DragState::new(),
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;

        // Clear hit regions and background
        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let pad = 20.0 * s;
        let row_h = 35.0 * s;
        let label_w = 80.0 * s;
        let font_size = 14.0 * s;
        let title_size = 20.0 * s;
        let content_w = self.physical_width as f32 - 2.0 * pad;
        let slider_w = content_w - label_w;
        let slider_h = 26.0 * s;

        let mut y = 12.0 * s;
        let tr = &mut self.text_renderer;

        // ── Title ──
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + title_size,
            "Gain Brain",
            title_size,
            widgets::color_text(),
        );

        y += row_h;

        // ── Group selector (stepper: [<] value [>]) ──
        let group_val = self.params.group.value();
        let group_text = if group_val == 0 {
            "X".to_string()
        } else {
            group_val.to_string()
        };
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + font_size,
            "Group",
            font_size,
            widgets::color_muted(),
        );
        let stepper_x = pad + label_w;
        let stepper_y = y + 4.0 * s;
        let arrow_w = 28.0 * s;
        let value_w_group = 40.0 * s;

        // Left arrow button
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            stepper_x,
            stepper_y,
            arrow_w,
            slider_h,
            "<",
            false,
            false,
        );
        self.drag.push_region(stepper_x, stepper_y, arrow_w, slider_h, HitAction::GroupDecrement);

        // Value display (centered text, no hit region)
        let val_x = stepper_x + arrow_w + 2.0 * s;
        widgets::draw_rect(
            &mut self.surface.pixmap,
            val_x,
            stepper_y,
            value_w_group,
            slider_h,
            widgets::color_control_bg(),
        );
        let group_text_w = tr.text_width(&group_text, font_size);
        let group_text_x = val_x + (value_w_group - group_text_w) * 0.5;
        let group_text_y = stepper_y + (slider_h + font_size) * 0.5 - 2.0;
        tr.draw_text(
            &mut self.surface.pixmap,
            group_text_x,
            group_text_y,
            &group_text,
            font_size,
            widgets::color_text(),
        );

        // Right arrow button
        let right_x = val_x + value_w_group + 2.0 * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            right_x,
            stepper_y,
            arrow_w,
            slider_h,
            ">",
            false,
            false,
        );
        self.drag.push_region(right_x, stepper_y, arrow_w, slider_h, HitAction::GroupIncrement);
        y += row_h;

        // ── Link mode selector (only visible when group > 0) ──
        if group_val > 0 {
            let link_mode = self.params.link_mode.value();
            let link_idx = match link_mode {
                crate::LinkMode::Absolute => 0,
                crate::LinkMode::Relative => 1,
            };
            tr.draw_text(
                &mut self.surface.pixmap,
                pad,
                y + font_size,
                "Link",
                font_size,
                widgets::color_muted(),
            );
            let link_x = pad + label_w;
            let link_y = y + 4.0 * s;
            // Reduce link selector width to make room for Inv button
            let inv_w = 40.0 * s;
            let inv_gap = 8.0 * s;
            let link_selector_w = slider_w - inv_w - inv_gap;
            widgets::draw_stepped_selector(
                &mut self.surface.pixmap,
                tr,
                link_x,
                link_y,
                link_selector_w,
                slider_h,
                &["Abs", "Rel"],
                link_idx,
            );
            let link_seg_w = link_selector_w / 2.0;
            for i in 0..2_i32 {
                self.drag.push_region(
                    link_x + i as f32 * link_seg_w,
                    link_y,
                    link_seg_w,
                    slider_h,
                    HitAction::SteppedSegment {
                        param: ParamId::LinkMode,
                        index: i,
                    },
                );
            }

            // Inv toggle button
            let inv_active = self.params.invert.value();
            let inv_x = link_x + link_selector_w + inv_gap;
            let inv_y = link_y;
            widgets::draw_button(
                &mut self.surface.pixmap,
                tr,
                inv_x,
                inv_y,
                inv_w,
                slider_h,
                "Inv",
                inv_active,
                false,
            );
            self.drag.push_region(inv_x, inv_y, inv_w, slider_h, HitAction::ToggleInvert);

            y += row_h;
        }

        // Extra breathing room before the dial
        y += 8.0 * s;

        // ── Gain dial ──
        // Read the effective gain from the shared atomic. This is written by
        // process() (group sync) and by the editor drag handler, so it always
        // reflects the true effective gain without the async SyncGainParam lag.
        let gain_db = self.display_gain_millibels.load(Ordering::Relaxed) as f32 / 100.0;
        let gain_text = format!("{:+.1} dB", gain_db);
        // Map dB linearly to 0-1 for the dial arc so the visual position
        // matches the dB scale (not the skewed linear-gain parameter range).
        let dial_normalized = ((gain_db - (-60.0)) / (60.0 - (-60.0))).clamp(0.0, 1.0);
        let dial_radius = 40.0 * s;
        let dial_total_h = dial_radius * 2.0 + 30.0 * s; // arc + label + value text
        let dial_cx = self.physical_width as f32 / 2.0;
        let dial_cy = y + dial_radius + 20.0 * s;
        widgets::draw_dial(
            &mut self.surface.pixmap,
            tr,
            dial_cx,
            dial_cy,
            dial_radius,
            "Gain",
            &gain_text,
            dial_normalized,
        );
        // Hit region covers the full dial area for vertical drag
        self.drag.push_region(dial_cx - dial_radius - 10.0 * s, y, dial_radius * 2.0 + 20.0 * s, dial_total_h, HitAction::Dial(ParamId::Gain));
        let _ = y + dial_total_h; // suppress unused warning; y is the layout cursor
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.begin_set_parameter(&self.params.gain),
            ParamId::LinkMode => setter.begin_set_parameter(&self.params.link_mode),
        }
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        match id {
            ParamId::Gain => {
                setter.set_parameter_normalized(&self.params.gain, normalized);
                // Signal user intent to the audio thread so group sync
                // reliably detects this as a user change, even if a stale
                // SyncGainParam overwrites the param later.
                let gain = self.params.gain.preview_plain(normalized);
                let db = nih_plug::util::gain_to_db(gain);
                let mb = (db * 100.0).round() as i32;
                self.user_gain_override.store(mb, Ordering::Relaxed);
            }
            ParamId::LinkMode => {
                setter.set_parameter_normalized(&self.params.link_mode, normalized)
            }
        }
    }

    fn set_param_stepped(&self, setter: &ParamSetter, id: ParamId, index: i32) {
        match id {
            ParamId::LinkMode => {
                let mode = match index {
                    0 => crate::LinkMode::Absolute,
                    _ => crate::LinkMode::Relative,
                };
                setter.set_parameter(&self.params.link_mode, mode);
            }
            ParamId::Gain => {
                // Gain uses a continuous slider, not stepped segments.
            }
        }
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        match id {
            ParamId::Gain => {
                setter.begin_set_parameter(&self.params.gain);
                setter.set_parameter_normalized(
                    &self.params.gain,
                    self.params.gain.default_normalized_value(),
                );
                setter.end_set_parameter(&self.params.gain);
                // Update display atomic so draw() reflects the reset immediately.
                let default_db = nih_plug::util::gain_to_db(self.params.gain.default_plain_value());
                let default_mb = (default_db * 100.0).round() as i32;
                self.display_gain_millibels.store(default_mb, Ordering::Relaxed);
                // Signal user intent to the audio thread.
                self.user_gain_override.store(default_mb, Ordering::Relaxed);
            }
            ParamId::LinkMode => {
                setter.begin_set_parameter(&self.params.link_mode);
                setter.set_parameter_normalized(
                    &self.params.link_mode,
                    self.params.link_mode.default_normalized_value(),
                );
                setter.end_set_parameter(&self.params.link_mode);
            }
        }
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.end_set_parameter(&self.params.gain),
            ParamId::LinkMode => setter.end_set_parameter(&self.params.link_mode),
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for GainBrainWindow {
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
        let _param_setter = ParamSetter::new(self.gui_context.as_ref());

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
                    // Drag operates in dB space mapped to 0-1: (dB + 60) / 120
                    let current_db =
                        self.display_gain_millibels.load(Ordering::Relaxed) as f32 / 100.0;
                    let current_norm = (current_db + 60.0) / 120.0;
                    if let Some(norm) = self.drag.update_drag(shift, current_norm) {
                        let target_db = (norm * 120.0 - 60.0).clamp(-60.0, 60.0);
                        // Convert dB to the parameter's skewed normalized value
                        let target_linear = nih_plug::util::db_to_gain(target_db);
                        let normalized = self.params.gain.preview_normalized(target_linear);
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, normalized);
                        // Keep the display atomic in sync so draw() shows the
                        // dragged value immediately, even before process() runs.
                        self.display_gain_millibels.store(
                            (target_db * 100.0).round() as i32,
                            Ordering::Relaxed,
                        );
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                if let Some(region) = self.drag.hit_test().cloned() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());

                    // End any pending drag before processing new click
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set_param(&setter, id);
                    }

                    let is_double = self.drag.check_double_click(&region.action);

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                // Map dB to 0-1 for DragState: (dB + 60) / 120
                                let display_db =
                                    self.display_gain_millibels.load(Ordering::Relaxed) as f32
                                        / 100.0;
                                let norm = (display_db + 60.0) / 120.0;
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(param_id), norm, shift);
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::SteppedSegment { param, index } => {
                            if is_double {
                                self.reset_param_to_default(&setter, param);
                            } else {
                                self.begin_set_param(&setter, param);
                                self.set_param_stepped(&setter, param, index);
                                self.end_set_param(&setter, param);
                            }
                        }
                        HitAction::GroupDecrement => {
                            let cur = self.params.group.value();
                            let prev = if cur <= 0 { 16 } else { cur - 1 };
                            setter.begin_set_parameter(&self.params.group);
                            setter.set_parameter(&self.params.group, prev);
                            setter.end_set_parameter(&self.params.group);
                        }
                        HitAction::GroupIncrement => {
                            let cur = self.params.group.value();
                            let next = if cur >= 16 { 0 } else { cur + 1 };
                            setter.begin_set_parameter(&self.params.group);
                            setter.set_parameter(&self.params.group, next);
                            setter.end_set_parameter(&self.params.group);
                        }
                        HitAction::ToggleInvert => {
                            let current = self.params.invert.value();
                            setter.begin_set_parameter(&self.params.invert);
                            setter.set_parameter(&self.params.invert, !current);
                            setter.end_set_parameter(&self.params.invert);
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                if let Some(_region) = self.drag.hit_test() {
                    // Right-click handling reserved for future use
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
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct GainBrainEditor {
    params: Arc<GainBrainParams>,
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
    /// Shared holder for the GuiContext so the audio thread's task_executor
    /// can use ParamSetter even when the editor window is closed.
    gui_context_holder: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    user_gain_override: Arc<std::sync::atomic::AtomicI32>,
    /// Packed (w << 32 | h) for host-initiated resize, consumed by window on next frame.
    pending_resize: Arc<std::sync::atomic::AtomicU64>,
}

pub(crate) fn create(
    params: Arc<GainBrainParams>,
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
    gui_context_holder: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    user_gain_override: Arc<std::sync::atomic::AtomicI32>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(GainBrainEditor {
        params,
        display_gain_millibels,
        gui_context_holder,
        user_gain_override,
        pending_resize: Arc::new(std::sync::atomic::AtomicU64::new(0)),
    }))
}

impl Editor for GainBrainEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        // Store the GuiContext in the shared holder so the audio thread's
        // task_executor can use ParamSetter even after this window closes.
        // The GuiContext Arc remains valid for the plugin's entire lifetime.
        if let Ok(mut guard) = self.gui_context_holder.lock() {
            *guard = Some(Arc::clone(&context));
        }

        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let display_gain = Arc::clone(&self.display_gain_millibels);
        let gui_ctx_holder = Arc::clone(&self.gui_context_holder);
        let user_gain_ovr = Arc::clone(&self.user_gain_override);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Gain Brain"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                GainBrainWindow::new(
                    window,
                    gui_context,
                    params,
                    display_gain,
                    gui_ctx_holder,
                    user_gain_ovr,
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

    fn param_value_changed(&self, id: &str, _normalized_value: f32) {
        if id == "gain" {
            let gain_db = nih_plug::util::gain_to_db(self.params.gain.value());
            self.display_gain_millibels.store(
                (gain_db * 100.0).round() as i32,
                Ordering::Relaxed,
            );
        }
    }
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {
        let gain_db = nih_plug::util::gain_to_db(self.params.gain.value());
        self.display_gain_millibels.store(
            (gain_db * 100.0).round() as i32,
            Ordering::Relaxed,
        );
    }
}
