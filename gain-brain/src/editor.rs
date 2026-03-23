//! Softbuffer-based editor for gain-brain. CPU rendering via tiny-skia.

use baseview::{WindowHandle, WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
use serde::{Deserialize, Serialize};
use std::num::{NonZeroIsize, NonZeroU32};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tiny_skia_widgets as widgets;
use crate::GainBrainParams;

const WINDOW_WIDTH: u32 = 300;
const WINDOW_HEIGHT: u32 = 250;

// ── Editor State (persisted by the host) ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct GainBrainEditorState {
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,
    #[serde(skip)]
    open: AtomicBool,
}

impl GainBrainEditorState {
    pub fn from_size(width: u32, height: u32) -> Arc<Self> {
        Arc::new(Self {
            size: AtomicCell::new((width, height)),
            open: AtomicBool::new(false),
        })
    }

    pub fn default_state() -> Arc<Self> {
        Self::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
    }

    pub fn size(&self) -> (u32, u32) {
        self.size.load()
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }
}

impl<'a> PersistentField<'a, GainBrainEditorState> for Arc<GainBrainEditorState> {
    fn set(&self, new_value: GainBrainEditorState) {
        self.size.store(new_value.size.load());
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&GainBrainEditorState) -> R,
    {
        f(self)
    }
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
    _sb_context: softbuffer::Context<SoftbufferHandleAdapter>,
    sb_surface: softbuffer::Surface<SoftbufferHandleAdapter, SoftbufferHandleAdapter>,
    pixmap: tiny_skia::Pixmap,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Shared with GainBrainEditor so Editor::size() stays in sync.
    shared_scale: Arc<AtomicCell<f32>>,

    params: Arc<GainBrainParams>,
    /// Effective gain in millibels, written by the audio thread.
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
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
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<GainBrainParams>,
        display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
        shared_scale: Arc<AtomicCell<f32>>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let target = baseview_window_to_surface_target(window);
        let sb_context =
            softbuffer::Context::new(target.clone()).expect("could not get softbuffer context");
        let mut sb_surface = softbuffer::Surface::new(&sb_context, target)
            .expect("could not create softbuffer surface");
        sb_surface
            .resize(NonZeroU32::new(pw).unwrap(), NonZeroU32::new(ph).unwrap())
            .unwrap();

        let pixmap = tiny_skia::Pixmap::new(pw, ph).expect("could not create pixmap");

        // Load embedded font
        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            _sb_context: sb_context,
            sb_surface,
            pixmap,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            params,
            display_gain_millibels,
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
        self.pixmap.fill(widgets::color_bg());

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
            &mut self.pixmap,
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
            &mut self.pixmap,
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
            &mut self.pixmap,
            pct_x + (scale_label_w - pct_text_w) / 2.0,
            plus_y + small_font + 4.0 * s,
            &pct_text,
            small_font,
            widgets::color_muted(),
        );

        // "-" button
        let minus_x = pct_x - scale_btn_size;
        widgets::draw_button(
            &mut self.pixmap,
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
            &mut self.pixmap,
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
        widgets::draw_button(&mut self.pixmap, tr, stepper_x, stepper_y, arrow_w, slider_h, "<", false, false);
        self.hit_regions.push(HitRegion {
            x: stepper_x, y: stepper_y, w: arrow_w, h: slider_h,
            action: HitAction::GroupDecrement,
        });

        // Value display (centered text, no hit region)
        let val_x = stepper_x + arrow_w + 2.0 * s;
        widgets::draw_rect(&mut self.pixmap, val_x, stepper_y, value_w_group, slider_h, widgets::color_control_bg());
        let group_text_w = tr.text_width(&group_text, font_size);
        let group_text_x = val_x + (value_w_group - group_text_w) * 0.5;
        let group_text_y = stepper_y + (slider_h + font_size) * 0.5 - 2.0;
        tr.draw_text(&mut self.pixmap, group_text_x, group_text_y, &group_text, font_size, widgets::color_text());

        // Right arrow button
        let right_x = val_x + value_w_group + 2.0 * s;
        widgets::draw_button(&mut self.pixmap, tr, right_x, stepper_y, arrow_w, slider_h, ">", false, false);
        self.hit_regions.push(HitRegion {
            x: right_x, y: stepper_y, w: arrow_w, h: slider_h,
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
                &mut self.pixmap,
                pad,
                y + font_size,
                "Link",
                font_size,
                widgets::color_muted(),
            );
            let link_x = pad + label_w;
            let link_y = y + 4.0 * s;
            widgets::draw_stepped_selector(
                &mut self.pixmap,
                tr,
                link_x,
                link_y,
                slider_w,
                slider_h,
                &["Abs", "Rel"],
                link_idx,
            );
            let link_seg_w = slider_w / 2.0;
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
            y += row_h;
        }

        // ── Gain dial ──
        let display_mb = self.display_gain_millibels.load(Ordering::Relaxed);
        let gain_db = display_mb as f32 / 100.0;
        let gain_text = format!("{:+.1} dB", gain_db);
        // Map dB linearly to 0-1 for the dial arc so the visual position
        // matches the dB scale (not the skewed linear-gain parameter range).
        let dial_normalized = ((gain_db - (-60.0)) / (60.0 - (-60.0))).clamp(0.0, 1.0);
        let dial_radius = 40.0 * s;
        let dial_total_h = dial_radius * 2.0 + 30.0 * s; // arc + label + value text
        let dial_cx = self.physical_width as f32 / 2.0;
        let dial_cy = y + dial_radius + 20.0 * s;
        widgets::draw_dial(
            &mut self.pixmap,
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
            ParamId::Gain => setter.set_parameter_normalized(&self.params.gain, normalized),
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
            self.params.editor_state.size.store((new_w, new_h));
            window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            self.gui_context.request_resize();
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        if let Some(new_pixmap) = tiny_skia::Pixmap::new(pw, ph) {
            self.pixmap = new_pixmap;
        }
        let _ = self.sb_surface.resize(
            NonZeroU32::new(pw).unwrap(),
            NonZeroU32::new(ph).unwrap(),
        );
        self.params.editor_state.size.store((
            (pw as f32 / self.scale_factor).round() as u32,
            (ph as f32 / self.scale_factor).round() as u32,
        ));
    }

    fn present(&mut self) {
        let mut buffer = self.sb_surface.buffer_mut().unwrap();
        let data = self.pixmap.data();
        // Convert tiny-skia premultiplied RGBA to softbuffer 0x00RRGGBB
        for (dst, src) in buffer.iter_mut().zip(data.chunks_exact(4)) {
            let r = src[0] as u32;
            let g = src[1] as u32;
            let b = src[2] as u32;
            *dst = 0xFF000000 | (r << 16) | (g << 8) | b;
        }
        buffer.present().unwrap();
    }
}

impl baseview::WindowHandler for GainBrainWindow {
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.present();
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
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, modifiers }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                if let Some(HitAction::Dial(param_id)) = self.drag_active {
                    let shift_now = modifiers.contains(keyboard_types::Modifiers::SHIFT);

                    // Detect shift transitions to re-anchor drag origin
                    if shift_now && !self.last_shift_state {
                        // Shift just pressed: anchor granular drag here
                        self.granular_drag_start_y = self.mouse_y;
                        self.granular_drag_start_value =
                            self.params.gain.unmodulated_normalized_value();
                    } else if !shift_now && self.last_shift_state {
                        // Shift just released: re-anchor normal drag here
                        self.drag_start_y = self.mouse_y;
                        self.drag_start_value =
                            self.params.gain.unmodulated_normalized_value();
                    }

                    let pixels_per_full_range = 600.0;
                    if shift_now {
                        let delta_y = self.granular_drag_start_y - self.mouse_y;
                        let delta_value = delta_y / pixels_per_full_range * 0.1;
                        let normalized = (self.granular_drag_start_value + delta_value).clamp(0.0, 1.0);
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, normalized);
                    } else {
                        let delta_y = self.drag_start_y - self.mouse_y;
                        let delta_value = delta_y / pixels_per_full_range;
                        let normalized = (self.drag_start_value + delta_value).clamp(0.0, 1.0);
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, normalized);
                    }

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
                    let is_double_click =
                        now.duration_since(self.last_click_time).as_millis() < 400
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
                                let current_value = self.params.gain.unmodulated_normalized_value();
                                self.drag_start_y = my;
                                self.drag_start_value = current_value;
                                self.granular_drag_start_y = my;
                                self.granular_drag_start_value = current_value;
                                self.last_shift_state = modifiers.contains(keyboard_types::Modifiers::SHIFT);
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
    /// Shared with GainBrainWindow so Editor::size() reflects runtime changes.
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(
    params: Arc<GainBrainParams>,
    display_gain_millibels: Arc<std::sync::atomic::AtomicI32>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(GainBrainEditor {
        params,
        display_gain_millibels,
        scaling_factor: Arc::new(AtomicCell::new(1.5)),
    }))
}

impl Editor for GainBrainEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let sf = self.scaling_factor.load();
        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let display_gain = Arc::clone(&self.display_gain_millibels);
        let shared_scale = Arc::clone(&self.scaling_factor);

        let scaled_w = (WINDOW_WIDTH as f32 * sf).round() as u32;
        let scaled_h = (WINDOW_HEIGHT as f32 * sf).round() as u32;
        self.params.editor_state.size.store((scaled_w, scaled_h));

        let window = baseview::Window::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Gain Brain"),
                size: baseview::Size::new(scaled_w as f64, scaled_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| GainBrainWindow::new(window, gui_context, params, display_gain, shared_scale, sf),
        );

        self.params
            .editor_state
            .open
            .store(true, Ordering::Release);
        Box::new(GainBrainEditorHandle {
            state: self.params.editor_state.clone(),
            window,
        })
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

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

struct GainBrainEditorHandle {
    state: Arc<GainBrainEditorState>,
    window: WindowHandle,
}

/// # Safety
///
/// The WindowHandle is created by baseview from the host-provided parent window
/// and is only used on the GUI thread. The `Send` bound is required by nih-plug's
/// `Editor::spawn` return type. This is the same pattern used by gs-meter and is
/// safe as long as the handle is not accessed from multiple threads simultaneously,
/// which nih-plug guarantees.
unsafe impl Send for GainBrainEditorHandle {}

impl Drop for GainBrainEditorHandle {
    fn drop(&mut self) {
        self.state.open.store(false, Ordering::Release);
        self.window.close();
    }
}

// ── Raw window handle adapters ──────────────────────────────────────────

struct ParentWindowHandleAdapter(nih_plug::editor::ParentWindowHandle);

unsafe impl HasRawWindowHandle for ParentWindowHandleAdapter {
    fn raw_window_handle(&self) -> RawWindowHandle {
        match self.0 {
            ParentWindowHandle::X11Window(window) => {
                let mut handle = raw_window_handle::XcbWindowHandle::empty();
                handle.window = window;
                RawWindowHandle::Xcb(handle)
            }
            ParentWindowHandle::AppKitNsView(ns_view) => {
                let mut handle = raw_window_handle::AppKitWindowHandle::empty();
                handle.ns_view = ns_view;
                RawWindowHandle::AppKit(handle)
            }
            ParentWindowHandle::Win32Hwnd(hwnd) => {
                let mut handle = raw_window_handle::Win32WindowHandle::empty();
                handle.hwnd = hwnd;
                RawWindowHandle::Win32(handle)
            }
        }
    }
}

#[derive(Clone)]
struct SoftbufferHandleAdapter {
    raw_display_handle: raw_window_handle_06::RawDisplayHandle,
    raw_window_handle: raw_window_handle_06::RawWindowHandle,
}

impl raw_window_handle_06::HasDisplayHandle for SoftbufferHandleAdapter {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle_06::DisplayHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::DisplayHandle::borrow_raw(
                self.raw_display_handle,
            ))
        }
    }
}

impl raw_window_handle_06::HasWindowHandle for SoftbufferHandleAdapter {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle_06::WindowHandle<'_>, raw_window_handle_06::HandleError> {
        unsafe {
            Ok(raw_window_handle_06::WindowHandle::borrow_raw(
                self.raw_window_handle,
            ))
        }
    }
}

fn baseview_window_to_surface_target(window: &baseview::Window<'_>) -> SoftbufferHandleAdapter {
    use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle};

    let raw_display = window.raw_display_handle();
    let raw_window = window.raw_window_handle();

    SoftbufferHandleAdapter {
        raw_display_handle: match raw_display {
            raw_window_handle::RawDisplayHandle::AppKit(_) => {
                raw_window_handle_06::RawDisplayHandle::AppKit(
                    raw_window_handle_06::AppKitDisplayHandle::new(),
                )
            }
            raw_window_handle::RawDisplayHandle::Xlib(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xlib(
                    raw_window_handle_06::XlibDisplayHandle::new(
                        NonNull::new(handle.display),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Xcb(handle) => {
                raw_window_handle_06::RawDisplayHandle::Xcb(
                    raw_window_handle_06::XcbDisplayHandle::new(
                        NonNull::new(handle.connection),
                        handle.screen,
                    ),
                )
            }
            raw_window_handle::RawDisplayHandle::Windows(_) => {
                raw_window_handle_06::RawDisplayHandle::Windows(
                    raw_window_handle_06::WindowsDisplayHandle::new(),
                )
            }
            _ => todo!("Unsupported display handle"),
        },
        raw_window_handle: match raw_window {
            raw_window_handle::RawWindowHandle::AppKit(handle) => {
                raw_window_handle_06::RawWindowHandle::AppKit(
                    raw_window_handle_06::AppKitWindowHandle::new(
                        NonNull::new(handle.ns_view).unwrap(),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Xlib(handle) => {
                raw_window_handle_06::RawWindowHandle::Xlib(
                    raw_window_handle_06::XlibWindowHandle::new(handle.window),
                )
            }
            raw_window_handle::RawWindowHandle::Xcb(handle) => {
                raw_window_handle_06::RawWindowHandle::Xcb(
                    raw_window_handle_06::XcbWindowHandle::new(
                        NonZeroU32::new(handle.window)
                            .expect("XCB window handle is 0 -- host provided invalid parent"),
                    ),
                )
            }
            raw_window_handle::RawWindowHandle::Win32(handle) => {
                let mut raw_handle = raw_window_handle_06::Win32WindowHandle::new(
                    NonZeroIsize::new(handle.hwnd as isize).unwrap(),
                );
                raw_handle.hinstance = NonZeroIsize::new(handle.hinstance as isize);
                raw_window_handle_06::RawWindowHandle::Win32(raw_handle)
            }
            _ => todo!("Unsupported window handle"),
        },
    }
}
