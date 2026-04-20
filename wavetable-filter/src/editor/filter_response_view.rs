//! Filter response + input spectrum visualization. Rewritten in Task 9.

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;

pub(crate) struct FftCache {
    pub planner: RealFftPlanner<f32>,
    pub frame_buf: Vec<f32>,
    pub spectrum: Vec<Complex<f32>>,
    pub cached_mags: Vec<f32>,
    pub cached_frame_pos: f32,
    pub cached_cutoff: f32,
    pub cached_resonance: f32,
    pub freq_table: Vec<f32>,
    pub freq_table_size: usize,
    pub cached_response_ys: Vec<f32>,
    pub cached_input_mags: Vec<f32>,
    pub cached_input_sr: f32,
}

impl FftCache {
    pub fn new() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            frame_buf: Vec::new(),
            spectrum: Vec::new(),
            cached_mags: Vec::new(),
            cached_frame_pos: -1.0,
            cached_cutoff: -1.0,
            cached_resonance: -1.0,
            freq_table: Vec::new(),
            freq_table_size: 0,
            cached_response_ys: Vec::new(),
            cached_input_mags: Vec::new(),
            cached_input_sr: 0.0,
        }
    }
}
