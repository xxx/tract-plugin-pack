//! Wavetable visualization (2D face-on / 3D overhead). Rewritten in Task 8.

#[allow(dead_code)]
pub(crate) struct FrameCache {
    pub cached_frames: Vec<Vec<f32>>,
    pub cached_version: u32,
    pub cached_frame_count: usize,
    pub cached_frame_size: usize,
    pub global_min: f32,
    pub global_max: f32,
}

impl FrameCache {
    pub fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX,
            cached_frame_count: 0,
            cached_frame_size: 0,
            global_min: 0.0,
            global_max: 0.0,
        }
    }
}
