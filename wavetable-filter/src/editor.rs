//! Softbuffer-based editor for Wavetable Filter. CPU rendering via tiny-skia.
//!
//! Layout (900x640, freely resizable):
//! - Top strip (~32px): Browse button + wavetable name + mode stepped selector
//! - Main area: wavetable view (left) + filter response view (right)
//! - Dials below each view: Frame | Frequency, Resonance, Drive, Mix

pub mod filter_response_view;
pub mod wavetable_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia_widgets as widgets;

use crate::wavetable::Wavetable;
use crate::{PendingReload, WavetableFilterParams};

pub const WINDOW_WIDTH: u32 = 900;
pub const WINDOW_HEIGHT: u32 = 640;
const MIN_WIDTH: u32 = 700;
const MIN_HEIGHT: u32 = 500;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ParamId {
    Frame,
    Frequency,
    Resonance,
    Drive,
    Mix,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ButtonAction {
    Browse,
    WavetableToggle2D3D,
    /// 0 = Raw, 1 = Phaseless. Matches the EnumParam::variants() order.
    Mode(u8),
}

// ── Window handler ──────────────────────────────────────────────────────

struct WavetableFilterWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<WavetableFilterParams>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    // Audio → GUI plumbing
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,

    // View-local state
    show_2d: bool,
    frame_cache: wavetable_view::FrameCache,
    fft_cache: filter_response_view::FftCache,
}

impl WavetableFilterWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<WavetableFilterParams>,
        should_reload: Arc<AtomicBool>,
        pending_reload: Arc<Mutex<Option<PendingReload>>>,
        shared_wavetable: Arc<Mutex<Wavetable>>,
        wavetable_version: Arc<AtomicU32>,
        shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
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
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            should_reload,
            pending_reload,
            shared_wavetable,
            wavetable_version,
            shared_input_spectrum,
            show_2d: false,
            frame_cache: wavetable_view::FrameCache::new(),
            fft_cache: filter_response_view::FftCache::new(),
        }
    }

    fn draw(&mut self) {
        // Full-frame clear; layout comes in Task 5.
        self.surface.pixmap.fill(widgets::color_bg());
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for WavetableFilterWindow {
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
                self.physical_width = info.physical_size().width.max(MIN_WIDTH);
                self.physical_height = info.physical_size().height.max(MIN_HEIGHT);
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

pub(crate) struct WavetableFilterEditor {
    params: Arc<WavetableFilterParams>,
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
    scaling_factor: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    params: Arc<WavetableFilterParams>,
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(WavetableFilterEditor {
        params,
        should_reload,
        pending_reload,
        shared_wavetable,
        wavetable_version,
        shared_input_spectrum,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for WavetableFilterEditor {
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
        let should_reload = Arc::clone(&self.should_reload);
        let pending_reload = Arc::clone(&self.pending_reload);
        let shared_wavetable = Arc::clone(&self.shared_wavetable);
        let wavetable_version = Arc::clone(&self.wavetable_version);
        let shared_input_spectrum = Arc::clone(&self.shared_input_spectrum);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Wavetable Filter"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                WavetableFilterWindow::new(
                    window,
                    gui_context,
                    params,
                    should_reload,
                    pending_reload,
                    shared_wavetable,
                    wavetable_version,
                    shared_input_spectrum,
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
