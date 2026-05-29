//! Editor stub — replaced in Task 9.
use std::sync::Arc;

use nih_plug::prelude::*;
use tiny_skia_widgets as widgets;

pub use widgets::EditorState;

use crate::{handoff::SequenceHandoff, NapParams};

const WINDOW_WIDTH: u32 = 560;
const WINDOW_HEIGHT: u32 = 720;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

pub fn create(
    _params: Arc<NapParams>,
    _handoff: Arc<SequenceHandoff>,
    _sample_rate: f32,
) -> Option<Box<dyn Editor>> {
    None
}
