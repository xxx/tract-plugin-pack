//! Editor stub. Real GUI lives in this file plus `editor/*.rs` later.

use std::sync::Arc;
use tiny_skia_widgets as widgets;

pub use widgets::EditorState;

const WINDOW_WIDTH: u32 = 720;
const WINDOW_HEIGHT: u32 = 500;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}
