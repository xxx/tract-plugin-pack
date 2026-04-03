use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

pub mod editor;
pub mod spectral;

// ── Constants ────────────────────────────────────────────────────────────────

pub const FFT_SIZE: usize = 4096;
pub const HOP_SIZE: usize = 1024;

/// Number of frequency bins in the display (downsampled from FFT).
pub const DISPLAY_BINS: usize = 128;
/// Number of time columns in the waterfall history.
pub const DISPLAY_COLUMNS: usize = 256;

// ── Spectral display ────────────────────────────────────────────────────────

/// Shared spectrogram data: audio thread writes columns, GUI reads them.
/// Uses atomic storage for lock-free sharing. Some tearing is acceptable
/// for visualization.
pub struct SpectralDisplay {
    /// Flat array of DISPLAY_COLUMNS * DISPLAY_BINS magnitude values,
    /// stored as f32 bit patterns in AtomicU32.
    data: Box<[AtomicU32]>,
    /// Current write position (column index, wraps at DISPLAY_COLUMNS).
    pub write_pos: AtomicUsize,
}

impl SpectralDisplay {
    pub fn new() -> Self {
        let size = DISPLAY_COLUMNS * DISPLAY_BINS;
        let mut data = Vec::with_capacity(size);
        for _ in 0..size {
            data.push(AtomicU32::new(0));
        }
        Self {
            data: data.into_boxed_slice(),
            write_pos: AtomicUsize::new(0),
        }
    }

    /// Write a column of magnitude data (called from audio thread).
    /// `magnitudes` should be DISPLAY_BINS long.
    pub fn push_column(&self, magnitudes: &[f32; DISPLAY_BINS]) {
        let col = self.write_pos.load(Ordering::Relaxed);
        let base = col * DISPLAY_BINS;
        for (i, &mag) in magnitudes.iter().enumerate() {
            self.data[base + i].store(mag.to_bits(), Ordering::Relaxed);
        }
        self.write_pos
            .store((col + 1) % DISPLAY_COLUMNS, Ordering::Relaxed);
    }

    /// Read a magnitude value at (column, bin).
    pub fn read(&self, column: usize, bin: usize) -> f32 {
        let idx = column * DISPLAY_BINS + bin;
        f32::from_bits(self.data[idx].load(Ordering::Relaxed))
    }
}

impl Default for SpectralDisplay {
    fn default() -> Self {
        Self::new()
    }
}

// ── Plugin struct ──────────────────────────────────────────────────────────────

pub struct WarpZone {
    params: Arc<WarpZoneParams>,
    display: Arc<SpectralDisplay>,
    shifter_l: spectral::SpectralShifter,
    shifter_r: spectral::SpectralShifter,
    // Feedback: previous wet output fed back into input
    feedback_l: f32,
    feedback_r: f32,
    // Dry signal delay to align with spectral latency
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
    sample_rate: f32,
    // Counter for display column updates
    display_counter: usize,
    // Pre-computed bin boundaries for downsample_magnitudes (avoids exp() per hop)
    display_bin_ranges: [(usize, usize); DISPLAY_BINS],
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct WarpZoneParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    #[id = "shift"]
    pub shift: FloatParam,

    #[id = "stretch"]
    pub stretch: FloatParam,

    #[id = "mix"]
    pub mix: FloatParam,

    #[id = "low-freq"]
    pub low_freq: FloatParam,

    #[id = "high-freq"]
    pub high_freq: FloatParam,

    #[id = "freeze"]
    pub freeze: BoolParam,

    #[id = "feedback"]
    pub feedback: FloatParam,
}

impl Default for WarpZone {
    fn default() -> Self {
        Self {
            params: Arc::new(WarpZoneParams::new()),
            display: Arc::new(SpectralDisplay::new()),
            shifter_l: spectral::SpectralShifter::new(FFT_SIZE, HOP_SIZE),
            shifter_r: spectral::SpectralShifter::new(FFT_SIZE, HOP_SIZE),
            feedback_l: 0.0,
            feedback_r: 0.0,
            dry_delay_l: vec![0.0; FFT_SIZE],
            dry_delay_r: vec![0.0; FFT_SIZE],
            dry_delay_pos: 0,
            sample_rate: 48000.0,
            display_counter: 0,
            display_bin_ranges: compute_display_bin_ranges(FFT_SIZE / 2 + 1),
        }
    }
}

impl WarpZoneParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            shift: FloatParam::new(
                "Shift",
                0.0,
                FloatRange::Linear {
                    min: -24.0,
                    max: 24.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" st")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),

            stretch: FloatParam::new(
                "Stretch",
                1.0,
                FloatRange::Linear {
                    min: 0.5,
                    max: 2.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit("x")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            mix: FloatParam::new(
                "Mix",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            low_freq: FloatParam::new(
                "Low Freq",
                20.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            high_freq: FloatParam::new(
                "High Freq",
                20000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            freeze: BoolParam::new("Freeze", false),

            feedback: FloatParam::new(
                "Feedback",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
        }
    }
}

// ── Downsample helper ─────────────────────────────────────────────────────────

/// Pre-compute logarithmically-spaced bin boundaries for display downsampling.
/// Called once in initialize(), avoids 256 exp() calls per hop at runtime.
fn compute_display_bin_ranges(half_plus_one: usize) -> [(usize, usize); DISPLAY_BINS] {
    let mut ranges = [(0usize, 0usize); DISPLAY_BINS];
    if half_plus_one <= 1 {
        return ranges;
    }
    let log_max = (half_plus_one as f32).ln();
    for (i, range) in ranges.iter_mut().enumerate() {
        let lo = ((log_max * i as f32 / DISPLAY_BINS as f32).exp()) as usize;
        let hi = ((log_max * (i + 1) as f32 / DISPLAY_BINS as f32).exp()) as usize;
        range.0 = lo.max(1).min(half_plus_one - 1);
        range.1 = hi.max(range.0 + 1).min(half_plus_one);
    }
    ranges
}

/// Downsample FFT magnitudes using pre-computed bin boundaries.
fn downsample_magnitudes(
    src: &[f32],
    dst: &mut [f32; DISPLAY_BINS],
    ranges: &[(usize, usize); DISPLAY_BINS],
) {
    for (slot, &(lo, hi)) in dst.iter_mut().zip(ranges.iter()) {
        let mut peak = 0.0_f32;
        let hi = hi.min(src.len());
        let lo = lo.min(hi);
        for &val in &src[lo..hi] {
            peak = peak.max(val);
        }
        *slot = peak;
    }
}

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for WarpZone {
    const NAME: &'static str = "Warp Zone";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.display.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.shifter_l.reset();
        self.shifter_r.reset();
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
        self.display_counter = 0;
        context.set_latency_samples(self.shifter_l.latency_samples() as u32);
        true
    }

    fn reset(&mut self) {
        self.shifter_l.reset();
        self.shifter_r.reset();
        self.feedback_l = 0.0;
        self.feedback_r = 0.0;
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
        self.display_counter = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = channel_slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        // Silence output when frozen and transport is stopped
        let playing = context.transport().playing;
        if self.params.freeze.value() && !playing {
            left.fill(0.0);
            right.fill(0.0);
            return ProcessStatus::Normal;
        }

        let mix = self.params.mix.value() / 100.0;
        let dry_mix = 1.0 - mix;
        let freeze = self.params.freeze.value();

        // Convert frequency range to bin indices
        let half_plus_one = FFT_SIZE / 2 + 1;
        let bin_hz = self.sample_rate / FFT_SIZE as f32;
        let low_bin = (self.params.low_freq.value() / bin_hz).round() as usize;
        let high_bin = (self.params.high_freq.value() / bin_hz).round() as usize;
        let low_bin = low_bin.max(1).min(half_plus_one);
        let high_bin = high_bin.max(low_bin).min(half_plus_one);

        // Shift and stretch only take effect at hop boundaries (every 1024 samples),
        // so read them once per block rather than advancing the smoother per-sample.
        let final_shift = self.params.shift.value();
        let final_stretch = self.params.stretch.value();

        for i in 0..num_samples {
            let fb_amount = self.params.feedback.smoothed.next() / 100.0;

            // Mix feedback from previous output into input
            let in_l = left[i] + self.feedback_l * fb_amount;
            let in_r = right[i] + self.feedback_r * fb_amount;

            // Dry delay for latency compensation
            let dry_l = self.dry_delay_l[self.dry_delay_pos];
            let dry_r = self.dry_delay_r[self.dry_delay_pos];
            self.dry_delay_l[self.dry_delay_pos] = left[i];
            self.dry_delay_r[self.dry_delay_pos] = right[i];
            self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

            // Phase vocoder
            let wet_l = self.shifter_l.process_sample(in_l, final_shift, final_stretch, freeze, low_bin, high_bin);
            let wet_r = self.shifter_r.process_sample(in_r, final_shift, final_stretch, freeze, low_bin, high_bin);

            // Store for feedback (clip to prevent runaway)
            self.feedback_l = wet_l.clamp(-4.0, 4.0);
            self.feedback_r = wet_r.clamp(-4.0, 4.0);

            // Dry/wet mix
            left[i] = mix * wet_l + dry_mix * dry_l;
            right[i] = mix * wet_r + dry_mix * dry_r;

            // Push display column at hop boundaries
            self.display_counter += 1;
            if self.display_counter >= HOP_SIZE {
                self.display_counter = 0;
                let mags = self.shifter_l.output_magnitudes();
                let mut display_col = [0.0_f32; DISPLAY_BINS];
                downsample_magnitudes(mags, &mut display_col, &self.display_bin_ranges);
                self.display.push_column(&display_col);
            }
        }

        ProcessStatus::Normal
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for WarpZone {
    const CLAP_ID: &'static str = "com.mpd.warp-zone";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A psychedelic spectral shifter/stretcher");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::PitchShifter];
}

impl Vst3Plugin for WarpZone {
    const VST3_CLASS_ID: [u8; 16] = *b"WarpZoneMpd\0\0\0\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::PitchShift];
}

nih_export_clap!(WarpZone);
nih_export_vst3!(WarpZone);

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_silence_passthrough() {
        let mut shifter_l = spectral::SpectralShifter::new(FFT_SIZE, HOP_SIZE);
        let mut shifter_r = spectral::SpectralShifter::new(FFT_SIZE, HOP_SIZE);

        for _ in 0..16384 {
            let out_l = shifter_l.process_sample(0.0, 0.0, 1.0, false, 0, usize::MAX);
            let out_r = shifter_r.process_sample(0.0, 0.0, 1.0, false, 0, usize::MAX);
            assert!(out_l.abs() < 1e-10);
            assert!(out_r.abs() < 1e-10);
        }
    }

    /// Test dry/wet mix with latency-compensated dry path.
    #[test]
    fn test_dry_wet_mix() {
        let sr = 48000.0_f32;
        let freq = 440.0;
        let num_samples = 32768;

        let input: Vec<f32> = (0..num_samples)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect();

        let mut shifter = spectral::SpectralShifter::new(FFT_SIZE, HOP_SIZE);
        let mut dry_delay = vec![0.0_f32; FFT_SIZE];
        let mut dry_pos = 0;

        let mix = 0.5_f32;
        let dry_mix = 1.0 - mix;

        let mut output = Vec::with_capacity(num_samples);
        for i in 0..num_samples {
            let dry = dry_delay[dry_pos];
            dry_delay[dry_pos] = input[i];
            dry_pos = (dry_pos + 1) % FFT_SIZE;

            let wet = shifter.process_sample(input[i], 0.0, 1.0, false, 0, usize::MAX);
            output.push(mix * wet + dry_mix * dry);
        }

        let skip = FFT_SIZE * 2;
        let mut max_err = 0.0_f32;
        for i in skip..num_samples {
            let expected = input[i - FFT_SIZE];
            let err = (output[i] - expected).abs();
            max_err = max_err.max(err);
        }
        assert!(max_err < 0.02, "dry/wet mix identity error: {max_err}");
    }

    #[test]
    fn test_band_magnitudes_nonzero() {
        let half = FFT_SIZE / 2 + 1;
        let mags: Vec<f32> = (0..half)
            .map(|k| if k > 0 && k < 1000 { 0.5 } else { 0.0 })
            .collect();

        let ranges = compute_display_bin_ranges(half);
        let mut display_col = [0.0_f32; DISPLAY_BINS];
        downsample_magnitudes(&mags, &mut display_col, &ranges);
        let total: f32 = display_col.iter().sum();
        assert!(total > 0.0, "display magnitudes should be non-zero");
    }

    #[test]
    fn test_spectral_display_push_read() {
        let display = SpectralDisplay::new();
        let mut col = [0.0_f32; DISPLAY_BINS];
        col[0] = 1.0;
        col[DISPLAY_BINS - 1] = 0.5;
        display.push_column(&col);

        assert_eq!(display.read(0, 0), 1.0);
        assert_eq!(display.read(0, DISPLAY_BINS - 1), 0.5);
        assert_eq!(display.write_pos.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_spectral_display_wraps() {
        let display = SpectralDisplay::new();
        let col = [0.42_f32; DISPLAY_BINS];
        for _ in 0..DISPLAY_COLUMNS {
            display.push_column(&col);
        }
        // write_pos should wrap back to 0
        assert_eq!(display.write_pos.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_downsample_magnitudes_empty() {
        let ranges = compute_display_bin_ranges(1);
        let src = [0.0_f32; 1];
        let mut dst = [0.0_f32; DISPLAY_BINS];
        downsample_magnitudes(&src, &mut dst, &ranges);
        assert!(dst.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn test_downsample_magnitudes_log_mapping() {
        let half = FFT_SIZE / 2 + 1;
        let ranges = compute_display_bin_ranges(half);
        let mut mags = vec![0.0_f32; half];
        mags[10] = 1.0;
        let mut dst = [0.0_f32; DISPLAY_BINS];
        downsample_magnitudes(&mags, &mut dst, &ranges);
        // At least one display bin should capture the low-frequency energy
        let total: f32 = dst.iter().sum();
        assert!(total > 0.0, "low-frequency energy should be captured");
    }
}
