//! Six Pack: 6-band parallel multiband saturator.

use nih_plug::prelude::*;
use std::sync::Arc;

pub mod bands;
pub mod editor;
pub mod oversampling;
pub mod saturation;
pub mod spectrum;
pub mod svf;

use crate::bands::{BandState, ChannelMode, FilterShape};
use crate::saturation::Algorithm;

const BAND_SHAPES: [FilterShape; 6] = [
    FilterShape::LowShelf,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::HighShelf,
];

#[inline]
fn dry_amp(mix: f32) -> f32 {
    (2.0 * (1.0 - mix)).clamp(0.0, 1.0)
}

#[inline]
fn wet_amp(mix: f32) -> f32 {
    (2.0 * mix).clamp(0.0, 1.0)
}

// ── Param-side enums ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum Quality {
    #[id = "off"]
    #[name = "Off"]
    Off,
    #[id = "x4"]
    #[name = "4×"]
    X4,
    #[id = "x8"]
    #[name = "8×"]
    X8,
    #[id = "x16"]
    #[name = "16×"]
    X16,
}

impl Quality {
    pub fn factor(self) -> usize {
        match self {
            Quality::Off => 1,
            Quality::X4 => 4,
            Quality::X8 => 8,
            Quality::X16 => 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum Drive {
    #[id = "easy"]
    #[name = "Easy"]
    Easy,
    #[id = "standard"]
    #[name = "Standard"]
    Standard,
    #[id = "crush"]
    #[name = "Crush"]
    Crush,
}

impl Drive {
    pub fn k(self) -> f32 {
        match self {
            Drive::Easy => 0.6,
            Drive::Standard => 1.0,
            Drive::Crush => 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum AlgoParam {
    #[id = "tube"]
    #[name = "Tube"]
    Tube,
    #[id = "tape"]
    #[name = "Tape"]
    Tape,
    #[id = "diode"]
    #[name = "Diode"]
    Diode,
    #[id = "digital"]
    #[name = "Digital"]
    Digital,
    #[id = "class_b"]
    #[name = "Class B"]
    ClassB,
    #[id = "wavefold"]
    #[name = "Wavefold"]
    Wavefold,
}

impl From<AlgoParam> for Algorithm {
    fn from(v: AlgoParam) -> Self {
        match v {
            AlgoParam::Tube => Algorithm::Tube,
            AlgoParam::Tape => Algorithm::Tape,
            AlgoParam::Diode => Algorithm::Diode,
            AlgoParam::Digital => Algorithm::Digital,
            AlgoParam::ClassB => Algorithm::ClassB,
            AlgoParam::Wavefold => Algorithm::Wavefold,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ChannelParam {
    #[id = "stereo"]
    #[name = "Stereo"]
    Stereo,
    #[id = "mid"]
    #[name = "Mid"]
    Mid,
    #[id = "side"]
    #[name = "Side"]
    Side,
}

impl From<ChannelParam> for ChannelMode {
    fn from(v: ChannelParam) -> Self {
        match v {
            ChannelParam::Stereo => ChannelMode::Stereo,
            ChannelParam::Mid => ChannelMode::Mid,
            ChannelParam::Side => ChannelMode::Side,
        }
    }
}

// ── Plugin params ──────────────────────────────────────────────────────────────

const BAND_DEFAULT_FREQS: [f32; 6] = [60.0, 180.0, 540.0, 1_600.0, 4_800.0, 12_000.0];

pub struct SixPack {
    params: Arc<SixPackParams>,
    bands: [BandState; 6],
    sample_rate: f32,
}

#[derive(Params)]
pub struct SixPackParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    #[id = "input"]
    pub input_gain: FloatParam,
    #[id = "output"]
    pub output_gain: FloatParam,
    #[id = "io_link"]
    pub io_link: BoolParam,
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "quality"]
    pub quality: EnumParam<Quality>,
    #[id = "drive"]
    pub drive: EnumParam<Drive>,
    #[id = "deemphasis"]
    pub deemphasis: BoolParam,

    #[nested(array, group = "Band")]
    pub bands: [BandParams; 6],
}

#[derive(Params)]
pub struct BandParams {
    #[id = "freq"]
    pub freq: FloatParam,
    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "q"]
    pub q: FloatParam,
    #[id = "algo"]
    pub algo: EnumParam<AlgoParam>,
    #[id = "channel"]
    pub channel: EnumParam<ChannelParam>,
    #[id = "enable"]
    pub enable: BoolParam,
}

fn make_band_params(slot: usize) -> BandParams {
    let freq_hz = BAND_DEFAULT_FREQS[slot];
    BandParams {
        freq: FloatParam::new(
            "Freq",
            freq_hz,
            FloatRange::Skewed {
                min: 20.0,
                max: 20_000.0,
                factor: FloatRange::skew_factor(-2.0),
            },
        )
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit(" Hz")
        .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
        .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

        gain: FloatParam::new(
            "Gain",
            0.0,
            FloatRange::Linear {
                min: 0.0,
                max: 18.0,
            },
        )
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit(" dB"),

        q: FloatParam::new(
            "Q",
            0.71,
            FloatRange::Skewed {
                min: 0.1,
                max: 10.0,
                factor: FloatRange::skew_factor(-1.0),
            },
        )
        .with_smoother(SmoothingStyle::Linear(20.0)),

        algo: EnumParam::new("Algo", AlgoParam::Tube),
        channel: EnumParam::new("Channel", ChannelParam::Stereo),
        enable: BoolParam::new("Enable", true),
    }
}

impl Default for SixPackParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_editor_state(),

            input_gain: FloatParam::new(
                "Input",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            output_gain: FloatParam::new(
                "Output",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            io_link: BoolParam::new("I/O Link", false),

            mix: FloatParam::new("Mix", 0.5, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),

            quality: EnumParam::new("Quality", Quality::Off),
            drive: EnumParam::new("Drive", Drive::Standard),
            deemphasis: BoolParam::new("De-Emphasis", true),

            bands: [
                make_band_params(0),
                make_band_params(1),
                make_band_params(2),
                make_band_params(3),
                make_band_params(4),
                make_band_params(5),
            ],
        }
    }
}

impl Default for SixPack {
    fn default() -> Self {
        Self {
            params: Arc::new(SixPackParams::default()),
            bands: [
                BandState::new(BAND_SHAPES[0]),
                BandState::new(BAND_SHAPES[1]),
                BandState::new(BAND_SHAPES[2]),
                BandState::new(BAND_SHAPES[3]),
                BandState::new(BAND_SHAPES[4]),
                BandState::new(BAND_SHAPES[5]),
            ],
            sample_rate: 48_000.0,
        }
    }
}

impl SixPack {
    fn recompute_band_coefs(&mut self) {
        let p = &self.params;
        for (i, band) in self.bands.iter_mut().enumerate() {
            let bp = &p.bands[i];
            band.shape = BAND_SHAPES[i];
            band.algo = bp.algo.value().into();
            band.mode = bp.channel.value().into();
            band.freq_hz = bp.freq.smoothed.next();
            band.q = bp.q.smoothed.next();
            band.gain_db = bp.gain.smoothed.next();
            band.enable = if bp.enable.value() { 1.0 } else { 0.0 };
            band.recompute_coefs(self.sample_rate);
        }
    }
}

impl Plugin for SixPack {
    const NAME: &'static str = "Six Pack";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0.1.0";
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

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _ctx: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.recompute_band_coefs();
        true
    }

    fn reset(&mut self) {
        for band in self.bands.iter_mut() {
            band.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = self.params.clone();

        // Read smoothed scalars at block start.
        let input_gain = p.input_gain.smoothed.next();
        let mix = p.mix.smoothed.next();
        let drive_k = p.drive.value().k();
        let deemph = p.deemphasis.value();
        let io_link = p.io_link.value();
        let output_gain = if io_link {
            // -input_gain in dB → invert linearly: 1 / input_gain
            if input_gain.abs() > 1e-12 {
                1.0 / input_gain
            } else {
                1.0
            }
        } else {
            p.output_gain.smoothed.next()
        };

        // Update per-band state every block (parameter automation/smoothing).
        self.recompute_band_coefs();

        let dry_amp = dry_amp(mix);
        let wet_amp = wet_amp(mix);

        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        // Stereo only.
        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (l_chan, r_chan) = channel_slices.split_at_mut(1);
        let l = &mut l_chan[0][..num_samples];
        let r = &mut r_chan[0][..num_samples];

        for i in 0..num_samples {
            let dry_l = l[i] * input_gain;
            let dry_r = r[i] * input_gain;

            let mut wet_l = 0.0f32;
            let mut wet_r = 0.0f32;
            let mut boost_l = 0.0f32;
            let mut boost_r = 0.0f32;
            for band in self.bands.iter_mut() {
                let out = band.process_sample(dry_l, dry_r, drive_k);
                wet_l += out.sat_l;
                wet_r += out.sat_r;
                boost_l += out.boost_l;
                boost_r += out.boost_r;
            }
            if deemph {
                wet_l -= boost_l;
                wet_r -= boost_r;
            }

            l[i] = (dry_amp * dry_l + wet_amp * wet_l) * output_gain;
            r[i] = (dry_amp * dry_r + wet_amp * wet_r) * output_gain;
        }

        // Sanity: NaN guard in debug builds.
        debug_assert!(l.iter().all(|s| s.is_finite()), "NaN in L output");
        debug_assert!(r.iter().all(|s| s.is_finite()), "NaN in R output");

        ProcessStatus::Normal
    }
}

impl ClapPlugin for SixPack {
    const CLAP_ID: &'static str = "com.mpd.six-pack";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("6-band parallel multiband saturator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Distortion,
    ];
}

impl Vst3Plugin for SixPack {
    const VST3_CLASS_ID: [u8; 16] = *b"mpdSixPack0001AB";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(SixPack);
nih_export_vst3!(SixPack);
