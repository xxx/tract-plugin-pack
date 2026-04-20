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

// ── Constants and helpers ───────────────────────────────────────────────

const TOP_STRIP_H: f32 = 32.0;
const STRIP_PAD: f32 = 8.0;

fn format_wavetable_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("(none)")
        .to_string()
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

    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Frame => &self.params.frame_position,
            ParamId::Frequency => &self.params.frequency,
            ParamId::Resonance => &self.params.resonance,
            ParamId::Drive => &self.params.drive,
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

    fn format_value(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        p.normalized_value_to_string(p.modulated_normalized_value(), true)
    }

    fn formatted_value_without_unit(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        p.normalized_value_to_string(p.modulated_normalized_value(), false)
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
        let Some(norm) = p.string_to_normalized_value(&text) else {
            return;
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set_param(&setter, param_id);
        self.set_param_normalized(&setter, param_id, norm);
        self.end_set_param(&setter, param_id);
    }

    fn set_mode(&self, variant: u8) {
        use crate::FilterMode;
        use nih_plug::prelude::Param;
        let target = match variant {
            0 => FilterMode::Raw,
            _ => FilterMode::Minimum,
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let norm = self.params.mode.preview_normalized(target);
        setter.begin_set_parameter(&self.params.mode);
        setter.set_parameter_normalized(&self.params.mode, norm);
        setter.end_set_parameter(&self.params.mode);
    }

    fn open_file_dialog(&mut self) {
        use nih_plug::nih_log;

        let mut dialog = rfd::FileDialog::new().add_filter("Wavetable files", &["wav", "wt"]);
        if let Ok(current) = self.params.wavetable_path.lock() {
            if let Some(dir) = std::path::Path::new(current.as_str()).parent() {
                if dir.exists() {
                    dialog = dialog.set_directory(dir);
                }
            }
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let Some(path_str) = path.to_str() else { return };
        let path_string = path_str.to_string();

        let new_wavetable = match Wavetable::from_file(&path_string) {
            Ok(wt) => wt,
            Err(e) => {
                nih_log!("Wavetable load error: {e}");
                return;
            }
        };

        // Pre-allocate FFT scratch on the GUI thread — audio thread stays allocation-free.
        let new_size = new_wavetable.frame_size;
        let spec_len = new_size / 2 + 1;
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(new_size);
        let reload = PendingReload {
            wavetable: new_wavetable.clone(),
            frame_fft,
            frame_cache: vec![0.0; new_size],
            frame_buf: vec![0.0; new_size],
            frame_spectrum: vec![rustfft::num_complex::Complex::new(0.0, 0.0); spec_len],
            frame_mags: vec![0.0; spec_len],
        };

        if let Ok(mut pending) = self.pending_reload.lock() {
            *pending = Some(reload);
        }
        if let Ok(mut shared) = self.shared_wavetable.lock() {
            *shared = new_wavetable;
        }
        self.wavetable_version.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut wt) = self.params.wavetable_path.lock() {
            *wt = path_string.clone();
        }
        self.should_reload.store(true, Ordering::Relaxed);
    }

    fn draw(&mut self) {
        let s = self.scale_factor;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let w = self.physical_width as f32;

        // ── Top strip: Browse | path | Mode selector ──
        let strip_y = 0.0;
        let strip_h = TOP_STRIP_H * s;
        let pad = STRIP_PAD * s;

        let browse_w = 72.0 * s;
        let browse_h = 22.0 * s;
        let browse_x = pad;
        let browse_y = strip_y + (strip_h - browse_h) * 0.5;

        widgets::draw_button(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            browse_x,
            browse_y,
            browse_w,
            browse_h,
            "Browse",
            false,
            false,
        );
        self.drag.push_region(
            browse_x,
            browse_y,
            browse_w,
            browse_h,
            HitAction::Button(ButtonAction::Browse),
        );

        // Mode selector (right-aligned)
        let mode_w = 160.0 * s;
        let mode_h = 22.0 * s;
        let mode_x = w - pad - mode_w;
        let mode_y = strip_y + (strip_h - mode_h) * 0.5;
        let active_idx = if self.params.mode.value() == crate::FilterMode::Raw {
            0
        } else {
            1
        };
        let segments = ["Raw", "Phaseless"];
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mode_x,
            mode_y,
            mode_w,
            mode_h,
            &segments,
            active_idx,
        );
        let seg_w = mode_w / segments.len() as f32;
        for i in 0..segments.len() as u8 {
            self.drag.push_region(
                mode_x + seg_w * i as f32,
                mode_y,
                seg_w,
                mode_h,
                HitAction::Button(ButtonAction::Mode(i)),
            );
        }

        // Path label between Browse and Mode selector
        let path_x = browse_x + browse_w + pad;
        let path_right = mode_x - pad;
        let path_w = (path_right - path_x).max(0.0);
        if path_w > 10.0 {
            let path_text = self
                .params
                .wavetable_path
                .lock()
                .map(|p| format_wavetable_label(&p))
                .unwrap_or_else(|_| "(locked)".to_string());
            let text_size = 13.0 * s;
            let text_y = strip_y + (strip_h + text_size) * 0.5 - 3.0 * s;
            self.text_renderer.draw_text(
                &mut self.surface.pixmap,
                path_x,
                text_y,
                &path_text,
                text_size,
                widgets::color_text(),
            );
        }

        // Bottom rule under the strip
        widgets::draw_rect(
            &mut self.surface.pixmap,
            0.0,
            strip_h - 1.0,
            w,
            1.0,
            widgets::color_border(),
        );

        let h = self.physical_height as f32;

        // ── Dial geometry ──
        let dial_row_h = 60.0 * s;
        let dial_radius = 22.0 * s;

        // Lower dial row: sits at the bottom of the window
        let dial_row_y = h - dial_row_h;

        // Frame dial takes the left half; Freq/Res/Drive/Mix share the right half
        let left_w = w * 0.5;
        let right_w = w - left_w;

        // Frame dial, centered in left half
        self.draw_dial(
            ParamId::Frame,
            "Frame",
            left_w * 0.5,
            dial_row_y + dial_row_h * 0.5,
            dial_radius,
        );

        // Right-side dials: 4 evenly spaced
        let right_dials: [(ParamId, &str); 4] = [
            (ParamId::Frequency, "Freq"),
            (ParamId::Resonance, "Res"),
            (ParamId::Drive, "Drive"),
            (ParamId::Mix, "Mix"),
        ];
        let spacing = right_w / right_dials.len() as f32;
        for (i, &(pid, label)) in right_dials.iter().enumerate() {
            let cx = left_w + spacing * (i as f32 + 0.5);
            let cy = dial_row_y + dial_row_h * 0.5;
            self.draw_dial(pid, label, cx, cy, dial_radius);
        }
    }

    fn draw_dial(&mut self, param_id: ParamId, label: &str, cx: f32, cy: f32, radius: f32) {
        use nih_plug::prelude::Param;
        let p = self.float_param(param_id);
        let unmod = p.unmodulated_normalized_value();
        let modulated = p.modulated_normalized_value();
        let value_text = self.format_value(param_id);

        let editing_buf: Option<String> = self
            .text_edit
            .active_for(&HitAction::Dial(param_id))
            .map(str::to_owned);
        let caret = self.text_edit.caret_visible();

        // Hit region is the bounding square around the dial plus label/value area.
        let hit_w = radius * 3.2;
        let hit_h = radius * 3.2;
        self.drag.push_region(
            cx - hit_w * 0.5,
            cy - hit_h * 0.5,
            hit_w,
            hit_h,
            HitAction::Dial(param_id),
        );

        widgets::draw_dial_ex(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            cx,
            cy,
            radius,
            label,
            &value_text,
            unmod,
            Some(modulated),
            editing_buf.as_deref(),
            caret,
        );
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
