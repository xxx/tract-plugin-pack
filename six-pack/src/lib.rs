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
use crate::oversampling::StereoOversampler;
use crate::saturation::Algorithm;
use crate::spectrum::SpectrumAnalyzer;

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
    os: StereoOversampler,
    max_block: usize,
    pub spectrum: SpectrumAnalyzer,
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
            os: StereoOversampler::new(),
            max_block: 1024,
            spectrum: SpectrumAnalyzer::new(rand_seed()),
        }
    }
}

fn rand_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

impl SixPack {
    fn recompute_band_coefs_for_os(&mut self, factor: usize) {
        let effective_sr = self.sample_rate * factor as f32;
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
            band.recompute_coefs(effective_sr);
        }
    }

    /// Backward-compatible wrapper used by tests that don't go through the
    /// oversampling path.
    #[cfg(test)]
    fn recompute_band_coefs(&mut self) {
        self.recompute_band_coefs_for_os(1);
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
        ctx: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.max_block = buffer_config.max_buffer_size as usize;
        let factor = self.params.quality.value().factor();
        self.os.set_factor(factor, self.max_block);
        self.recompute_band_coefs_for_os(factor);
        ctx.set_latency_samples(self.os.latency_samples() as u32);
        true
    }

    fn reset(&mut self) {
        for band in self.bands.iter_mut() {
            band.reset();
        }
        self.os.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let p = self.params.clone();

        // Handle Quality changes: re-allocate scratch (only at OS-factor boundary,
        // which is a rare user-initiated event), reset filter state to avoid
        // clicks, and report the new latency to the host.
        let new_factor = p.quality.value().factor();
        if new_factor != self.os.factor() {
            self.os.set_factor(new_factor, self.max_block);
            ctx.set_latency_samples(self.os.latency_samples() as u32);
            self.recompute_band_coefs_for_os(new_factor);
            for band in self.bands.iter_mut() {
                band.reset();
            }
        }

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
        // Use the OS-effective sample rate so SVF coefficients see the full
        // oversampled bandwidth.
        self.recompute_band_coefs_for_os(self.os.factor());

        let dry_amp_v = dry_amp(mix);
        let wet_amp_v = wet_amp(mix);

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

        // Apply input_gain in place at native rate.
        for s in l.iter_mut() {
            *s *= input_gain;
        }
        for s in r.iter_mut() {
            *s *= input_gain;
        }

        // Feed the spectrum analyzer at native rate (post-input-gain, pre-OS).
        let n = num_samples;
        for i in 0..n {
            let m = (l[i] + r[i]) * 0.5;
            self.spectrum.push_sample(m);
        }

        // Upsample to the OS scratch.
        let (os_l, os_r) = self.os.upsample_block(l, r);
        let len_os = os_l.len();

        // Per-oversampled-sample DSP loop.
        for i in 0..len_os {
            let dry_l = os_l[i];
            let dry_r = os_r[i];

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

            os_l[i] = dry_amp_v * dry_l + wet_amp_v * wet_l;
            os_r[i] = dry_amp_v * dry_r + wet_amp_v * wet_r;
        }

        // Downsample back to native rate.
        self.os.downsample_block(l, r);

        // Apply output_gain in place at native rate.
        for s in l.iter_mut() {
            *s *= output_gain;
        }
        for s in r.iter_mut() {
            *s *= output_gain;
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

#[cfg(test)]
mod plugin_tests {
    use super::*;

    /// All bands at 0 dB, mix ≤ 50%: output equals input (both channels)
    /// regardless of de-emphasis.
    #[test]
    fn all_bands_zero_db_passes_dry_at_low_mix() {
        for mix in [0.0_f32, 0.25, 0.5] {
            for deemph in [false, true] {
                let mut plugin = SixPack::default();
                plugin.sample_rate = 48_000.0;
                plugin.recompute_band_coefs();

                let mut wet_l_total: f32 = 0.0;
                let mut wet_r_total: f32 = 0.0;
                let mut dry_total: f32 = 0.0;
                let n = 4_000;
                for i in 0..n {
                    let phase = i as f32 / 100.0 * std::f32::consts::TAU;
                    let dry_l = phase.sin() * 0.3;
                    let dry_r = (phase * 0.7).cos() * 0.3;

                    let mut wet_l = 0.0;
                    let mut wet_r = 0.0;
                    let mut boost_l = 0.0;
                    let mut boost_r = 0.0;
                    for band in plugin.bands.iter_mut() {
                        let out = band.process_sample(dry_l, dry_r, 1.0);
                        wet_l += out.sat_l;
                        wet_r += out.sat_r;
                        boost_l += out.boost_l;
                        boost_r += out.boost_r;
                    }
                    if deemph {
                        wet_l -= boost_l;
                        wet_r -= boost_r;
                    }
                    let dry_a = super::dry_amp(mix);
                    let wet_a = super::wet_amp(mix);
                    let out_l = dry_a * dry_l + wet_a * wet_l;
                    let out_r = dry_a * dry_r + wet_a * wet_r;

                    wet_l_total += (out_l - dry_l).abs();
                    wet_r_total += (out_r - dry_r).abs();
                    dry_total += dry_l.abs() + dry_r.abs();
                }
                // Output must equal input within tight float epsilon.
                let normalized = (wet_l_total + wet_r_total) / dry_total.max(1e-9);
                assert!(
                    normalized < 1e-3,
                    "deemph={deemph}, mix={mix}: total deviation = {wet_l_total} + {wet_r_total} ({normalized} relative)"
                );
            }
        }
    }

    #[test]
    fn single_peak_at_9db_generates_harmonics() {
        use crate::bands::{BandState, ChannelMode, FilterShape};
        use crate::saturation::Algorithm;

        // One band at 1 kHz, +9 dB, Tube. Drive a 1 kHz sine through it; output
        // should contain meaningful energy at 2 kHz, 3 kHz, etc.
        let sr = 48_000.0;
        let mut band = BandState::new(FilterShape::Peak);
        band.algo = Algorithm::Tube;
        band.mode = ChannelMode::Stereo;
        band.freq_hz = 1_000.0;
        band.q = 0.71;
        band.gain_db = 9.0;
        band.recompute_coefs(sr);

        let n = 8192;
        let mut sat_signal = vec![0.0_f32; n];
        for i in 0..n {
            let phase = (i as f32) / sr * std::f32::consts::TAU * 1_000.0;
            let dry = phase.sin() * 0.5;
            let out = band.process_sample(dry, dry, 2.0); // drive=Crush
            sat_signal[i] = out.sat_l;
        }

        // Compute Goertzel-style energy at a few harmonic frequencies.
        let energy_at = |freq: f32| -> f32 {
            let mut acc = 0.0_f32;
            for i in 0..n {
                let p = (i as f32) / sr * std::f32::consts::TAU * freq;
                acc += sat_signal[i] * p.sin();
            }
            acc.abs() / (n as f32)
        };

        let e_fundamental = energy_at(1_000.0);
        let e_2nd = energy_at(2_000.0);
        let e_3rd = energy_at(3_000.0);

        // Tube is symmetric: odd harmonics dominate. 3rd should exceed 2nd by
        // some margin.
        assert!(e_3rd > 0.001, "3rd harmonic energy too low: {}", e_3rd);
        assert!(
            e_3rd > e_2nd * 0.5,
            "3rd should be comparable to 2nd: {} vs {}",
            e_3rd,
            e_2nd
        );
        assert!(
            e_fundamental > 0.001,
            "fundamental energy too low: {}",
            e_fundamental
        );
    }

    #[test]
    fn mix_curve_endpoints() {
        assert_eq!(super::dry_amp(0.0), 1.0);
        assert_eq!(super::wet_amp(0.0), 0.0);
        assert_eq!(super::dry_amp(0.5), 1.0);
        assert_eq!(super::wet_amp(0.5), 1.0);
        assert_eq!(super::dry_amp(1.0), 0.0);
        assert_eq!(super::wet_amp(1.0), 1.0);
    }

    #[test]
    fn mix_curve_monotone() {
        let mut prev_d = 1.0_f32;
        let mut prev_w = 0.0_f32;
        for i in 0..=100 {
            let m = i as f32 / 100.0;
            let d = super::dry_amp(m);
            let w = super::wet_amp(m);
            assert!(
                d <= prev_d + 1e-7,
                "dry_amp must be non-increasing: {} vs {}",
                d,
                prev_d
            );
            assert!(
                w >= prev_w - 1e-7,
                "wet_amp must be non-decreasing: {} vs {}",
                w,
                prev_w
            );
            prev_d = d;
            prev_w = w;
        }
    }

    /// Trivial-saturation limit: replace `Algorithm::apply` semantically by
    /// using `Digital` clip with a quiet input — the clip never triggers, so
    /// `saturate(x) == x`. With de-emph on, the boost cancels the wet.
    #[test]
    fn deemph_cancellation_per_channel_mode() {
        use crate::bands::{BandState, ChannelMode, FilterShape};
        use crate::saturation::Algorithm;
        let sr = 48_000.0;
        for mode in [ChannelMode::Stereo, ChannelMode::Mid, ChannelMode::Side] {
            let mut band = BandState::new(FilterShape::Peak);
            band.algo = Algorithm::Digital; // x.clamp(-1, 1)
            band.mode = mode;
            band.freq_hz = 1_000.0;
            band.q = 0.71;
            band.gain_db = 6.0;
            band.recompute_coefs(sr);

            for i in 0..200 {
                let phase = i as f32 / 200.0 * std::f32::consts::TAU;
                // Quiet input: |x| << 1, so digital clip never engages.
                let dry_l = phase.sin() * 0.05;
                let dry_r = (phase * 1.3).sin() * 0.05;
                let out = band.process_sample(dry_l, dry_r, 1.0);
                // wet_l - boost_l should be 0 (per-channel cancellation in trivial-sat).
                assert!(
                    (out.sat_l - out.boost_l).abs() < 1e-5,
                    "{:?} sat_l-boost_l mismatch: {} - {} = {}",
                    mode,
                    out.sat_l,
                    out.boost_l,
                    out.sat_l - out.boost_l
                );
                assert!(
                    (out.sat_r - out.boost_r).abs() < 1e-5,
                    "{:?} sat_r-boost_r mismatch: {} - {} = {}",
                    mode,
                    out.sat_r,
                    out.boost_r,
                    out.sat_r - out.boost_r
                );
            }
        }
    }
}
