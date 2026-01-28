use nih_plug::prelude::*;
use std::sync::Arc;

mod wavetable;
mod editor;

use wavetable::Wavetable;

struct WavetableFilter {
    params: Arc<WavetableFilterParams>,
    wavetable: Option<Wavetable>,
    sample_rate: f32,
    // Circular buffer for convolution (per channel)
    filter_state: [FilterState; 2],
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
        Self {
            params: Arc::new(WavetableFilterParams::default()),
            wavetable: Some(Self::create_default_wavetable()),
            sample_rate: 44100.0,
            filter_state: [
                FilterState::new(2048),
                FilterState::new(2048),
            ],
        }
    }
}

impl WavetableFilter {
    /// Create a default lowpass filter wavetable
    fn create_default_wavetable() -> Wavetable {
        const FRAME_SIZE: usize = 512;
        const FRAME_COUNT: usize = 16;
        let mut samples = Vec::with_capacity(FRAME_SIZE * FRAME_COUNT);

        for frame_idx in 0..FRAME_COUNT {
            // Create progressively darker lowpass filters
            let cutoff = 1.0 - (frame_idx as f32 / FRAME_COUNT as f32) * 0.9;

            for i in 0..FRAME_SIZE {
                let freq = i as f32 / FRAME_SIZE as f32;

                // Simple lowpass filter kernel
                let response = if freq < cutoff {
                    1.0
                } else {
                    // Smooth rolloff
                    ((1.0 - (freq - cutoff) / (1.0 - cutoff)) * std::f32::consts::PI).cos() * 0.5 + 0.5
                };

                samples.push(response);
            }
        }

        Wavetable::new(samples, FRAME_SIZE).expect("Failed to create default wavetable")
    }

    pub fn load_wavetable_from_file(&mut self, path: &str) -> Result<(), String> {
        let wavetable = Wavetable::from_file(path)?;

        // Resize filter state if needed
        let new_size = wavetable.frame_size;
        for state in &mut self.filter_state {
            if state.history.len() != new_size {
                *state = FilterState::new(new_size);
            }
        }

        self.wavetable = Some(wavetable);
        Ok(())
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

impl Default for WavetableFilterParams {
    fn default() -> Self {
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

            frame_position: FloatParam::new(
                "Frame Position",
                0.0,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_smoother(SmoothingStyle::Linear(50.0)),

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
    const VENDOR: &'static str = "Your Name";
    const URL: &'static str = "https://github.com/yourusername/wavetable-filter";
    const EMAIL: &'static str = "your.email@example.com";
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

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        true
    }

    fn reset(&mut self) {
        for state in &mut self.filter_state {
            state.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // If no wavetable loaded, pass through the audio
        let Some(ref wavetable) = self.wavetable else {
            return ProcessStatus::Normal;
        };

        for (channel_idx, mut channel_samples) in buffer.iter_samples().enumerate() {
            let frame_pos = self.params.frame_position.smoothed.next();
            let mix = self.params.mix.smoothed.next();
            let drive = self.params.drive.smoothed.next();

            // Get mutable access to the filter state for this channel
            let state_idx = channel_idx.min(1);

            for sample in channel_samples.iter_mut() {
                let input = *sample;

                // Apply drive
                let driven_input = (input * drive).tanh();

                // Push input into history buffer
                self.filter_state[state_idx].push(driven_input);

                // Get the current filter kernel (wavetable frame)
                let filter_kernel = wavetable.get_frame_interpolated(frame_pos);
                let kernel_size = filter_kernel.len();

                // Perform convolution: output = sum(input[n-k] * kernel[k])
                let mut filtered = 0.0;
                for k in 0..kernel_size {
                    filtered += self.filter_state[state_idx].get(k) * filter_kernel[k];
                }

                // Normalize by kernel size to prevent volume buildup
                filtered /= kernel_size as f32;

                // Mix dry and wet signals
                let output = input * (1.0 - mix) + filtered * mix;

                *sample = output;
            }
        }

        ProcessStatus::Normal
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone())
    }
}

impl ClapPlugin for WavetableFilter {
    const CLAP_ID: &'static str = "com.yourname.wavetable-filter";
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
