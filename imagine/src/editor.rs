//! Softbuffer-based editor for Imagine. CPU rendering via tiny-skia.
//!
//! Layout B:
//! ```text
//! +-----------------------------------------------------------+
//! |                          |   spectrum view (top right)    |
//! |                          +--------------------------------+
//! |     vectorscope panel    |   per-band strip (mid right)   |
//! |     (left ~40%)          +--------------------------------+
//! |                          |   coherence spectrum (bot rt)  |
//! +-----------------------------------------------------------+
//! | global strip: Recover Sides / Link / Quality              |
//! +-----------------------------------------------------------+
//! ```
//!
//! Mouse handling and text-edit are wired in Task 17. This skeleton just
//! manages the softbuffer + baseview lifecycle and dispatches the layout
//! rectangles to per-section view modules (filled in by Tasks 13-16).

mod band_strip;
mod global_strip;
mod spectrum_view;
mod vectorscope_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::vectorscope::VectorConsumer;
use crate::ImagineParams;
use tiny_skia_widgets as widgets;

pub const WINDOW_WIDTH: u32 = 960;
pub const WINDOW_HEIGHT: u32 = 640;
pub const MIN_WIDTH: u32 = 720;
pub const MIN_HEIGHT: u32 = 580;

/// Bottom strip height in logical pixels (scaled by `scale_factor`).
const BOTTOM_STRIP_H: f32 = 36.0;

// ── Window handler ──────────────────────────────────────────────────────

struct ImagineWindow {
    _gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    pending_resize: Arc<AtomicU64>,

    params: Arc<ImagineParams>,
    vectorscope: Arc<VectorConsumer>,

    /// Scratch buffers reused each frame so the vectorscope drain doesn't
    /// allocate per-frame. The audio thread fills the SPSC ring; the editor
    /// drains into these on the GUI thread.
    vec_l: Vec<f32>,
    vec_r: Vec<f32>,
}

impl ImagineWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<ImagineParams>,
        vectorscope: Arc<VectorConsumer>,
        pending_resize: Arc<AtomicU64>,
        physical_width: u32,
        physical_height: u32,
        scale_factor: f32,
    ) -> Self {
        let surface = widgets::SoftbufferSurface::new(window, physical_width, physical_height);

        Self {
            _gui_context: gui_context,
            surface,
            physical_width,
            physical_height,
            scale_factor,
            pending_resize,
            params,
            vectorscope,
            vec_l: Vec::with_capacity(2048),
            vec_r: Vec::with_capacity(2048),
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    fn draw(&mut self) {
        let s = self.scale_factor;
        let w = self.physical_width as i32;
        let h = self.physical_height as i32;

        // Clear background.
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, crate::theme::bg());

        // Bottom strip occupies a fixed-height (logical) row at the bottom.
        let bottom_h = (BOTTOM_STRIP_H * s).round() as i32;
        let main_h = (h - bottom_h).max(1);

        // Layout B: left vectorscope column (~40%) + right column with three
        // stacked sub-panels.
        let left_w = ((w as f32) * 0.4).round() as i32;
        let right_x = left_w;
        let right_w = (w - left_w).max(1);

        let spectrum_h = ((main_h as f32) * 0.55).round() as i32;
        let band_h = ((main_h as f32) * 0.30).round() as i32;
        let coherence_h = (main_h - spectrum_h - band_h).max(1);

        let spectrum_y = 0;
        let band_y = spectrum_y + spectrum_h;
        let coherence_y = band_y + band_h;

        let bottom_y = main_h;

        // Dispatch to view modules. Each is a no-op stub today but will own
        // its own region by Task 13-16.
        let mut pm = self.surface.pixmap.as_mut();
        vectorscope_view::draw(
            &mut pm,
            0,
            0,
            left_w,
            main_h,
            &self.params,
            &self.vectorscope,
            &mut self.vec_l,
            &mut self.vec_r,
        );
        spectrum_view::draw(
            &mut pm,
            right_x,
            spectrum_y,
            right_w,
            spectrum_h,
            &self.params,
        );
        band_strip::draw(&mut pm, right_x, band_y, right_w, band_h, &self.params);
        spectrum_view::draw_coherence(
            &mut pm,
            right_x,
            coherence_y,
            right_w,
            coherence_h,
            &self.params,
        );
        global_strip::draw(&mut pm, 0, bottom_y, w, bottom_h, &self.params);
    }
}

impl baseview::WindowHandler for ImagineWindow {
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
        if let baseview::Event::Window(baseview::WindowEvent::Resized(info)) = &event {
            self.physical_width = info.physical_size().width;
            self.physical_height = info.physical_size().height;
            let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
            self.scale_factor = sf;
            self.resize_buffers();
        }
        // Mouse handling deferred to Task 17.
        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

pub struct ImagineEditor {
    pub params: Arc<ImagineParams>,
    pub vectorscope: Arc<VectorConsumer>,
    pub pending_resize: Arc<AtomicU64>,
}

impl Editor for ImagineEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let vectorscope = Arc::clone(&self.vectorscope);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Imagine"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                ImagineWindow::new(
                    window,
                    gui_context,
                    params,
                    vectorscope,
                    pending_resize,
                    persisted_w,
                    persisted_h,
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

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_dimensions_within_default() {
        assert!(MIN_WIDTH <= WINDOW_WIDTH);
        assert!(MIN_HEIGHT <= WINDOW_HEIGHT);
    }

    #[test]
    fn pending_resize_packs_round_trip() {
        let r = Arc::new(AtomicU64::new(0));
        let editor = ImagineEditor {
            params: Arc::new(ImagineParams::default()),
            vectorscope: Arc::new(crate::vectorscope::ring_pair().1),
            pending_resize: Arc::clone(&r),
        };
        assert!(editor.set_size(1024, 768));
        let packed = r.load(Ordering::Relaxed);
        assert_eq!((packed >> 32) as u32, 1024);
        assert_eq!((packed & 0xFFFF_FFFF) as u32, 768);
    }

    #[test]
    fn set_size_rejects_zero() {
        let r = Arc::new(AtomicU64::new(0));
        let editor = ImagineEditor {
            params: Arc::new(ImagineParams::default()),
            vectorscope: Arc::new(crate::vectorscope::ring_pair().1),
            pending_resize: Arc::clone(&r),
        };
        assert!(!editor.set_size(0, 768));
        assert!(!editor.set_size(1024, 0));
        assert_eq!(r.load(Ordering::Relaxed), 0);
    }
}
