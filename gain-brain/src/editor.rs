//! Softbuffer-based editor for gain-brain. CPU rendering via tiny-skia.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
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

/// A rectangular hit region with an associated action.
#[derive(Clone)]
struct HitRegion {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    action: HitAction,
}

#[derive(Clone, Copy, PartialEq)]
enum HitAction {
    Dial(ParamId),
    SteppedSegment { param: ParamId, index: i32 },
    Button(ButtonAction),
    GroupDecrement,
    GroupIncrement,
    ToggleInvert,
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Gain,
    LinkMode,
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    ScaleDown,
    ScaleUp,
}

struct GainBrainWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Shared with GainBrainEditor so Editor::size() stays in sync.
    shared_scale: Arc<AtomicCell<f32>>,

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

    /// Hit regions rebuilt each frame during draw().
    hit_regions: Vec<HitRegion>,
    /// Currently dragging a dial.
    drag_active: Option<HitAction>,
    /// Y coordinate where the current drag started (for dial vertical drag).
    drag_start_y: f32,
    /// Normalized value when the current drag started (for dial vertical drag).
    drag_start_value: f32,
    /// Shift state from the last mouse event, for detecting transitions.
    last_shift_state: bool,
    /// When shift is pressed mid-drag, snapshot the current Y and value
    /// so the fine-control drag is relative to that point.
    granular_drag_start_y: f32,
    granular_drag_start_value: f32,
    /// Mouse position in physical pixels.
    mouse_x: f32,
    mouse_y: f32,
    /// Timestamp of last click for double-click detection.
    last_click_time: std::time::Instant,
    last_click_action: Option<HitAction>,
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
        shared_scale: Arc<AtomicCell<f32>>,
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
            shared_scale,
            params,
            display_gain_millibels,
            gui_context_holder,
            user_gain_override,
            text_renderer,
            hit_regions: Vec::new(),
            drag_active: None,
            drag_start_y: 0.0,
            drag_start_value: 0.0,
            last_shift_state: false,
            granular_drag_start_y: 0.0,
            granular_drag_start_value: 0.0,
            mouse_x: 0.0,
            mouse_y: 0.0,
            last_click_time: std::time::Instant::now(),
            last_click_action: None,
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;

        // Clear hit regions and background
        self.hit_regions.clear();
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

        // ── Title row with scale controls on the right ──
        tr.draw_text(
            &mut self.surface.pixmap,
            pad,
            y + title_size,
            "Gain Brain",
            title_size,
            widgets::color_text(),
        );

        let scale_btn_size = 22.0 * s;
        let scale_label_w = 44.0 * s;
        let small_font = 11.0 * s;
        let w = self.physical_width as f32;

        // "+" button (rightmost)
        let plus_x = w - pad - scale_btn_size;
        let plus_y = y + 2.0 * s;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            plus_x,
            plus_y,
            scale_btn_size,
            scale_btn_size,
            "+",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: plus_x,
            y: plus_y,
            w: scale_btn_size,
            h: scale_btn_size,
            action: HitAction::Button(ButtonAction::ScaleUp),
        });

        // Scale percentage label
        let pct_text = format!("{}%", (self.scale_factor * 100.0).round() as u32);
        let pct_x = plus_x - scale_label_w;
        let pct_text_w = tr.text_width(&pct_text, small_font);
        tr.draw_text(
            &mut self.surface.pixmap,
            pct_x + (scale_label_w - pct_text_w) / 2.0,
            plus_y + small_font + 4.0 * s,
            &pct_text,
            small_font,
            widgets::color_muted(),
        );

        // "-" button
        let minus_x = pct_x - scale_btn_size;
        widgets::draw_button(
            &mut self.surface.pixmap,
            tr,
            minus_x,
            plus_y,
            scale_btn_size,
            scale_btn_size,
            "-",
            false,
            false,
        );
        self.hit_regions.push(HitRegion {
            x: minus_x,
            y: plus_y,
            w: scale_btn_size,
            h: scale_btn_size,
            action: HitAction::Button(ButtonAction::ScaleDown),
        });

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
        self.hit_regions.push(HitRegion {
            x: stepper_x,
            y: stepper_y,
            w: arrow_w,
            h: slider_h,
            action: HitAction::GroupDecrement,
        });

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
        self.hit_regions.push(HitRegion {
            x: right_x,
            y: stepper_y,
            w: arrow_w,
            h: slider_h,
            action: HitAction::GroupIncrement,
        });
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
                self.hit_regions.push(HitRegion {
                    x: link_x + i as f32 * link_seg_w,
                    y: link_y,
                    w: link_seg_w,
                    h: slider_h,
                    action: HitAction::SteppedSegment {
                        param: ParamId::LinkMode,
                        index: i,
                    },
                });
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
            self.hit_regions.push(HitRegion {
                x: inv_x,
                y: inv_y,
                w: inv_w,
                h: slider_h,
                action: HitAction::ToggleInvert,
            });

            y += row_h;
        }

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
        self.hit_regions.push(HitRegion {
            x: dial_cx - dial_radius - 10.0 * s,
            y,
            w: dial_radius * 2.0 + 20.0 * s,
            h: dial_total_h,
            action: HitAction::Dial(ParamId::Gain),
        });
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

    fn apply_scale_change(&mut self, delta: f32, window: &mut baseview::Window) {
        let old = self.scale_factor;
        self.scale_factor = (self.scale_factor + delta).clamp(0.75, 3.0);
        if (self.scale_factor - old).abs() > 0.01 {
            // Update shared scale so Editor::size() returns the correct value
            self.shared_scale.store(self.scale_factor);
            let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
            let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
            self.params.editor_state.store_size(new_w, new_h);
            window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            self.gui_context.request_resize();
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
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        let _param_setter = ParamSetter::new(self.gui_context.as_ref());

        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width;
                self.physical_height = info.physical_size().height;
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                if let Some(HitAction::Dial(param_id)) = self.drag_active {
                    let shift_now = modifiers.contains(keyboard_types::Modifiers::SHIFT);

                    // drag_start_value stores dB, not normalized.
                    // Detect shift transitions to re-anchor drag origin.
                    let current_display_db =
                        self.display_gain_millibels.load(Ordering::Relaxed) as f32 / 100.0;
                    if shift_now && !self.last_shift_state {
                        self.granular_drag_start_y = self.mouse_y;
                        self.granular_drag_start_value = current_display_db;
                    } else if !shift_now && self.last_shift_state {
                        self.drag_start_y = self.mouse_y;
                        self.drag_start_value = current_display_db;
                    }

                    // Drag in dB: 600px = 120 dB range, up = increase
                    let db_per_pixel = 120.0 / 600.0;
                    let target_db = if shift_now {
                        let delta_y = self.granular_drag_start_y - self.mouse_y;
                        (self.granular_drag_start_value + delta_y * db_per_pixel * 0.1)
                            .clamp(-60.0, 60.0)
                    } else {
                        let delta_y = self.drag_start_y - self.mouse_y;
                        (self.drag_start_value + delta_y * db_per_pixel).clamp(-60.0, 60.0)
                    };
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

                    self.last_shift_state = shift_now;
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

                // Find which hit region was clicked
                let hit = self
                    .hit_regions
                    .iter()
                    .find(|r| mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h)
                    .cloned();

                if let Some(region) = hit {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    let now = std::time::Instant::now();
                    let is_double_click = now.duration_since(self.last_click_time).as_millis()
                        < 400
                        && self.last_click_action.as_ref() == Some(&region.action);
                    self.last_click_time = now;
                    self.last_click_action = Some(region.action);

                    // End any pending drag before processing new click
                    if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                        self.end_set_param(&setter, id);
                    }

                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double_click {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                // Read the display atomic for the drag start value.
                                // This matches what draw() displays, so the drag
                                // starts from the visually shown position.
                                let display_db =
                                    self.display_gain_millibels.load(Ordering::Relaxed) as f32
                                        / 100.0;
                                self.drag_start_y = my;
                                self.drag_start_value = display_db;
                                self.granular_drag_start_y = my;
                                self.granular_drag_start_value = display_db;
                                self.last_shift_state =
                                    modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag_active = Some(HitAction::Dial(param_id));
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::SteppedSegment { param, index } => {
                            if is_double_click {
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
                        HitAction::Button(ButtonAction::ScaleDown) => {
                            self.apply_scale_change(-0.25, window);
                        }
                        HitAction::Button(ButtonAction::ScaleUp) => {
                            self.apply_scale_change(0.25, window);
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

                let hit = self
                    .hit_regions
                    .iter()
                    .find(|r| mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h)
                    .cloned();

                if let Some(_region) = hit {
                    // Right-click handling reserved for future use
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag_active.take() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
                }
            }
            baseview::Event::Keyboard(kb_event) => {
                use keyboard_types::{Key, KeyState, Modifiers};
                if kb_event.state == KeyState::Down
                    && kb_event.modifiers.contains(Modifiers::CONTROL)
                {
                    match &kb_event.key {
                        Key::Character(c) if c == "=" || c == "+" => {
                            self.apply_scale_change(0.25, window);
                        }
                        Key::Character(c) if c == "-" => {
                            self.apply_scale_change(-0.25, window);
                        }
                        _ => {}
                    }
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
    /// Shared with GainBrainWindow so Editor::size() reflects runtime changes.
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(
    params: Arc<GainBrainParams>,
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
    gui_context_holder: Arc<std::sync::Mutex<Option<Arc<dyn GuiContext>>>>,
    user_gain_override: Arc<std::sync::atomic::AtomicI32>,
) -> Option<Box<dyn Editor>> {
    // NOTE: persisted state may not be restored yet (host calls create() before set()).
    // Scale factor is derived from persisted size in spawn() instead.
    Some(Box::new(GainBrainEditor {
        params,
        display_gain_millibels,
        gui_context_holder,
        user_gain_override,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
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

        // Derive scale factor from persisted size (restored by host before spawn).
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.75, 3.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let display_gain = Arc::clone(&self.display_gain_millibels);
        let gui_ctx_holder = Arc::clone(&self.gui_context_holder);
        let user_gain_ovr = Arc::clone(&self.user_gain_override);
        let shared_scale = Arc::clone(&self.scaling_factor);

        let scaled_w = persisted_w;
        let scaled_h = persisted_h;

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Gain Brain"),
                size: baseview::Size::new(scaled_w as f64, scaled_h as f64),
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
                    shared_scale,
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
        let sf = self.scaling_factor.load();
        let w = (WINDOW_WIDTH as f32 * sf).round() as u32;
        let h = (WINDOW_HEIGHT as f32 * sf).round() as u32;
        (w, h)
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
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
