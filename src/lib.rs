#![feature(portable_simd)]

use nih_plug::prelude::*;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::simd::f32x16;

pub mod wavetable;
mod editor;

use wavetable::Wavetable;

pub struct WavetableFilter {
    params: Arc<WavetableFilterParams>,
    wavetable: Option<Wavetable>,
    sample_rate: f32,
    // Circular buffer for convolution (per channel)
    filter_state: [FilterState; 2],
    // Shared state for wavetable path
    wavetable_path: Arc<Mutex<String>>,
    should_reload: Arc<AtomicBool>,
    // Shared wavetable for UI display
    shared_wavetable: Arc<Mutex<Wavetable>>,
    // Version counter to trigger UI updates
    wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    // Current frame count for parameter display
    current_frame_count: Arc<std::sync::atomic::AtomicUsize>,
    // Silence detection counter
    silence_samples: usize,
    // Cached filter kernel
    current_kernel: Vec<f32>,
    last_frame_pos: f32,
}

struct FilterState {
    // Circular buffer for input history (size = max wavetable frame size)
    history: Vec<f32>,
    write_pos: usize,
}

#[derive(Params)]
struct WavetableFilterParams {
    #[id = "frequency"]
    pub frequency: FloatParam,

    #[id = "frame_position"]
    pub frame_position: FloatParam,

    #[id = "mix"]
    pub mix: FloatParam,

    #[id = "drive"]
    pub drive: FloatParam,
}

impl Default for WavetableFilter {
    fn default() -> Self {
        let default_wt = Self::create_default_wavetable();
        let frame_count = default_wt.frame_count;
        let frame_size = default_wt.frame_size;

        let current_frame_count = Arc::new(std::sync::atomic::AtomicUsize::new(frame_count));

        Self {
            params: Arc::new(WavetableFilterParams::new(current_frame_count.clone())),
            wavetable: Some(default_wt.clone()),
            sample_rate: 44100.0,
            filter_state: [
                FilterState::new(2048),
                FilterState::new(2048),
            ],
            wavetable_path: Arc::new(Mutex::new(String::from("Default"))),
            should_reload: Arc::new(AtomicBool::new(false)),
            shared_wavetable: Arc::new(Mutex::new(default_wt)),
            wavetable_version: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            current_frame_count,
            silence_samples: 0,
            current_kernel: vec![0.0; frame_size],
            last_frame_pos: -1.0,
        }
    }
}

impl WavetableFilter {
    /// Create a default lowpass filter wavetable
    pub fn create_default_wavetable() -> Wavetable {
        const FRAME_SIZE: usize = 256;  // Smaller for more aggressive filtering
        const FRAME_COUNT: usize = 16;
        let mut samples = Vec::with_capacity(FRAME_SIZE * FRAME_COUNT);

        for frame_idx in 0..FRAME_COUNT {
            let mut frame_samples = Vec::with_capacity(FRAME_SIZE);
            let mut sum = 0.0;

            // Create different filter types across frames
            let filter_type = frame_idx as f32 / (FRAME_COUNT - 1) as f32;

            if filter_type < 0.33 {
                // Frames 0-5: Aggressive lowpass (simple moving average)
                let window_size = (FRAME_SIZE as f32 * (0.05 + filter_type * 0.15)) as usize;
                for i in 0..FRAME_SIZE {
                    let value = if i < window_size {
                        1.0
                    } else {
                        0.0
                    };
                    frame_samples.push(value);
                    sum += value;
                }
            } else if filter_type < 0.66 {
                // Frames 6-10: Bandpass (two peaks)
                let center = FRAME_SIZE / 2;
                let width = (20.0 + (filter_type - 0.33) * 60.0) as usize;
                for i in 0..FRAME_SIZE {
                    let dist = (i as i32 - center as i32).abs() as usize;
                    let value = if dist < width {
                        (1.0 - (dist as f32 / width as f32)).max(0.0)
                    } else {
                        0.0
                    };
                    frame_samples.push(value);
                    sum += value;
                }
            } else {
                // Frames 11-15: Highpass (invert lowpass)
                let window_size = (FRAME_SIZE as f32 * 0.05) as usize;
                for i in 0..FRAME_SIZE {
                    let value = if i == 0 {
                        1.0  // DC component
                    } else if i < window_size {
                        -0.8 / window_size as f32  // Negative for highpass
                    } else {
                        0.0
                    };
                    frame_samples.push(value);
                    sum += value.abs();  // Use abs for normalization
                }
            }

            // Normalize the filter kernel
            let normalization = if sum.abs() > 0.0001 { 1.0 / sum } else { 1.0 };
            for sample in &mut frame_samples {
                *sample *= normalization;
            }

            samples.extend(frame_samples);
        }

        Wavetable::new(samples, FRAME_SIZE).expect("Failed to create default wavetable")
    }

    pub fn load_wavetable_from_file(&mut self, path: &str) -> Result<(), String> {
        let wavetable = Wavetable::from_file(path)?;

        nih_log!("Loaded wavetable: {} frames, {} samples per frame", wavetable.frame_count, wavetable.frame_size);

        // Resize filter state if needed
        let new_size = wavetable.frame_size;
        for state in &mut self.filter_state {
            if state.history.len() != new_size.max(2048) {
                *state = FilterState::new(new_size.max(2048));
            }
        }

        // Update frame count for parameter display
        self.current_frame_count.store(wavetable.frame_count, Ordering::Relaxed);

        self.wavetable = Some(wavetable.clone());

        // Update shared wavetable for UI
        if let Ok(mut shared_wt) = self.shared_wavetable.lock() {
            *shared_wt = wavetable;
        }

        // Increment version to trigger UI redraw
        let new_version = self.wavetable_version.fetch_add(1, Ordering::Relaxed) + 1;
        nih_log!("Updated wavetable version to {}", new_version);

        if let Ok(mut path_lock) = self.wavetable_path.lock() {
            *path_lock = path.to_string();
        }
        Ok(())
    }

    pub fn set_wavetable_path(&self, path: String) {
        if let Ok(mut path_lock) = self.wavetable_path.lock() {
            *path_lock = path;
        }
        self.should_reload.store(true, Ordering::Relaxed);
    }

    /// Try to load a wavetable from environment variable or default location
    pub fn try_load_user_wavetable(&mut self) {
        // First, try environment variable WAVETABLE_FILTER_PATH
        if let Ok(path) = std::env::var("WAVETABLE_FILTER_PATH") {
            if std::path::Path::new(&path).exists() {
                if let Ok(_) = self.load_wavetable_from_file(&path) {
                    return;
                }
            }
        }

        // Fall back to ~/wavetable-filter/wavetable.wav or wavetable.wt
        if let Some(home) = std::env::var_os("HOME") {
            let base_path = std::path::Path::new(&home).join("wavetable-filter");

            for ext in &["wav", "wt"] {
                let path = base_path.join(format!("wavetable.{}", ext));
                if path.exists() {
                    if let Ok(_) = self.load_wavetable_from_file(path.to_str().unwrap()) {
                        return;
                    }
                }
            }
        }
    }
}

impl FilterState {
    fn new(size: usize) -> Self {
        Self {
            history: vec![0.0; size],
            write_pos: 0,
        }
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.write_pos = 0;
    }

    fn push(&mut self, sample: f32) {
        self.history[self.write_pos] = sample;
        self.write_pos = (self.write_pos + 1) % self.history.len();
    }

    fn get(&self, offset: usize) -> f32 {
        let idx = (self.write_pos + self.history.len() - offset - 1) % self.history.len();
        self.history[idx]
    }
}

impl WavetableFilterParams {
    fn new(frame_count: Arc<std::sync::atomic::AtomicUsize>) -> Self {
        Self {
            frequency: FloatParam::new(
                "Frequency",
                1000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" Hz"),

            frame_position: {
                let frame_count_clone = frame_count.clone();
                FloatParam::new(
                    "Frame Position",
                    0.0,
                    FloatRange::Linear { min: 0.0, max: 1.0 },
                )
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_value_to_string(Arc::new(move |value| {
                    let count = frame_count.load(std::sync::atomic::Ordering::Relaxed);
                    let frame_num = (value * (count - 1) as f32).round() as usize + 1;
                    format!("{}", frame_num)
                }))
                .with_string_to_value(Arc::new(move |string| {
                    let count = frame_count_clone.load(std::sync::atomic::Ordering::Relaxed);
                    string.parse::<f32>().ok().map(|frame| (frame - 1.0) / (count - 1).max(1) as f32)
                }))
            },

            mix: FloatParam::new(
                "Mix",
                1.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit("%")
            .with_value_to_string(formatters::v2s_f32_percentage(0)),

            drive: FloatParam::new(
                "Drive",
                1.0,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0)),
        }
    }
}

impl Plugin for WavetableFilter {
    const NAME: &'static str = "Wavetable Filter";
    const VENDOR: &'static str = "Michael Dungan";
    const URL: &'static str = "https://github.com/xxx/wavetable-filter";
    const EMAIL: &'static str = "no-reply@example.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    // Tail time in seconds - max wavetable frame size at low frequencies
    // 2048 samples at 44.1kHz = ~46ms, but give it more headroom
    const HARD_REALTIME_ONLY: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.wavetable_path.clone(),
            self.should_reload.clone(),
            self.shared_wavetable.clone(),
            self.wavetable_version.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;

        // Try to load a user wavetable on initialization
        self.try_load_user_wavetable();

        true
    }

    fn reset(&mut self) {
        for state in &mut self.filter_state {
            state.reset();
        }
        self.silence_samples = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Check if we should reload the wavetable
        if self.should_reload.load(Ordering::Relaxed) {
            // Copy from shared_wavetable (loaded by GUI thread) to audio thread's wavetable
            if let Ok(shared_wt) = self.shared_wavetable.lock() {
                // Update frame count for parameter display
                self.current_frame_count.store(shared_wt.frame_count, Ordering::Relaxed);

                self.wavetable = Some(shared_wt.clone());

                // Resize filter state if needed
                let new_size = shared_wt.frame_size;
                for state in &mut self.filter_state {
                    if state.history.len() != new_size.max(2048) {
                        *state = FilterState::new(new_size.max(2048));
                    }
                }

                // Resize kernel buffer to match new frame size
                self.current_kernel.resize(new_size, 0.0);
                self.last_frame_pos = -1.0;
            }
            self.should_reload.store(false, Ordering::Relaxed);
        }

        // If no wavetable loaded, pass through the audio
        let Some(ref wavetable) = self.wavetable else {
            return ProcessStatus::Normal;
        };

        // Check if input is silent (to clear filter state when playback stops)
        let silence_threshold = 1e-6f32;
        let mut is_silent = true;

        let num_samples = buffer.samples();
        let num_channels = buffer.channels();

        // Get raw pointers for each channel
        let channel_ptrs: Vec<(*const f32, *mut f32)> = buffer
            .as_slice()
            .iter()
            .map(|slice| (slice.as_ptr(), slice.as_ptr() as *mut f32))
            .collect();

        // Process each sample across all channels
        for sample_idx in 0..num_samples {
            // Get smoothed parameters once per sample (not once per channel!)
            let frame_pos = self.params.frame_position.smoothed.next();
            let mix = self.params.mix.smoothed.next();
            let drive = self.params.drive.smoothed.next();

            // Update kernel only when frame position changes enough to warrant a new interpolation
            // Use a smaller threshold to track parameter changes more closely
            let needs_update = (frame_pos - self.last_frame_pos).abs() > 0.0001 || self.last_frame_pos < 0.0;

            if needs_update {
                self.current_kernel = wavetable.get_frame_interpolated(frame_pos);
                self.last_frame_pos = frame_pos;
            }

            let kernel_size = self.current_kernel.len();

            // Process each channel for this sample
            for channel_idx in 0..num_channels {
                let state_idx = channel_idx.min(1);

                // Safety: we know sample_idx and channel_idx are in bounds
                let (input_ptr, output_ptr) = channel_ptrs[channel_idx];
                let input = unsafe { *input_ptr.add(sample_idx) };

                // Check if this sample is above silence threshold
                if input.abs() > silence_threshold {
                    is_silent = false;
                }

                // Apply drive
                let driven_input = (input * drive).tanh();

                // Push input into history buffer
                self.filter_state[state_idx].push(driven_input);

                // Perform convolution with SIMD: output = sum(input[n-k] * kernel[k])
                let mut filtered = 0.0;
                let simd_lanes = 16;
                let simd_chunks = kernel_size / simd_lanes;

                // Process 16 samples at a time with SIMD (512-bit AVX-512 or 256-bit AVX2)
                let mut acc = f32x16::splat(0.0);
                for chunk_idx in 0..simd_chunks {
                    let k = chunk_idx * simd_lanes;

                    // Load 16 history samples
                    let mut history = [0.0f32; 16];
                    for i in 0..16 {
                        history[i] = self.filter_state[state_idx].get(k + i);
                    }
                    let history_vec = f32x16::from_array(history);

                    // Load 16 kernel coefficients
                    let kernel_slice = &self.current_kernel[k..k + 16];
                    let kernel_vec = f32x16::from_slice(kernel_slice);

                    // Multiply and accumulate
                    acc += history_vec * kernel_vec;
                }

                // Sum the SIMD accumulator
                let acc_array = acc.to_array();
                for val in acc_array {
                    filtered += val;
                }

                // Handle remaining samples
                for k in (simd_chunks * simd_lanes)..kernel_size {
                    filtered += self.filter_state[state_idx].get(k) * self.current_kernel[k];
                }

                // Mix dry and wet signals
                let output = input * (1.0 - mix) + filtered * mix;

                // Write output
                unsafe { *output_ptr.add(sample_idx) = output };
            }
        }

        // Track silence duration and clear filter state if silent for too long
        if is_silent {
            self.silence_samples += buffer.samples();
            // Clear after ~100ms of silence (assuming 44.1kHz)
            if self.silence_samples > (self.sample_rate * 0.1) as usize {
                for state in &mut self.filter_state {
                    state.reset();
                }
            }
        } else {
            self.silence_samples = 0;
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for WavetableFilter {
    const CLAP_ID: &'static str = "com.mpd.wavetable-filter";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A wavetable-based filter plugin");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Filter,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for WavetableFilter {
    const VST3_CLASS_ID: [u8; 16] = *b"WavetableFilter1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Filter,
    ];
}

nih_export_clap!(WavetableFilter);
nih_export_vst3!(WavetableFilter);
