//! Native validation harness for the shared logical/physical editor window.

use baseview::{Event, EventStatus, MouseEvent, Window, WindowHandler};
use nih_plug::prelude::PluginApi;
use std::sync::{Arc, atomic::AtomicU64};
use tiny_skia_widgets as widgets;

struct Probe {
    surface: widgets::SoftbufferSurface,
    text: widgets::TextRenderer,
    state: Arc<widgets::EditorState>,
    state_path: Option<std::path::PathBuf>,
    scale: f32,
    pointer: (f32, f32),
    clicks: usize,
    frames: usize,
    pending: Arc<AtomicU64>,
    backing_scale: f64,
}

impl widgets::PhysicalEditor for Probe {
    fn prepare_resize(&mut self, _: &mut Window<'_>, _: f32) {}
}

impl WindowHandler for Probe {
    fn on_frame(&mut self, _window: &mut Window<'_>) {
        self.frames += 1;
        if let Ok(limit) = std::env::var("DPI_PROBE_EXIT_AFTER")
            && let Ok(limit) = limit.parse::<usize>()
            && self.frames >= limit
        {
            std::process::exit(0);
        }
        #[cfg(target_os = "macos")]
        if self.frames == 1 {
            use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
            if let (Ok(x), Ok(y)) = (
                std::env::var("DPI_PROBE_LEFT"),
                std::env::var("DPI_PROBE_TOP"),
            ) && let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>())
                && let RawWindowHandle::AppKit(handle) = _window.raw_window_handle()
            {
                // SAFETY: the probe is executing on the live baseview window's GUI thread.
                if let Some(view) =
                    unsafe { handle.ns_view.cast::<objc2_app_kit::NSView>().as_ref() }
                    && let Some(window) = view.window()
                {
                    let mut point = window.frame().origin;
                    point.x = x;
                    point.y = y;
                    window.setFrameTopLeftPoint(point);
                }
            }
        }
        if self.frames == 2
            && let Ok(size) = std::env::var("DPI_PROBE_RESIZE")
            && let Some((width, height)) = size.split_once(',')
            && let (Ok(width), Ok(height)) = (width.parse::<f64>(), height.parse::<f64>())
        {
            let width = (width * self.backing_scale).round() as u64;
            let height = (height * self.backing_scale).round() as u64;
            self.pending
                .store((width << 32) | height, std::sync::atomic::Ordering::Relaxed);
        }
        self.surface.pixmap.fill(widgets::color_bg());
        let s = self.scale;
        widgets::draw_rect(
            &mut self.surface.pixmap,
            20.0 * s,
            60.0 * s,
            120.0 * s,
            60.0 * s,
            widgets::color_accent(),
        );
        self.text.draw_text(
            &mut self.surface.pixmap,
            20.0 * s,
            30.0 * s,
            "Editor DPI probe",
            18.0 * s,
            widgets::color_text(),
        );
        self.text.draw_text(
            &mut self.surface.pixmap,
            30.0 * s,
            96.0 * s,
            &format!("Clicks {}", self.clicks),
            16.0 * s,
            widgets::color_bg(),
        );
        self.text.draw_text(
            &mut self.surface.pixmap,
            20.0 * s,
            155.0 * s,
            &format!("Logical size {:?}", self.state.size()),
            14.0 * s,
            widgets::color_text(),
        );
        self.surface.present();
    }
    fn on_event(&mut self, _: &mut Window<'_>, event: Event) -> EventStatus {
        match event {
            Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.backing_scale = info.scale();
                self.scale = widgets::editor_scale(&info, 420);
                self.surface
                    .resize(info.physical_size().width, info.physical_size().height);
                eprintln!(
                    "DPI logical={:?} physical={:?} backing={} persisted={:?}",
                    info.logical_size(),
                    info.physical_size(),
                    info.scale(),
                    self.state.size()
                );
                if let Some(path) = &self.state_path {
                    let result = serde_json::to_vec(self.state.as_ref())
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| std::fs::write(path, bytes).map_err(|e| e.to_string()));
                    if let Err(error) = result {
                        eprintln!("STATE error={error}");
                    }
                }
            }
            Event::Mouse(MouseEvent::CursorMoved { position, .. }) => {
                self.pointer = (position.x as f32, position.y as f32);
                eprintln!("MOVE physical=({},{})", position.x, position.y);
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                let (x, y) = self.pointer;
                let s = self.scale;
                eprintln!("PRESS physical=({x},{y}) scale={s}");
                if (20.0 * s..140.0 * s).contains(&x) && (60.0 * s..120.0 * s).contains(&y) {
                    self.clicks += 1;
                    eprintln!("CLICK count={} physical=({x},{y})", self.clicks);
                }
            }
            _ => {}
        }
        EventStatus::Captured
    }
}

fn main() {
    let state_path = std::env::args().nth(1).map(std::path::PathBuf::from);
    let state = state_path
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|data| serde_json::from_slice::<widgets::EditorState>(&data).ok())
        .map(Arc::new)
        .unwrap_or_else(|| widgets::EditorState::from_size(420, 280));
    let (width, height) = state.size();
    let options = baseview::WindowOpenOptions {
        title: "Editor DPI probe".into(),
        size: baseview::Size::new(width as f64, height as f64),
        scale: baseview::WindowScalePolicy::ScaleFactor(1.0),
        gl_config: None,
    };
    baseview::Window::open_blocking(options, move |window| {
        let pending = Arc::new(AtomicU64::new(0));
        let inner = Probe {
            surface: widgets::SoftbufferSurface::new(window, width, height),
            text: widgets::TextRenderer::new(widgets::DEFAULT_FONT),
            state: state.clone(),
            state_path,
            scale: 1.0,
            pointer: (0.0, 0.0),
            clicks: 0,
            frames: 0,
            pending: pending.clone(),
            backing_scale: 1.0,
        };
        widgets::EditorWindow::new(inner, state, pending, PluginApi::Standalone)
    });
}
