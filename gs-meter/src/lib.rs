#![feature(portable_simd)]

use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod editor;
pub mod lufs;
pub mod meter;

use meter::{linear_to_db, StereoMeter};

/// Shared meter readings for the GUI (written by audio thread, read by GUI).
pub struct MeterReadings {
    pub peak_max_db: std::sync::atomic::AtomicI32,
    pub true_peak_max_db: std::sync::atomic::AtomicI32,
    pub rms_integrated_db: std::sync::atomic::AtomicI32,
    pub rms_momentary_db: std::sync::atomic::AtomicI32,
    pub rms_momentary_max_db: std::sync::atomic::AtomicI32,
    pub crest_factor_db: std::sync::atomic::AtomicI32,
    // LUFS mode readings (Phase 2 will populate with real values)
    pub lufs_integrated: std::sync::atomic::AtomicI32,
    pub lufs_short_term: std::sync::atomic::AtomicI32,
    pub lufs_short_term_max: std::sync::atomic::AtomicI32,
    pub lufs_momentary: std::sync::atomic::AtomicI32,
    pub lufs_momentary_max: std::sync::atomic::AtomicI32,
    pub lufs_range: std::sync::atomic::AtomicI32,
}

impl MeterReadings {
    fn new() -> Self {
        Self {
            peak_max_db: std::sync::atomic::AtomicI32::new(-10000),
            true_peak_max_db: std::sync::atomic::AtomicI32::new(-10000),
            rms_integrated_db: std::sync::atomic::AtomicI32::new(-10000),
            rms_momentary_db: std::sync::atomic::AtomicI32::new(-10000),
            rms_momentary_max_db: std::sync::atomic::AtomicI32::new(-10000),
            crest_factor_db: std::sync::atomic::AtomicI32::new(-10000),
            lufs_integrated: std::sync::atomic::AtomicI32::new(-10000),
            lufs_short_term: std::sync::atomic::AtomicI32::new(-10000),
            lufs_short_term_max: std::sync::atomic::AtomicI32::new(-10000),
            lufs_momentary: std::sync::atomic::AtomicI32::new(-10000),
            lufs_momentary_max: std::sync::atomic::AtomicI32::new(-10000),
            lufs_range: std::sync::atomic::AtomicI32::new(-10000),
        }
    }

    /// Store a dB value as fixed-point (multiply by 100 for 0.01 dB resolution).
    fn store_db(atom: &std::sync::atomic::AtomicI32, db: f32) {
        let fixed = if db.is_finite() {
            (db * 100.0).round() as i32
        } else {
            -10000 // -100.00 dB floor
        };
        atom.store(fixed, Ordering::Relaxed);
    }

    /// Load a dB value from fixed-point.
    pub fn load_db(atom: &std::sync::atomic::AtomicI32) -> f32 {
        atom.load(Ordering::Relaxed) as f32 / 100.0
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ChannelMode {
    #[id = "stereo"]
    #[name = "Stereo"]
    Stereo,

    #[id = "left"]
    #[name = "Left"]
    Left,

    #[id = "right"]
    #[name = "Right"]
    Right,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum MeterMode {
    #[id = "db"]
    #[name = "dB"]
    Db,

    #[id = "lufs"]
    #[name = "LUFS"]
    Lufs,
}

pub struct GsMeter {
    params: Arc<GsMeterParams>,
    stereo_meter: StereoMeter,
    lufs_meter: lufs::LufsMeter,
    sample_rate: f32,
    last_window_ms: f32,
    readings: Arc<MeterReadings>,
    should_reset: Arc<AtomicBool>,
}

#[derive(Params)]
pub struct GsMeterParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    #[id = "gain"]
    pub gain: FloatParam,

    #[id = "reference"]
    pub reference_level: FloatParam,

    #[id = "gain_lufs"]
    pub gain_lufs: FloatParam,

    #[id = "reference_lufs"]
    pub reference_lufs: FloatParam,

    #[id = "rms_window"]
    pub rms_window_ms: FloatParam,

    #[id = "channel_mode"]
    pub channel_mode: EnumParam<ChannelMode>,

    #[id = "meter_mode"]
    pub meter_mode: EnumParam<MeterMode>,
}

impl Default for GsMeter {
    fn default() -> Self {
        let default_sr = 48000.0;
        let default_window_ms = 600.0;
        let window_samples = (default_sr * default_window_ms / 1000.0) as usize;

        Self {
            params: Arc::new(GsMeterParams::new()),
            stereo_meter: StereoMeter::new(window_samples),
            lufs_meter: lufs::LufsMeter::new(default_sr as f64),
            sample_rate: default_sr,
            last_window_ms: default_window_ms,
            readings: Arc::new(MeterReadings::new()),
            should_reset: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl GsMeterParams {
    fn new() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(40.0),
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            reference_level: FloatParam::new(
                "Reference",
                0.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 0.0,
                },
            )
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),

            gain_lufs: FloatParam::new(
                "Gain (LUFS)",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(40.0),
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" LU")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            reference_lufs: FloatParam::new(
                "Reference (LUFS)",
                -14.0,
                FloatRange::Linear {
                    min: -60.0,
                    max: 0.0,
                },
            )
            .with_unit(" LUFS")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),

            rms_window_ms: FloatParam::new(
                "RMS Window",
                600.0,
                FloatRange::Skewed {
                    min: 50.0,
                    max: 3000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            channel_mode: EnumParam::new("Channel", ChannelMode::Stereo),
            meter_mode: EnumParam::new("Mode", MeterMode::Db),
        }
    }
}

impl Plugin for GsMeter {
    const NAME: &'static str = "GS Meter";
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
        editor::create(
            self.params.clone(),
            self.readings.clone(),
            self.should_reset.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.stereo_meter.set_sample_rate(self.sample_rate);
        self.lufs_meter.set_sample_rate(self.sample_rate as f64);
        let window_ms = self.params.rms_window_ms.value();
        let window_samples = (self.sample_rate * window_ms / 1000.0) as usize;
        self.stereo_meter.set_window_size(window_samples);
        self.stereo_meter.reset();
        self.lufs_meter.reset();
        self.last_window_ms = window_ms;
        true
    }

    fn reset(&mut self) {
        self.stereo_meter.reset();
        self.lufs_meter.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Check for reset request from GUI
        if self.should_reset.swap(false, Ordering::Relaxed) {
            self.stereo_meter.reset();
            self.lufs_meter.reset();
        }

        // Check if RMS window size changed
        let window_ms = self.params.rms_window_ms.value();
        if (window_ms - self.last_window_ms).abs() > 0.5 {
            let window_samples = (self.sample_rate * window_ms / 1000.0) as usize;
            self.stereo_meter.set_window_size(window_samples);
            self.last_window_ms = window_ms;
        }

        let num_channels = buffer.channels();
        if num_channels < 2 {
            return ProcessStatus::Normal;
        }

        // Apply gain and collect samples for metering
        let num_samples = buffer.samples();
        let channel_slices = buffer.as_slice();

        // Apply gain sample-by-sample (with smoothing)
        // Use mode-appropriate gain parameter
        let meter_mode = self.params.meter_mode.value();
        // Indexing two channels simultaneously — iterator doesn't apply cleanly
        #[allow(clippy::needless_range_loop)]
        for i in 0..num_samples {
            let gain = match meter_mode {
                MeterMode::Db => self.params.gain.smoothed.next(),
                MeterMode::Lufs => self.params.gain_lufs.smoothed.next(),
            };
            channel_slices[0][i] *= gain;
            channel_slices[1][i] *= gain;
        }

        // Meter the post-gain signal
        let (left, right) = channel_slices.split_at(1);
        self.stereo_meter.process_buffer(left[0], right[0]);

        // LUFS metering (K-weighted, always runs regardless of mode so values
        // are available immediately when switching to LUFS mode)
        for i in 0..num_samples {
            self.lufs_meter.process_sample(left[0][i], right[0][i]);
        }
        self.lufs_meter.update_maxes();

        // Update shared readings for GUI
        let mode = self.params.channel_mode.value();
        let (peak, true_peak, rms_int, rms_mom, rms_mom_max, crest) = match mode {
            ChannelMode::Stereo => (
                self.stereo_meter.peak_max_stereo(),
                self.stereo_meter.true_peak_max_stereo(),
                self.stereo_meter.rms_integrated_stereo(),
                self.stereo_meter.rms_momentary_stereo(),
                self.stereo_meter.rms_momentary_max_stereo(),
                self.stereo_meter.crest_factor_db_stereo(),
            ),
            ChannelMode::Left => (
                self.stereo_meter.left.peak_max(),
                self.stereo_meter.left.true_peak_max(),
                self.stereo_meter.left.rms_integrated_linear(),
                self.stereo_meter.left.rms_momentary_linear(),
                self.stereo_meter.left.rms_momentary_max(),
                self.stereo_meter.left.crest_factor_db(),
            ),
            ChannelMode::Right => (
                self.stereo_meter.right.peak_max(),
                self.stereo_meter.right.true_peak_max(),
                self.stereo_meter.right.rms_integrated_linear(),
                self.stereo_meter.right.rms_momentary_linear(),
                self.stereo_meter.right.rms_momentary_max(),
                self.stereo_meter.right.crest_factor_db(),
            ),
        };

        MeterReadings::store_db(&self.readings.peak_max_db, linear_to_db(peak));
        MeterReadings::store_db(&self.readings.true_peak_max_db, linear_to_db(true_peak));
        MeterReadings::store_db(&self.readings.rms_integrated_db, linear_to_db(rms_int));
        MeterReadings::store_db(&self.readings.rms_momentary_db, linear_to_db(rms_mom));
        MeterReadings::store_db(
            &self.readings.rms_momentary_max_db,
            linear_to_db(rms_mom_max),
        );
        MeterReadings::store_db(&self.readings.crest_factor_db, crest);

        // LUFS readings from EBU R128 metering
        MeterReadings::store_db(
            &self.readings.lufs_integrated,
            self.lufs_meter.integrated_lufs() as f32,
        );
        MeterReadings::store_db(
            &self.readings.lufs_short_term,
            self.lufs_meter.short_term_lufs() as f32,
        );
        MeterReadings::store_db(
            &self.readings.lufs_short_term_max,
            self.lufs_meter.short_term_max_lufs() as f32,
        );
        MeterReadings::store_db(
            &self.readings.lufs_momentary,
            self.lufs_meter.momentary_lufs() as f32,
        );
        MeterReadings::store_db(
            &self.readings.lufs_momentary_max,
            self.lufs_meter.momentary_max_lufs() as f32,
        );
        MeterReadings::store_db(
            &self.readings.lufs_range,
            self.lufs_meter.loudness_range() as f32,
        );

        ProcessStatus::Normal
    }
}

impl ClapPlugin for GsMeter {
    const CLAP_ID: &'static str = "com.mpd.gs-meter";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A loudness meter with gain utility for clip-to-zero workflow");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Analyzer,
        ClapFeature::Utility,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for GsMeter {
    const VST3_CLASS_ID: [u8; 16] = *b"GsMeterMpdPlugin";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Analyzer,
        Vst3SubCategory::Tools,
    ];
}

nih_export_clap!(GsMeter);
nih_export_vst3!(GsMeter);
