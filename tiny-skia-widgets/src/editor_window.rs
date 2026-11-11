//! Logical native windows around physical-pixel CPU renderers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use baseview::{Event, EventStatus, MouseEvent, Size, Window, WindowHandler, WindowInfo};
use nih_plug::prelude::PluginApi;

use crate::EditorState;

/// A physical-pixel renderer hosted by [`EditorWindow`].
pub trait PhysicalEditor: WindowHandler {
    /// Finish/cancel pointer gestures, close popups, and update input sensitivity.
    /// Called before receiving the new physical `Resized` event.
    fn prepare_resize(&mut self, window: &mut Window<'_>, backing_scale: f32);
}

/// Owns DPI conversion, native geometry, pending host resizes, and logical persistence.
/// The child receives physical pointer positions and physical `WindowInfo` sizes;
/// wheel detents and keyboard events retain their original semantics.
pub struct EditorWindow<H: PhysicalEditor> {
    inner: H,
    state: Arc<EditorState>,
    pending: Arc<AtomicU64>,
    api: PluginApi,
    info: WindowInfo,
    initialized: bool,
    pointer: Option<MouseEvent>,
    pointer_inside: bool,
}

impl<H: PhysicalEditor> EditorWindow<H> {
    /// Wrap an editor once in the native window's builder closure.
    pub fn new(inner: H, state: Arc<EditorState>, pending: Arc<AtomicU64>, api: PluginApi) -> Self {
        let (width, height) = state.size();
        Self {
            inner,
            state,
            pending,
            api,
            info: WindowInfo::from_logical_size(Size::new(width as f64, height as f64), 1.0),
            initialized: false,
            pointer: None,
            pointer_inside: false,
        }
    }

    fn changed(&self, info: WindowInfo) -> bool {
        !self.initialized
            || self.info.logical_size() != info.logical_size()
            || self.info.physical_size() != info.physical_size()
            || self.info.scale() != info.scale()
    }

    fn persist(&self) {
        let size = self.info.logical_size();
        self.state.store_size(
            (size.width.round() as u32).max(1),
            (size.height.round() as u32).max(1),
        );
    }

    fn physical_event(&self, mut event: Event) -> Event {
        if let Event::Mouse(MouseEvent::CursorMoved { position, .. }) = &mut event {
            position.x *= self.info.scale();
            position.y *= self.info.scale();
        }
        event
    }

    fn take_resize(&self, info: WindowInfo, physical: bool) -> Option<Size> {
        let packed = self.pending.swap(0, Ordering::Relaxed);
        let (width, height) = ((packed >> 32) as u32, packed as u32);
        if width == 0 || height == 0 {
            return None;
        }
        if physical && info.physical_size() == baseview::PhySize::new(width, height) {
            return None;
        }
        let requested = if physical {
            baseview::PhySize::new(width, height).to_logical(&info)
        } else {
            Size::new(width as f64, height as f64)
        };
        (requested != info.logical_size()).then_some(requested)
    }

    fn synchronize(&mut self, window: &mut Window<'_>, info: WindowInfo) {
        if !self.changed(info) {
            return;
        }
        self.inner.prepare_resize(window, info.scale() as f32);
        self.info = info;
        self.initialized = true;
        self.persist();
        self.inner
            .on_event(window, Event::Window(baseview::WindowEvent::Resized(info)));
        // Rebuild hit regions before a click can arrive after a DPI/size change.
        self.inner.on_frame(window);
        if let Some(pointer) = self.pointer.clone() {
            let event = self.physical_event(Event::Mouse(pointer));
            self.inner.on_event(window, event);
            if !self.pointer_inside {
                self.inner
                    .on_event(window, Event::Mouse(MouseEvent::CursorLeft));
            }
        }
    }
}

impl<H: PhysicalEditor> WindowHandler for EditorWindow<H> {
    fn on_frame(&mut self, window: &mut Window<'_>) {
        let info = native_window_info(window).unwrap_or(self.info);
        let physical = !cfg!(target_os = "macos") || self.api == PluginApi::Standalone;
        if let Some(size) = self.take_resize(info, physical) {
            window.resize(size);
        }
        let info = native_window_info(window).unwrap_or(info);
        self.synchronize(window, info);
        self.inner.on_frame(window);
    }

    fn on_event(&mut self, window: &mut Window<'_>, event: Event) -> EventStatus {
        let info = match &event {
            Event::Window(baseview::WindowEvent::Resized(info)) => Some(*info),
            _ => None,
        };
        if let Some(info) = native_window_info(window).or(info) {
            self.synchronize(window, info);
        }
        if matches!(event, Event::Window(baseview::WindowEvent::Resized(_))) {
            return EventStatus::Captured;
        }
        match &event {
            Event::Mouse(mouse @ MouseEvent::CursorMoved { .. }) => {
                self.pointer = Some(mouse.clone());
                self.pointer_inside = true;
            }
            Event::Mouse(MouseEvent::CursorEntered) => self.pointer_inside = true,
            Event::Mouse(MouseEvent::CursorLeft) => self.pointer_inside = false,
            _ => {}
        }
        let event = self.physical_event(event);
        self.inner.on_event(window, event)
    }
}

/// Drawing scale: clamp user zoom in logical units, then apply backing DPI.
pub fn editor_scale(info: &WindowInfo, base_width: u32) -> f32 {
    (info.logical_size().width as f32 / base_width as f32).clamp(0.5, 4.0) * info.scale() as f32
}

/// Read native geometry even when baseview misses a macOS frame/backing notification.
#[cfg(target_os = "macos")]
pub fn native_window_info(window: &Window<'_>) -> Option<WindowInfo> {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};
    let RawWindowHandle::AppKit(handle) = window.raw_window_handle() else {
        return None;
    };
    // SAFETY: baseview owns this view for the window's lifetime; callbacks run on its GUI thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() }?;
    let bounds = view.bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return None;
    }
    let scale = view.window()?.backingScaleFactor();
    Some(WindowInfo::from_logical_size(
        Size::new(bounds.size.width, bounds.size.height),
        scale,
    ))
}

/// Other platforms deliver native size changes through baseview events.
#[cfg(not(target_os = "macos"))]
pub fn native_window_info(_window: &Window<'_>) -> Option<WindowInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Probe;
    impl WindowHandler for Probe {
        fn on_frame(&mut self, _: &mut Window<'_>) {}
        fn on_event(&mut self, _: &mut Window<'_>, _: Event) -> EventStatus {
            EventStatus::Ignored
        }
    }
    impl PhysicalEditor for Probe {
        fn prepare_resize(&mut self, _: &mut Window<'_>, _: f32) {}
    }
    fn editor() -> EditorWindow<Probe> {
        EditorWindow::new(
            Probe,
            EditorState::from_size(480, 840),
            Arc::new(AtomicU64::new(0)),
            PluginApi::Standalone,
        )
    }
    #[test]
    fn logical_size_survives_reopening_at_different_backing_scales() {
        let mut window = editor();
        for scale in [2.0, 2.0, 1.5, 1.0, 2.0] {
            window.info = WindowInfo::from_logical_size(Size::new(501.0, 841.0), scale);
            window.persist();
            let serialized = serde_json::to_string(window.state.as_ref()).unwrap();
            let state: EditorState = serde_json::from_str(&serialized).unwrap();
            assert_eq!(state.size(), (501, 841));
            window = EditorWindow::new(
                Probe,
                Arc::new(state),
                Arc::new(AtomicU64::new(0)),
                PluginApi::Standalone,
            );
            assert_eq!(window.info.logical_size(), Size::new(501.0, 841.0));
        }
    }
    #[test]
    fn host_requests_distinguish_physical_pixels_from_logical_points() {
        let window = editor();
        let info = WindowInfo::from_logical_size(Size::new(480.0, 840.0), 2.0);
        window
            .pending
            .store((960_u64 << 32) | 1680, Ordering::Relaxed);
        assert_eq!(window.take_resize(info, true), None);
        window
            .pending
            .store((480_u64 << 32) | 840, Ordering::Relaxed);
        assert_eq!(window.take_resize(info, false), None);
        window
            .pending
            .store((1200_u64 << 32) | 1800, Ordering::Relaxed);
        assert_eq!(
            window.take_resize(info, true),
            Some(Size::new(600.0, 900.0))
        );
        let fractional = WindowInfo::from_logical_size(Size::new(501.0, 841.0), 1.5);
        window
            .pending
            .store((752_u64 << 32) | 1262, Ordering::Relaxed);
        assert_eq!(window.take_resize(fractional, true), None);
        assert_eq!(window.pending.load(Ordering::Relaxed), 0);
    }
    #[test]
    fn pointer_events_scale_but_wheel_detents_do_not() {
        let mut window = editor();
        window.info = WindowInfo::from_logical_size(Size::new(480.0, 840.0), 2.0);
        let event = Event::Mouse(MouseEvent::CursorMoved {
            position: baseview::Point::new(49.0, 71.5),
            modifiers: Default::default(),
        });
        let Event::Mouse(MouseEvent::CursorMoved { position, .. }) = window.physical_event(event)
        else {
            panic!()
        };
        assert_eq!(position, baseview::Point::new(98.0, 143.0));
        let wheel = Event::Mouse(MouseEvent::WheelScrolled {
            delta: baseview::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            modifiers: Default::default(),
        });
        assert!(matches!(
            window.physical_event(wheel),
            Event::Mouse(MouseEvent::WheelScrolled {
                delta: baseview::ScrollDelta::Lines { y: 1.0, .. },
                ..
            })
        ));
    }
    #[test]
    fn initial_sync_and_large_retina_zoom_are_preserved() {
        let window = editor();
        assert!(window.changed(window.info));
        let info = WindowInfo::from_logical_size(Size::new(1440.0, 2000.0), 2.0);
        assert_eq!(editor_scale(&info, 480), 6.0);
    }
}
