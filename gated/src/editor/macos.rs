//! Query the native view directly because baseview 0.1.1 misses macOS resize notifications.

use objc2_app_kit::NSView;
use raw_window_handle::{HasRawWindowHandle, RawWindowHandle};

pub(super) fn window_info(window: &baseview::Window<'_>) -> Option<baseview::WindowInfo> {
    let RawWindowHandle::AppKit(handle) = window.raw_window_handle() else {
        return None;
    };
    // SAFETY: baseview owns this NSView for the window's lifetime; callers run on its GUI thread.
    let view = unsafe { handle.ns_view.cast::<NSView>().as_ref() }?;
    let bounds = view.bounds();
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return None;
    }
    let scale = view
        .window()
        .map_or(1.0, |window| window.backingScaleFactor());
    Some(baseview::WindowInfo::from_logical_size(
        baseview::Size::new(bounds.size.width, bounds.size.height),
        scale,
    ))
}
