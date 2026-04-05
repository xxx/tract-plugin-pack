#![feature(portable_simd)]

use nih_plug::prelude::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

mod editor;
pub mod limiter;
pub mod true_peak;

use limiter::Limiter;
use true_peak::TruePeakDetector;

// ── Meter readings ───────────────────────────────────────────────────────────

/// Shared meter readings for the GUI (written by audio thread, read by GUI).
///
/// Values are stored as fixed-point i32 (value * 100) for 0.01 dB resolution.
pub struct MeterReadings {
    pub input_peak_l: AtomicI32,
    pub input_peak_r: AtomicI32,
    pub output_peak_l: AtomicI32,
    pub output_peak_r: AtomicI32,
    pub gain_reduction: AtomicI32,
}

impl MeterReadings {
    fn new() -> Self {
        Self {
            input_peak_l: AtomicI32::new(-10000),
            input_peak_r: AtomicI32::new(-10000),
            output_peak_l: AtomicI32::new(-10000),
            output_peak_r: AtomicI32::new(-10000),
            gain_reduction: AtomicI32::new(0),
        }
    }

    /// Store a dB value as fixed-point (multiply by 100 for 0.01 dB resolution).
    pub fn store_db(atom: &AtomicI32, db: f32) {
        let fixed = if db.is_finite() {
            (db * 100.0).round() as i32
        } else {
            -10000 // -100.00 dB floor
        };
        atom.store(fixed, Ordering::Relaxed);
    }

    /// Load a dB value from fixed-point.
    pub fn load_db(atom: &AtomicI32) -> f32 {
        atom.load(Ordering::Relaxed) as f32 / 100.0
    }
}

// ── Constants ────────────────────────────────────────────────────────────────

/// Maximum lookahead / attack time in milliseconds.
const MAX_LOOKAHEAD_MS: f32 = 10.0;

/// Default sample rate for initialization before the host calls initialize().
const DEFAULT_SAMPLE_RATE: f32 = 48000.0;

// ── Plugin struct ──────────────────────────────────────────────────────────────

pub struct Tinylimit {
    params: Arc<TinylimitParams>,
    limiter: Limiter,
    readings: Arc<MeterReadings>,
    true_peak_detectors: [TruePeakDetector; 2],
    last_reported_latency: u32,
    last_attack_ms: f32,
    last_release_ms: f32,
    /// Counts silent input samples for conditional ProcessStatus::Tail.
    silent_input_samples: u32,
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct TinylimitParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    /// Input gain: stored as linear gain, displayed/edited in dB.
    #[id = "input"]
    pub input: FloatParam,

    /// Threshold in dB (linear dB range, not stored as linear gain).
    #[id = "threshold"]
    pub threshold: FloatParam,

    /// Output ceiling in dB.
    #[id = "ceiling"]
    pub ceiling: FloatParam,

    /// Attack time in milliseconds.
    #[id = "attack"]
    pub attack: FloatParam,

    /// Release time in milliseconds.
    #[id = "release"]
    pub release: FloatParam,

    /// Soft knee width in dB.
    #[id = "knee"]
    pub knee: FloatParam,

    /// Stereo link amount (0–100%).
    #[id = "stereo_link"]
    pub stereo_link: FloatParam,

    /// Transient mix amount (0–100%).
    #[id = "transient_mix"]
    pub transient_mix: FloatParam,

    /// Intersample peak detection toggle.
    #[id = "isp"]
    pub isp: BoolParam,

    /// Link input gain to threshold.
    #[id = "gain_link"]
    pub gain_link: BoolParam,
}

impl Default for Tinylimit {
    fn default() -> Self {
        Self {
            params: Arc::new(TinylimitParams::new()),
            limiter: Limiter::new(DEFAULT_SAMPLE_RATE, MAX_LOOKAHEAD_MS),
            readings: Arc::new(MeterReadings::new()),
            true_peak_detectors: [TruePeakDetector::new(), TruePeakDetector::new()],
            last_reported_latency: 0,
            last_attack_ms: f32::NAN,
            last_release_ms: f32::NAN,
            silent_input_samples: 0,
        }
    }
}

impl TinylimitParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            input: FloatParam::new(
                "Input",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-60.0),
                    max: util::db_to_gain(18.0),
                    factor: FloatRange::gain_skew_factor(-60.0, 18.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            threshold: FloatParam::new(
                "Threshold",
                0.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 0.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            ceiling: FloatParam::new(
                "Ceiling",
                -0.1,
                FloatRange::Linear {
                    min: -30.0,
                    max: 0.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            attack: FloatParam::new(
                "Attack",
                5.0,
                FloatRange::Linear {
                    min: 0.1,
                    max: 10.0,
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            release: FloatParam::new(
                "Release",
                200.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 1000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            knee: FloatParam::new(
                "Knee",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 12.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),

            stereo_link: FloatParam::new(
                "Stereo Link",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            transient_mix: FloatParam::new(
                "Transient Mix",
                50.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            isp: BoolParam::new("ISP", false),

            gain_link: BoolParam::new("Gain Link", false),
        }
    }
}

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for Tinylimit {
    const NAME: &'static str = "tinylimit";
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
        editor::create(self.params.clone(), self.readings.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
        self.limiter.set_sample_rate(sr);
        self.limiter
            .set_max_block_size(buffer_config.max_buffer_size as usize);
        self.limiter
            .set_params(self.params.attack.value(), self.params.release.value());
        self.limiter.reset();
        self.true_peak_detectors[0].set_sample_rate(sr);
        self.true_peak_detectors[0].reset();
        self.true_peak_detectors[1].set_sample_rate(sr);
        self.true_peak_detectors[1].reset();
        true
    }

    fn reset(&mut self) {
        self.limiter.reset();
        self.true_peak_detectors[0].reset();
        self.true_peak_detectors[1].reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Read current parameter values
        let attack_ms = self.params.attack.value();
        let release_ms = self.params.release.value();
        let knee_db = self.params.knee.value();
        let transient_mix = self.params.transient_mix.value() / 100.0; // 0..1
        let stereo_link = self.params.stereo_link.value() / 100.0; // 0..1
        let gain_link = self.params.gain_link.value();
        let isp = self.params.isp.value();

        // Update limiter envelope / lookahead from current params (only when changed)
        if attack_ms != self.last_attack_ms || release_ms != self.last_release_ms {
            self.limiter.set_params(attack_ms, release_ms);
            self.last_attack_ms = attack_ms;
            self.last_release_ms = release_ms;
        }

        // Report latency to host (only when changed)
        let latency = self.limiter.latency_samples() as u32;
        if latency != self.last_reported_latency {
            context.set_latency_samples(latency);
            self.last_reported_latency = latency;
        }

        // Process in blocks (nih-plug provides sample-accurate blocks)
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        // Get channel slices — split_at_mut avoids double mutable borrow
        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = channel_slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        // 1. Apply input gain (smoothed), measure input peaks, then apply threshold boost (smoothed)
        //
        // Threshold and ceiling smoothers: read start/end values, convert to linear
        // with exp() (2 calls per block instead of N powf calls), and lerp per-sample.
        let db_to_linear_neg = -std::f32::consts::LN_10 / 20.0; // for 10^(-x/20)
        let db_to_linear_pos = std::f32::consts::LN_10 / 20.0; // for 10^(x/20)

        let threshold_db_start = self.params.threshold.smoothed.previous_value();
        // Advance smoothers through the block
        for _ in 0..num_samples {
            let _ = self.params.threshold.smoothed.next();
            let _ = self.params.ceiling.smoothed.next();
        }
        let threshold_db_end = self.params.threshold.smoothed.previous_value();
        let ceiling_db_end = self.params.ceiling.smoothed.previous_value();

        // Convert to linear domain (2 exp calls each instead of N powf calls)
        let boost_start = (threshold_db_start * db_to_linear_neg).exp();
        let boost_end = (threshold_db_end * db_to_linear_neg).exp();

        let inv_num_samples = 1.0 / num_samples as f32;

        let mut input_peak_l = 0.0_f32;
        let mut input_peak_r = 0.0_f32;
        for i in 0..num_samples {
            let input_gain = self.params.input.smoothed.next();

            // Apply input gain first
            left[i] *= input_gain;
            right[i] *= input_gain;

            // Measure input peaks HERE (post-input-gain, pre-threshold)
            let al = left[i].abs();
            let ar = right[i].abs();
            if al > input_peak_l {
                input_peak_l = al;
            }
            if ar > input_peak_r {
                input_peak_r = ar;
            }

            // Then apply threshold boost (cheap lerp instead of per-sample powf)
            let t = i as f32 * inv_num_samples;
            let threshold_boost = boost_start + t * (boost_end - boost_start);
            left[i] *= threshold_boost;
            right[i] *= threshold_boost;
        }

        // Read final smoothed values for limiter block processing
        // (the smoothers have been advanced through the block above)
        let threshold_db = threshold_db_end;
        let ceiling_db = ceiling_db_end;
        let ceiling_linear = (ceiling_db * db_to_linear_pos).exp();
        let effective_ceiling_linear = if gain_link {
            (threshold_db * db_to_linear_pos).exp()
        } else {
            ceiling_linear
        };

        // 2. Run the limiter
        let true_peak_opt = if isp {
            Some(&mut self.true_peak_detectors)
        } else {
            None
        };
        let gr = self.limiter.process_block(
            left,
            right,
            knee_db,
            transient_mix,
            stereo_link,
            effective_ceiling_linear,
            true_peak_opt,
        );

        // 3. Apply ceiling gain (scale output to ceiling level)
        //    The limiter clamps to effective_ceiling_linear. If not gain-linked,
        //    we need to scale so the output ceiling matches the user's ceiling param.
        //    When gain_link is off, the limiter ceiling is ceiling_linear, so no
        //    additional scaling needed. When gain_link is on, the limiter ceiling
        //    is 1/threshold_boost = threshold_linear, and the user expects output
        //    at that level, so no additional scaling is needed either.

        // 4. Measure output peaks
        let mut output_peak_l = 0.0_f32;
        let mut output_peak_r = 0.0_f32;
        for i in 0..num_samples {
            let al = left[i].abs();
            let ar = right[i].abs();
            if al > output_peak_l {
                output_peak_l = al;
            }
            if ar > output_peak_r {
                output_peak_r = ar;
            }
        }

        // 5. Store meter readings
        MeterReadings::store_db(&self.readings.input_peak_l, linear_to_db(input_peak_l));
        MeterReadings::store_db(&self.readings.input_peak_r, linear_to_db(input_peak_r));
        MeterReadings::store_db(&self.readings.output_peak_l, linear_to_db(output_peak_l));
        MeterReadings::store_db(&self.readings.output_peak_r, linear_to_db(output_peak_r));
        MeterReadings::store_db(&self.readings.gain_reduction, gr);

        let tail_len = self.limiter.latency_samples() as u32;
        let input_silent = input_peak_l < 1e-6 && input_peak_r < 1e-6;
        if input_silent {
            self.silent_input_samples += num_samples as u32;
        } else {
            self.silent_input_samples = 0;
        }
        if self.silent_input_samples > 0 && self.silent_input_samples <= tail_len {
            ProcessStatus::Tail(tail_len)
        } else {
            ProcessStatus::Normal
        }
    }
}

/// Convert linear amplitude to dB. Returns -100.0 for zero/negative input.
#[inline]
fn linear_to_db(linear: f32) -> f32 {
    if linear <= 0.0 {
        -100.0
    } else {
        20.0 * linear.log10()
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for Tinylimit {
    const CLAP_ID: &'static str = "com.mpd.tinylimit";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A low-latency wideband peak limiter");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Mastering,
        ClapFeature::Limiter,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Tinylimit {
    const VST3_CLASS_ID: [u8; 16] = *b"TinylimitMpdPlg\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(Tinylimit);
nih_export_vst3!(Tinylimit);
