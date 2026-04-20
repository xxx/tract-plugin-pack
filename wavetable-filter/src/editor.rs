use std::sync::Arc;
use nih_plug::prelude::Editor;
pub use tiny_skia_widgets::EditorState;

pub const WINDOW_WIDTH: u32 = 900;
pub const WINDOW_HEIGHT: u32 = 640;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    _params: Arc<crate::WavetableFilterParams>,
    _should_reload: Arc<std::sync::atomic::AtomicBool>,
    _pending_reload: Arc<std::sync::Mutex<Option<crate::PendingReload>>>,
    _shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    _wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    _shared_input_spectrum: Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    None
}
