//! Softbuffer-based editor for satch. CPU rendering via tiny-skia.

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
use crate::SatchParams;

const WINDOW_WIDTH: u32 = 300;
const WINDOW_HEIGHT: u32 = 380;

// ── Editor State (persisted by the host) ────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct SatchEditorState {
    #[serde(with = "nih_plug::params::persist::serialize_atomic_cell")]
    size: AtomicCell<(u32, u32)>,
    #[serde(skip)]
    open: AtomicBool,
}

impl SatchEditorState {
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

impl<'a> PersistentField<'a, SatchEditorState> for Arc<SatchEditorState> {
    fn set(&self, new_value: SatchEditorState) {
        let sz = new_value.size.load();
        nih_plug::nih_log!("[satch] PersistentField::set() size=({}, {})", sz.0, sz.1);
        self.size.store(sz);
    }

    fn map<F, R>(&self, f: F) -> R
    where
        F: Fn(&SatchEditorState) -> R,
    {
        f(self)
    }
}

// ── Hit testing ─────────────────────────────────────────────────────────

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
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq)]
enum ParamId {
    Gain,
    Threshold,
    Knee,
    Detail,
    Mix,
}

#[derive(Clone, Copy, PartialEq)]
enum ButtonAction {
    ScaleDown,
    ScaleUp,
}

// ── Window handler ──────────────────────────────────────────────────────

struct SatchWindow {
    gui_context: Arc<dyn GuiContext>,
    _sb_context: softbuffer::Context<SoftbufferHandleAdapter>,
    sb_surface: softbuffer::Surface<SoftbufferHandleAdapter, SoftbufferHandleAdapter>,
    pixmap: tiny_skia::Pixmap,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Shared with SatchEditor so Editor::size() stays in sync.
    shared_scale: Arc<AtomicCell<f32>>,

    params: Arc<SatchParams>,
    text_renderer: widgets::TextRenderer,

    /// Hit regions rebuilt each frame during draw().
    hit_regions: Vec<HitRegion>,
    /// Currently dragging a dial.
    drag_active: Option<HitAction>,
    /// Y coordinate where the current drag started.
    drag_start_y: f32,
    /// Normalized value (0.0–1.0) when the current drag started.
    drag_start_value: f32,
    /// Shift state from the last mouse event.
    last_shift_state: bool,
    /// When shift is pressed mid-drag, snapshot current Y and normalized value.
    granular_drag_start_y: f32,
    granular_drag_start_value: f32,
    /// Mouse position in physical pixels.
    mouse_x: f32,
    mouse_y: f32,
    /// Timestamp of last click for double-click detection.
    last_click_time: std::time::Instant,
    last_click_action: Option<HitAction>,
}

impl SatchWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<SatchParams>,
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

        self.hit_regions.clear();
        self.pixmap.fill(widgets::color_bg());

        let pad = 20.0 * s;
        let title_size = 20.0 * s;
        let small_font = 11.0 * s;
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
        let pct_text = format!("{}%", (self.scale_factor * 100.0).round() as u32);

        let tr = &mut self.text_renderer;

        // ── Title row with scale controls on the right ──
        tr.draw_text(
            &mut self.pixmap,
            pad,
            y + title_size,
            "satch",
            title_size,
            widgets::color_text(),
        );

        let scale_btn_size = 22.0 * s;
        let scale_label_w = 44.0 * s;

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
            widgets::draw_dial(
                &mut self.pixmap, tr, cx, row1_cy, dial_radius,
                label, value_text, *normalized,
            );
            self.hit_regions.push(HitRegion {
                x: cx - col_spacing / 2.0, y: y, w: col_spacing, h: row_h,
                action: HitAction::Dial(*param_id),
            });
        }

        // Row 2: Detail, Knee
        for (i, (param_id, label, normalized, value_text)) in row2.iter().enumerate() {
            let cx = pad + col_spacing * (i as f32 + 0.5);
            widgets::draw_dial(
                &mut self.pixmap, tr, cx, row2_cy, dial_radius,
                label, value_text, *normalized,
            );
            self.hit_regions.push(HitRegion {
                x: cx - col_spacing / 2.0, y: y + row_h, w: col_spacing, h: row_h,
                action: HitAction::Dial(*param_id),
            });
        }

        // Mix: bottom-right corner, smaller
        {
            let (param_id, label, normalized, ref value_text) = mix_data;
            let mix_radius = 22.0 * s;
            let mix_cx = w - pad - mix_radius - 10.0 * s;
            let mix_cy = y + row_h * 2.5;
            widgets::draw_dial(
                &mut self.pixmap, tr, mix_cx, mix_cy, mix_radius,
                label, value_text, normalized,
            );
            let hit_w = col_spacing;
            self.hit_regions.push(HitRegion {
                x: mix_cx - hit_w / 2.0, y: y + row_h * 2.0, w: hit_w, h: row_h,
                action: HitAction::Dial(param_id),
            });
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

    fn apply_scale_change(&mut self, delta: f32, window: &mut baseview::Window) {
        let old = self.scale_factor;
        self.scale_factor = (self.scale_factor + delta).clamp(0.75, 3.0);
        if (self.scale_factor - old).abs() > 0.01 {
            self.shared_scale.store(self.scale_factor);
            let new_w = (WINDOW_WIDTH as f32 * self.scale_factor).round() as u32;
            let new_h = (WINDOW_HEIGHT as f32 * self.scale_factor).round() as u32;
            self.params.editor_state.size.store((new_w, new_h));
            nih_plug::nih_log!("[satch] apply_scale_change() sf={:.2} stored=({}, {})", self.scale_factor, new_w, new_h);
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
        nih_plug::nih_log!("[satch] resize_buffers() pw={} ph={} sf={:.2} storing=({}, {})", pw, ph, self.scale_factor, pw, ph);
        self.params.editor_state.size.store((
            pw,
            ph,
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

impl baseview::WindowHandler for SatchWindow {
    fn on_frame(&mut self, _window: &mut baseview::Window) {
        self.draw();
        self.present();
    }

    fn on_event(
        &mut self,
        window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
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

                    // Get the current normalized value for detecting shift transitions
                    let current_norm =
                        self.float_param(param_id).unmodulated_normalized_value();
                    if shift_now && !self.last_shift_state {
                        self.granular_drag_start_y = self.mouse_y;
                        self.granular_drag_start_value = current_norm;
                    } else if !shift_now && self.last_shift_state {
                        self.drag_start_y = self.mouse_y;
                        self.drag_start_value = current_norm;
                    }

                    // 600px = full normalized range (0..1), up = increase
                    let target_norm = if shift_now {
                        let delta_y = self.granular_drag_start_y - self.mouse_y;
                        (self.granular_drag_start_value + delta_y / 600.0 * 0.1).clamp(0.0, 1.0)
                    } else {
                        let delta_y = self.drag_start_y - self.mouse_y;
                        (self.drag_start_value + delta_y / 600.0).clamp(0.0, 1.0)
                    };

                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.set_param_normalized(&setter, param_id, target_norm);

                    self.last_shift_state = shift_now;
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                let mx = self.mouse_x;
                let my = self.mouse_y;

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
                                let norm = self
                                    .float_param(param_id)
                                    .unmodulated_normalized_value();
                                self.drag_start_y = my;
                                self.drag_start_value = norm;
                                self.granular_drag_start_y = my;
                                self.granular_drag_start_value = norm;
                                self.last_shift_state = modifiers
                                    .contains(keyboard_types::Modifiers::SHIFT);
                                self.drag_active = Some(HitAction::Dial(param_id));
                                self.begin_set_param(&setter, param_id);
                            }
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

pub(crate) struct SatchEditor {
    params: Arc<SatchParams>,
    /// Shared with SatchWindow so Editor::size() reflects runtime changes.
    scaling_factor: Arc<AtomicCell<f32>>,
}

pub(crate) fn create(params: Arc<SatchParams>) -> Option<Box<dyn Editor>> {
    // NOTE: persisted state may not be restored yet (host calls create() before set()).
    // Scale factor is derived from persisted size in spawn() instead.
    Some(Box::new(SatchEditor {
        params,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
    }))
}

impl Editor for SatchEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        // Derive scale factor from persisted size (restored by host before spawn).
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.75, 3.0);
        self.scaling_factor.store(sf);
        nih_plug::nih_log!("[satch] spawn() persisted=({}, {}) sf={:.2}", persisted_w, persisted_h, sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let shared_scale = Arc::clone(&self.scaling_factor);

        let scaled_w = persisted_w;
        let scaled_h = persisted_h;

        let window = baseview::Window::open_parented(
            &ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("satch"),
                size: baseview::Size::new(scaled_w as f64, scaled_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| SatchWindow::new(window, gui_context, params, shared_scale, sf),
        );

        self.params
            .editor_state
            .open
            .store(true, Ordering::Release);
        Box::new(SatchEditorHandle {
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

struct SatchEditorHandle {
    state: Arc<SatchEditorState>,
    window: WindowHandle,
}

/// # Safety
///
/// The WindowHandle is created by baseview from the host-provided parent window
/// and is only used on the GUI thread. The `Send` bound is required by nih-plug's
/// `Editor::spawn` return type. This is the same pattern used by gain-brain and is
/// safe as long as the handle is not accessed from multiple threads simultaneously,
/// which nih-plug guarantees.
unsafe impl Send for SatchEditorHandle {}

impl Drop for SatchEditorHandle {
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
