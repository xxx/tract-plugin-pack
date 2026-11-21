use nih_plug::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crossover::{Crossover, MAX_HZ, MIN_HZ, Splits};
use dynamics::{BandControls, Dynamics, Ramp};

mod crossover;
mod dynamics;
mod editor;

const REFERENCE_SAMPLE_RATE: f32 = 48_000.0;
const CONTROL_INTERVAL: u8 = 8;

pub struct Lincoln {
    params: Arc<LincolnParams>,
    meters: Arc<Meters>,
    engine: Engine,
}

#[derive(Params)]
struct LincolnParams {
    #[persist = "editor-state"]
    editor_state: Arc<tiny_skia_widgets::EditorState>,
    #[id = "depth"]
    depth: FloatParam,
    #[id = "time"]
    time: FloatParam,
    #[id = "input"]
    input: FloatParam,
    #[id = "output"]
    output: FloatParam,
    #[id = "upward"]
    upward: FloatParam,
    #[id = "downward"]
    downward: FloatParam,
    #[id = "split_low"]
    split_low: FloatParam,
    #[id = "split_high"]
    split_high: FloatParam,
    #[nested(array, group = "Band")]
    bands: [BandParams; 3],
    #[id = "bypass"]
    bypass: BoolParam,
}

#[derive(Params)]
struct BandParams {
    #[id = "threshold"]
    threshold: FloatParam,
    #[id = "gain"]
    gain: FloatParam,
    #[id = "bypass_up"]
    bypass_up: BoolParam,
    #[id = "bypass_down"]
    bypass_down: BoolParam,
}

impl BandParams {
    fn new(name: &str) -> Self {
        Self {
            threshold: percentage(
                format!("{name} Threshold"),
                FloatRange::Linear { min: 0.0, max: 2.0 },
            ),
            gain: gain(
                format!("{name} Gain"),
                FloatRange::Linear { min: 0.0, max: 2.0 },
            ),
            bypass_up: BoolParam::new(format!("{name} Upward Bypass"), false),
            bypass_down: BoolParam::new(format!("{name} Downward Bypass"), false),
        }
    }

    fn controls(&self) -> BandControls {
        BandControls {
            threshold: self.threshold.value(),
            gain: self.gain.value(),
            upward: !self.bypass_up.value(),
            downward: !self.bypass_down.value(),
        }
    }

    fn next_controls(&self) -> BandControls {
        BandControls {
            threshold: self.threshold.smoothed.next(),
            gain: self.gain.smoothed.next(),
            upward: !self.bypass_up.value(),
            downward: !self.bypass_down.value(),
        }
    }
}

fn percentage(name: impl Into<String>, range: FloatRange) -> FloatParam {
    FloatParam::new(name, 1.0, range)
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
        .with_smoother(SmoothingStyle::Linear(20.0))
}

fn gain(name: impl Into<String>, range: FloatRange) -> FloatParam {
    FloatParam::new(name, 1.0, range)
        .with_unit(" dB")
        .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
        .with_string_to_value(formatters::s2v_f32_gain_to_db())
        .with_smoother(SmoothingStyle::Linear(20.0))
}

fn frequency(name: &str, default: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min: MIN_HZ,
            max: MAX_HZ,
            factor: FloatRange::skew_factor(-2.75),
        },
    )
    .with_unit(" Hz")
    .with_value_to_string(formatters::v2s_f32_rounded(1))
    .with_string_to_value(formatters::s2v_f32_hz_then_khz())
    .with_smoother(SmoothingStyle::Logarithmic(30.0))
}

impl Default for LincolnParams {
    fn default() -> Self {
        let strength = FloatRange::Linear { min: 0.0, max: 2.0 };
        let centered = |max| FloatRange::SymmetricalSkewed {
            min: 0.0,
            max,
            factor: 1.0,
            center: 1.0,
        };
        Self {
            editor_state: editor::default_state(),
            depth: percentage("Depth", FloatRange::Linear { min: 0.0, max: 1.0 }),
            time: percentage("Time", centered(10.0)),
            input: gain("In Gain", centered(9.0)),
            output: gain("Out Gain", centered(9.0)),
            upward: percentage("Upward", strength),
            downward: percentage("Downward", strength),
            split_low: frequency("Low/Mid", Splits::FACTORY.lower),
            split_high: frequency("Mid/High", Splits::FACTORY.upper),
            bands: ["Low", "Mid", "High"].map(BandParams::new),
            bypass: BoolParam::new("Bypass", false).make_bypass(),
        }
    }
}

impl LincolnParams {
    fn splits(&self, sample_rate: f32) -> Splits {
        Splits::new(self.split_low.value(), self.split_high.value(), sample_rate)
    }
}

struct Meters {
    gain_db: [AtomicU32; 3],
    sample_rate: AtomicU32,
    applied_splits: AtomicU64,
    processed_blocks: AtomicU64,
    max_block_size: AtomicU32,
}

impl Default for Meters {
    fn default() -> Self {
        Self {
            gain_db: Default::default(),
            sample_rate: AtomicU32::new(REFERENCE_SAMPLE_RATE.to_bits()),
            applied_splits: AtomicU64::new(Splits::FACTORY.to_bits()),
            processed_blocks: AtomicU64::new(0),
            max_block_size: AtomicU32::new(0),
        }
    }
}

impl Meters {
    fn update_splits(&self, applied: Splits) {
        self.applied_splits
            .store(applied.to_bits(), Ordering::Relaxed);
    }

    fn publish_splits(&self, applied: Splits) {
        self.update_splits(applied);
        self.processed_blocks.fetch_add(1, Ordering::Release);
    }
}

struct Engine {
    sample_rate: f32,
    splits: Splits,
    crossover: Crossover,
    channels: [crossover::Channel; 2],
    bands: [Dynamics; 3],
    time: f32,
    wet: Ramp,
    control_count: u8,
}

impl Engine {
    fn new(sample_rate: f32, params: &LincolnParams) -> Self {
        let splits = params.splits(sample_rate);
        let time = params.time.value();
        Self {
            sample_rate,
            splits,
            crossover: Crossover::new(splits, sample_rate),
            channels: Default::default(),
            bands: std::array::from_fn(|i| {
                Dynamics::new(i, sample_rate, time, params.bands[i].controls())
            }),
            time,
            wet: Ramp::new(!params.bypass.value(), sample_rate),
            control_count: 0,
        }
    }

    fn update_controls(&mut self, params: &LincolnParams) {
        let lower = params.split_low.smoothed.next();
        let upper = params.split_high.smoothed.next();
        let time = params.time.smoothed.next();
        if self.control_count == 0 {
            let splits = Splits::new(lower, upper, self.sample_rate);
            if splits != self.splits {
                self.splits = splits;
                self.crossover = Crossover::new(splits, self.sample_rate);
            }
            if time != self.time {
                self.time = time;
                for band in &mut self.bands {
                    band.set_time(time, self.sample_rate);
                }
            }
            self.control_count = CONTROL_INTERVAL;
        }
        self.control_count -= 1;
    }

    fn process_status(&self) -> ProcessStatus {
        match self.bands.iter().map(Dynamics::tail_samples).max() {
            Some(samples) if samples > 0 => ProcessStatus::Tail(samples),
            _ => ProcessStatus::Normal,
        }
    }

    fn process(
        &mut self,
        input: [f32; 2],
        mono: bool,
        params: &LincolnParams,
    ) -> ([f32; 2], [f32; 3]) {
        self.update_controls(params);
        let input_gain = params.input.smoothed.next();
        let output_gain = params.output.smoothed.next();
        let depth = params.depth.smoothed.next();
        let upward = params.upward.smoothed.next();
        let downward = params.downward.smoothed.next();
        let left = self
            .crossover
            .process(&mut self.channels[0], input[0] * input_gain);
        let right = if mono {
            left
        } else {
            self.crossover
                .process(&mut self.channels[1], input[1] * input_gain)
        };
        let mut wet = [0.0; 2];
        let gain_db = std::array::from_fn(|i| {
            let (gain, gain_db) = self.bands[i].process(
                [left[i], right[i]],
                depth,
                upward,
                downward,
                params.bands[i].next_controls(),
            );
            wet[0] += left[i] * gain;
            wet[1] += right[i] * gain;
            gain_db
        });
        let amount = self.wet.next(!params.bypass.value());
        let output = if amount == 0.0 {
            input
        } else {
            std::array::from_fn(|i| input[i] * (1.0 - amount) + wet[i] * (amount * output_gain))
        };
        (output, gain_db.map(|db| db * amount))
    }
}

impl Default for Lincoln {
    fn default() -> Self {
        let params = Arc::new(LincolnParams::default());
        let engine = Engine::new(REFERENCE_SAMPLE_RATE, &params);
        Self {
            params,
            meters: Arc::new(Meters::default()),
            engine,
        }
    }
}

impl Plugin for Lincoln {
    const NAME: &'static str = "Lincoln";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
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
    const SAMPLE_ACCURATE_AUTOMATION: bool = true;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        Some(editor::create(self.params.clone(), self.meters.clone()))
    }

    fn initialize(
        &mut self,
        _: &AudioIOLayout,
        config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        if !config.sample_rate.is_finite() || config.sample_rate < 100.0 {
            return false;
        }
        self.meters
            .sample_rate
            .store(config.sample_rate.to_bits(), Ordering::Relaxed);
        self.meters
            .max_block_size
            .store(config.max_buffer_size, Ordering::Relaxed);
        self.engine = Engine::new(config.sample_rate, &self.params);
        self.meters.update_splits(self.engine.splits);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.engine = Engine::new(self.engine.sample_rate, &self.params);
        self.meters.update_splits(self.engine.splits);
        for meter in &self.meters.gain_db {
            meter.store(0, Ordering::Relaxed);
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _: &mut AuxiliaryBuffers,
        _: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let mut meter = [0.0; 3];
        for channels in buffer.iter_samples() {
            let mut channels = channels.into_iter();
            let Some(left) = channels.next() else {
                continue;
            };
            let right = channels.next();
            let input = [*left, right.as_deref().copied().unwrap_or(*left)];
            let (output, band_gain) = self.engine.process(input, right.is_none(), &self.params);
            *left = output[0];
            if let Some(right) = right {
                *right = output[1];
            }
            meter = band_gain;
        }
        for (slot, db) in self.meters.gain_db.iter().zip(meter) {
            slot.store(db.to_bits(), Ordering::Relaxed);
        }
        if buffer.samples() > 0 {
            self.meters.publish_splits(self.engine.splits);
        }
        self.engine.process_status()
    }
}

impl ClapPlugin for Lincoln {
    const CLAP_ID: &'static str = "com.mpd.lincoln";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Three-band upward and downward compressor");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Compressor,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Lincoln {
    const VST3_CLASS_ID: [u8; 16] = *b"LincolnCompMpd\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(Lincoln);
nih_export_vst3!(Lincoln);

#[cfg(test)]
mod tests {
    use super::*;

    fn factory_params() -> LincolnParams {
        let params = LincolnParams::default();
        let globals = [
            &params.depth,
            &params.time,
            &params.input,
            &params.output,
            &params.upward,
            &params.downward,
            &params.split_low,
            &params.split_high,
        ];
        for param in globals.into_iter().chain(
            params
                .bands
                .iter()
                .flat_map(|band| [&band.threshold, &band.gain]),
        ) {
            param.smoothed.reset(param.value());
        }
        params
    }

    #[test]
    fn parameters_use_measured_factory_values() {
        let params = LincolnParams::default();
        for param in [
            &params.depth,
            &params.time,
            &params.input,
            &params.output,
            &params.upward,
            &params.downward,
        ] {
            assert_eq!(param.value(), 1.0);
        }
        assert_eq!(params.time.default_normalized_value(), 0.5);
        assert_eq!(params.splits(48_000.0), Splits::FACTORY);
        assert!(!params.bypass.value());
        for band in &params.bands {
            let c = band.controls();
            assert_eq!(
                (c.threshold, c.gain, c.upward, c.downward),
                (1.0, 1.0, true, true)
            );
        }
    }

    #[test]
    fn stereo_is_linked_without_crossfeed_and_mono_matches_dual_mono() {
        let params = factory_params();
        let mut stereo = Engine::new(48_000.0, &params);
        let mut mono = Engine::new(48_000.0, &params);
        let mut split = Engine::new(48_000.0, &params);
        let mut peak = 0.0_f32;
        for i in 0..1024 {
            let input = (i as f32 * 0.17).sin() * 0.17;
            let (both, _) = stereo.process([input; 2], false, &params);
            let (one, _) = mono.process([input; 2], true, &params);
            let (asymmetrical, _) = split.process([input, -0.4 * input], false, &params);
            peak = peak.max(both[0].abs());
            assert_eq!(both, one);
            assert!((asymmetrical[0] * -0.4 - asymmetrical[1]).abs() < 1e-6);
        }
        assert!(peak > 0.01);
    }

    #[test]
    fn silence_stays_exactly_silent_at_factory_settings() {
        let params = factory_params();
        let mut engine = Engine::new(48_000.0, &params);
        for _ in 0..128 {
            assert_eq!(engine.process([0.0; 2], false, &params).0, [0.0; 2]);
        }
    }

    #[test]
    fn simultaneous_split_automation_smooths_and_preserves_band_order() {
        let params = factory_params();
        let sample_rate = 48_000.0;
        let mut engine = Engine::new(sample_rate, &params);
        params.depth.smoothed.reset(0.0);
        params.split_low.smoothed.set_target(sample_rate, 9000.0);
        params.split_high.smoothed.set_target(sample_rate, 1400.0);

        let requested = Splits::new(9000.0, 1400.0, sample_rate);
        let mut latest = engine.splits;
        for sample in 0..2000 {
            let input = (sample as f32 * 0.21).sin() * 0.1;
            let (output, _) = engine.process([input; 2], false, &params);
            let splits = engine.splits;
            assert!(splits.lower < splits.upper && splits.upper < sample_rate * 0.5);
            assert!(output[0].abs() < 0.4);
            assert!(
                (splits.lower / latest.lower).ln().abs() < 0.05,
                "{latest:?} => {splits:?}"
            );
            latest = splits;
        }
        assert_eq!(latest, requested);
    }

    #[test]
    fn bypassed_silence_keeps_the_detector_running_until_it_settles() {
        let mut params = factory_params();
        params.bypass = BoolParam::new("Bypass", true).make_bypass();
        let mut engine = Engine::new(48_000.0, &params);
        assert_eq!(engine.process_status(), ProcessStatus::Normal);
        for index in 0..4096 {
            let signal = (index as f32 * 0.18).sin() * 0.4;
            engine.process([signal; 2], false, &params);
        }

        for _ in 0..128 {
            assert_eq!(engine.process([0.0; 2], false, &params).0, [0.0; 2]);
        }
        let ProcessStatus::Tail(samples) = engine.process_status() else {
            panic!("a silent bypass block cut off the detector release");
        };

        for _ in 0..samples + 4096 {
            engine.process([0.0; 2], false, &params);
        }
        assert_eq!(engine.process_status(), ProcessStatus::Normal);
    }
}
