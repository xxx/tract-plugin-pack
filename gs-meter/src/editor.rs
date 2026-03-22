//! Softbuffer-based editor for gs-meter. CPU rendering via tiny-skia, no GPU required.

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

use crate::widgets;

use crate::{GsMeterParams, MeterReadings};

const WINDOW_WIDTH: u32 = 420;
const WINDOW_HEIGHT: u32 = 540;

// ── Editor State (persisted by the host) ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct GsMeterEditorState {
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,
    #[serde(skip)]
    open: AtomicBool,
}

impl GsMeterEditorState {
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

impl<'a> PersistentField<'a, GsMeterEditorState> for Arc<GsMeterEditorState> {
    fn set(&self, new_value: GsMeterEditorState) {
        self.size.store(new_value.size.load());
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&GsMeterEditorState) -> R,
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
    Slider(ParamId),
    SteppedSegment { param: ParamId, index: i32 },
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Gain,
    Reference,
    RmsWindow,
    ChannelMode,
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    Reset,
    GainFromReading(GainSource),
}

#[derive(Clone, Copy, PartialEq)]
enum GainSource {
    PeakMax,
    TruePeak,
    RmsIntegrated,
    RmsMomentary,
    RmsMomentaryMax,
}

struct GsMeterWindow {
    gui_context: Arc<dyn GuiContext>,
    _sb_context: softbuffer::Context<SoftbufferHandleAdapter>,
    sb_surface: softbuffer::Surface<SoftbufferHandleAdapter, SoftbufferHandleAdapter>,
    pixmap: tiny_skia::Pixmap,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,

    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
    text_renderer: widgets::TextRenderer,

    /// Hit regions rebuilt each frame during draw().
    hit_regions: Vec<HitRegion>,
    /// Currently dragging a slider.
    drag_active: Option<ParamId>,
    /// Mouse position in physical pixels.
    mouse_x: f32,
    mouse_y: f32,
}

impl GsMeterWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<GsMeterParams>,
        readings: Arc<MeterReadings>,
        should_reset: Arc<AtomicBool>,
        scale_factor: f32,
    ) -> Self {
        let (uw, uh) = params.editor_state.size();
        let pw = (uw as f32 * scale_factor).round() as u32;
        let ph = (uh as f32 * scale_factor).round() as u32;

        let target = baseview_window_to_surface_target(window);
        let sb_context =
            softbuffer::Context::new(target.clone()).expect("could not get softbuffer context");
        let mut sb_surface =
            softbuffer::Surface::new(&sb_context, target).expect("could not create softbuffer surface");
        sb_surface
            .resize(NonZeroU32::new(pw).unwrap(), NonZeroU32::new(ph).unwrap())
            .unwrap();

        let pixmap = tiny_skia::Pixmap::new(pw, ph).expect("could not create pixmap");
        eprintln!("[INIT] logical={}x{}, scale={}, physical={}x{}", uw, uh, scale_factor, pw, ph);

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
            params,
            readings,
            should_reset,
            text_renderer,
            hit_regions: Vec::new(),
            drag_active: None,
            mouse_x: 0.0,
            mouse_y: 0.0,
        }
    }

    fn draw(&mut self) {
        let s = self.scale_factor;

        // Clear hit regions and background
        self.hit_regions.clear();
        self.pixmap.fill(widgets::color_bg());

        let pad = 20.0 * s;
        let row_h = 35.0 * s;
        let label_w = 100.0 * s;
        let font_size = 14.0 * s;
        let title_size = 24.0 * s;
        let slider_w = 200.0 * s;
        let slider_h = 26.0 * s;
        let value_w = 120.0 * s;
        let btn_w = 70.0 * s;
        let btn_h = 24.0 * s;
        let gap = 10.0 * s;

        let mut y = 15.0 * s;
        let tr = &mut self.text_renderer;

        // Title
        tr.draw_text(&mut self.pixmap, pad, y + title_size, "GS Meter", title_size, widgets::color_text());
        y += row_h;

        // Read meter values
        let peak_db = MeterReadings::load_db(&self.readings.peak_max_db);
        let true_peak_db = MeterReadings::load_db(&self.readings.true_peak_max_db);
        let rms_int_db = MeterReadings::load_db(&self.readings.rms_integrated_db);
        let rms_mom_db = MeterReadings::load_db(&self.readings.rms_momentary_db);
        let rms_max_db = MeterReadings::load_db(&self.readings.rms_momentary_max_db);
        let crest_db = MeterReadings::load_db(&self.readings.crest_factor_db);

        // Channel mode selector
        let mode_idx = match self.params.channel_mode.value() {
            crate::ChannelMode::Stereo => 0,
            crate::ChannelMode::Left => 1,
            crate::ChannelMode::Right => 2,
        };
        tr.draw_text(&mut self.pixmap, pad, y + font_size, "Channel", font_size, widgets::color_muted());
        let sel_x = pad + label_w;
        let sel_y = y + 4.0 * s;
        widgets::draw_stepped_selector(
            &mut self.pixmap, tr, sel_x, sel_y, slider_w, slider_h,
            &["Stereo", "Left", "Right"], mode_idx,
        );
        // Register hit regions for each segment
        let seg_w = slider_w / 3.0;
        for i in 0..3 {
            self.hit_regions.push(HitRegion {
                x: sel_x + i as f32 * seg_w, y: sel_y, w: seg_w, h: slider_h,
                action: HitAction::SteppedSegment { param: ParamId::ChannelMode, index: i },
            });
        }
        y += row_h;

        // Helper: draw a labeled slider and register its hit region
        macro_rules! slider_row {
            ($label:expr, $param:expr, $param_id:expr, $value_text:expr) => {
                tr.draw_text(&mut self.pixmap, pad, y + font_size, $label, font_size, widgets::color_muted());
                let sx = pad + label_w;
                let sy = y + 4.0 * s;
                widgets::draw_slider(
                    &mut self.pixmap, tr, sx, sy, slider_w, slider_h,
                    "", $value_text, $param.unmodulated_normalized_value(),
                );
                self.hit_regions.push(HitRegion {
                    x: sx, y: sy, w: slider_w, h: slider_h,
                    action: HitAction::Slider($param_id),
                });
                y += row_h;
            };
        }

        let gain_text = format!("{:.1} dB", nih_plug::util::gain_to_db(self.params.gain.value()));
        slider_row!("Gain", self.params.gain, ParamId::Gain, &gain_text);

        let ref_text = format!("{:.1} dB", self.params.reference_level.value());
        slider_row!("Reference", self.params.reference_level, ParamId::Reference, &ref_text);

        let window_text = format!("{:.0} ms", self.params.rms_window_ms.value());
        slider_row!("RMS Window", self.params.rms_window_ms, ParamId::RmsWindow, &window_text);

        // Readings header
        tr.draw_text(&mut self.pixmap, pad, y + font_size + 2.0 * s, "Readings", font_size * 1.1, widgets::color_text());
        y += 30.0 * s;

        // Meter rows with → Gain buttons
        let gain_sources = [
            ("Peak Max", peak_db, GainSource::PeakMax),
            ("True Peak", true_peak_db, GainSource::TruePeak),
            ("RMS (Int)", rms_int_db, GainSource::RmsIntegrated),
            ("RMS (Mom)", rms_mom_db, GainSource::RmsMomentary),
            ("RMS Max", rms_max_db, GainSource::RmsMomentaryMax),
        ];

        for &(label, db, source) in &gain_sources {
            let val = format_db(db);
            tr.draw_text(&mut self.pixmap, pad, y + font_size, label, font_size, widgets::color_muted());
            tr.draw_text(&mut self.pixmap, pad + label_w + gap, y + font_size, &val, font_size, widgets::color_text());
            let bx = pad + label_w + gap + value_w + gap;
            let by = y + 2.0 * s;
            widgets::draw_button(
                &mut self.pixmap, tr, bx, by, btn_w, btn_h,
                "\u{2192} Gain", false, false,
            );
            self.hit_regions.push(HitRegion {
                x: bx, y: by, w: btn_w, h: btn_h,
                action: HitAction::Button(ButtonAction::GainFromReading(source)),
            });
            y += row_h;
        }

        // Crest (no button)
        let crest_val = if crest_db <= -100.0 { "-- dB".to_string() } else { format!("{:.1} dB", crest_db) };
        tr.draw_text(&mut self.pixmap, pad, y + font_size, "Crest", font_size, widgets::color_muted());
        tr.draw_text(&mut self.pixmap, pad + label_w + gap, y + font_size, &crest_val, font_size, widgets::color_text());
        y += row_h;

        // Reset button
        let reset_x = pad;
        let reset_y = y + 2.0 * s;
        let reset_w = 100.0 * s;
        let reset_h = 28.0 * s;
        widgets::draw_button(&mut self.pixmap, tr, reset_x, reset_y, reset_w, reset_h, "Reset", false, false);
        self.hit_regions.push(HitRegion {
            x: reset_x, y: reset_y, w: reset_w, h: reset_h,
            action: HitAction::Button(ButtonAction::Reset),
        });
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.begin_set_parameter(&self.params.gain),
            ParamId::Reference => setter.begin_set_parameter(&self.params.reference_level),
            ParamId::RmsWindow => setter.begin_set_parameter(&self.params.rms_window_ms),
            ParamId::ChannelMode => setter.begin_set_parameter(&self.params.channel_mode),
        }
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        match id {
            ParamId::Gain => setter.set_parameter_normalized(&self.params.gain, normalized),
            ParamId::Reference => setter.set_parameter_normalized(&self.params.reference_level, normalized),
            ParamId::RmsWindow => setter.set_parameter_normalized(&self.params.rms_window_ms, normalized),
            ParamId::ChannelMode => setter.set_parameter_normalized(&self.params.channel_mode, normalized),
        }
    }

    fn set_param_stepped(&self, setter: &ParamSetter, id: ParamId, index: i32) {
        match id {
            ParamId::ChannelMode => {
                let mode = match index {
                    0 => crate::ChannelMode::Stereo,
                    1 => crate::ChannelMode::Left,
                    _ => crate::ChannelMode::Right,
                };
                setter.set_parameter(&self.params.channel_mode, mode);
            }
            _ => {}
        }
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        match id {
            ParamId::Gain => setter.end_set_parameter(&self.params.gain),
            ParamId::Reference => setter.end_set_parameter(&self.params.reference_level),
            ParamId::RmsWindow => setter.end_set_parameter(&self.params.rms_window_ms),
            ParamId::ChannelMode => setter.end_set_parameter(&self.params.channel_mode),
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

fn format_db(db: f32) -> String {
    if db <= -100.0 {
        "-inf dB".to_string()
    } else {
        format!("{:.1} dB", db)
    }
}

impl baseview::WindowHandler for GsMeterWindow {
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
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved { position, .. }) => {
                self.mouse_x = position.x as f32;
                self.mouse_y = position.y as f32;

                // Handle slider drag
                if let Some(param_id) = self.drag_active {
                    if let Some(region) = self.hit_regions.iter().find(|r| {
                        matches!(&r.action, HitAction::Slider(id) if *id == param_id)
                    }) {
                        let normalized = ((self.mouse_x - region.x) / region.w).clamp(0.0, 1.0);
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, normalized);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed { button: baseview::MouseButton::Left, .. }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

                // Find which hit region was clicked
                let hit = self.hit_regions.iter().find(|r| {
                    mx >= r.x && mx < r.x + r.w && my >= r.y && my < r.y + r.h
                }).cloned();

                if let Some(region) = hit {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    match region.action {
                        HitAction::Slider(param_id) => {
                            self.drag_active = Some(param_id);
                            let normalized = ((mx - region.x) / region.w).clamp(0.0, 1.0);
                            self.begin_set_param(&setter, param_id);
                            self.set_param_normalized(&setter, param_id, normalized);
                        }
                        HitAction::SteppedSegment { param, index } => {
                            self.begin_set_param(&setter, param);
                            self.set_param_stepped(&setter, param, index);
                            self.end_set_param(&setter, param);
                        }
                        HitAction::Button(ButtonAction::Reset) => {
                            self.should_reset.store(true, Ordering::Relaxed);
                        }
                        HitAction::Button(ButtonAction::GainFromReading(source)) => {
                            let meter_db = match source {
                                GainSource::PeakMax => MeterReadings::load_db(&self.readings.peak_max_db),
                                GainSource::TruePeak => MeterReadings::load_db(&self.readings.true_peak_max_db),
                                GainSource::RmsIntegrated => MeterReadings::load_db(&self.readings.rms_integrated_db),
                                GainSource::RmsMomentary => MeterReadings::load_db(&self.readings.rms_momentary_db),
                                GainSource::RmsMomentaryMax => MeterReadings::load_db(&self.readings.rms_momentary_max_db),
                            };
                            if meter_db > -100.0 {
                                let reference = self.params.reference_level.value();
                                let target_gain_db = reference - meter_db;
                                let target_gain_linear = nih_plug::util::db_to_gain(target_gain_db);
                                let normalized = self.params.gain.preview_normalized(target_gain_linear);
                                setter.begin_set_parameter(&self.params.gain);
                                setter.set_parameter_normalized(&self.params.gain, normalized);
                                setter.end_set_parameter(&self.params.gain);
                            }
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased { button: baseview::MouseButton::Left, .. }) => {
                if let Some(param_id) = self.drag_active.take() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, param_id);
                }
            }
            baseview::Event::Keyboard(kb_event) => {
                use keyboard_types::{Key, KeyState, Modifiers};
                if kb_event.state == KeyState::Down && kb_event.modifiers.contains(Modifiers::CONTROL) {
                    let old_scale = self.scale_factor;
                    match &kb_event.key {
                        Key::Character(c) if c == "=" || c == "+" => {
                            self.scale_factor = (self.scale_factor + 0.25).min(3.0);
                        }
                        Key::Character(c) if c == "-" => {
                            self.scale_factor = (self.scale_factor - 0.25).max(0.75);
                        }
                        _ => {}
                    }
                    if (self.scale_factor - old_scale).abs() > 0.01 {
                        let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
                        let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
                        // Update the editor state size so Editor::size() returns the new value
                        self.params.editor_state.size.store((new_w, new_h));
                        // Resize our child window
                        window.resize(baseview::Size::new(new_w as f64, new_h as f64));
                        // Ask the host/standalone to resize the parent container
                        self.gui_context.request_resize();
                    }
                }
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ──────────────────────────────────────────

pub(crate) struct GsMeterEditor {
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
    scaling_factor: AtomicCell<Option<f32>>,
}

pub(crate) fn create(
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(GsMeterEditor {
        params,
        readings,
        should_reset,
        #[cfg(target_os = "macos")]
        scaling_factor: AtomicCell::new(None),
        #[cfg(not(target_os = "macos"))]
        scaling_factor: AtomicCell::new(Some(1.5)),
    }))
}

impl Editor for GsMeterEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (uw, uh) = self.params.editor_state.size();
        let scaling_factor = self.scaling_factor.load();
        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let readings = Arc::clone(&self.readings);
        let should_reset = Arc::clone(&self.should_reset);

        let window = baseview::Window::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("GS Meter"),
                // Open at the scaled size directly (scale factor controls rendering, not DPI)
                size: {
                    let sf = scaling_factor.unwrap_or(1.0) as f64;
                    baseview::Size::new(uw as f64 * sf, uh as f64 * sf)
                },
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                GsMeterWindow::new(
                    window,
                    gui_context,
                    params,
                    readings,
                    should_reset,
                    scaling_factor.unwrap_or(1.0),
                )
            },
        );

        self.params.editor_state.open.store(true, Ordering::Release);
        Box::new(GsMeterEditorHandle {
            state: self.params.editor_state.clone(),
            window,
        })
    }

    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(Some(factor));
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

struct GsMeterEditorHandle {
    state: Arc<GsMeterEditorState>,
    window: WindowHandle,
}

unsafe impl Send for GsMeterEditorHandle {}

impl Drop for GsMeterEditorHandle {
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

fn baseview_window_to_surface_target(
    window: &baseview::Window<'_>,
) -> SoftbufferHandleAdapter {
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
                        NonZeroU32::new(handle.window).unwrap(),
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
