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

/// Drive selector. With de-emphasis on (the default), each setting produces
/// a structurally different effect — not just three intensities of the same
/// thing. The labels reflect the *character*:
///
/// - **Carve** (`k = 0.6`): the saturator runs gently so its linear-domain
///   contribution is below the boost; the de-emph subtract removes more
///   than the wet provides, leaving a phase-inverted residue that *carves
///   a notch* in the band's frequency range, plus quiet harmonics.
/// - **Color** (`k = 1.0`): clean cancellation. The wet path's linear part
///   is exactly the boost, so the subtract zeroes it. Output gets only the
///   saturator's harmonics — pure tonal coloring with no EQ shape.
/// - **Crush** (`k = 2.0`): the saturator is pushed hard; its linear part
///   exceeds the boost. The de-emph subtract under-cancels, so the boost
///   shape stays in the band *and* loud harmonics ride on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum Drive {
    #[id = "carve"]
    #[name = "Carve"]
    Carve,
    #[id = "color"]
    #[name = "Color"]
    Color,
    #[id = "crush"]
    #[name = "Crush"]
    Crush,
}

impl Drive {
    pub fn k(self) -> f32 {
        match self {
            Drive::Carve => 0.6,
            Drive::Color => 1.0,
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
    /// Spectrum of the wet path — what Six Pack is *adding* to the dry
    /// signal, sampled at native rate from inside the OS loop. Drives the
    /// "harmonics added" overlay so users can see the plugin's contribution
    /// even at low drive where the output spectrum looks indistinguishable
    /// from the input.
    pub spectrum_wet: SpectrumAnalyzer,
    /// Per-band post-saturation RMS, updated once per block. The GUI reads
    /// this lock-free to glow each band dot proportionally to how much
    /// harmonic content that band is currently producing.
    pub band_activity: Arc<[std::sync::atomic::AtomicU32; 6]>,
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
                max: 30.0,
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
            drive: EnumParam::new("Drive", Drive::Color),
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

/// Floor on the oversampler scratch capacity.
///
/// Two host behaviors force this to be generous:
/// 1. The standalone JACK backend reports `max_buffer_size == 0` to
///    `initialize()` — without a floor the scratches end up zero-length.
/// 2. Bitwig (and likely other VST3 hosts) can change the engine buffer
///    size at runtime via "IO buses or latency changed" restarts that do
///    NOT round-trip through `initialize()`, so the scratch we sized in
///    `initialize()` can later be undersized for a much larger block.
///
/// 16384 covers Bitwig's typical engine buffers (up to 8192) plus headroom
/// and uses ~64 KB of scratch per channel at factor=1, ~1 MB per channel at
/// factor=16. The `assert_process_allocs` feature forbids growing this
/// inside `process()`, so the floor must be set high once at construction.
const MIN_MAX_BLOCK: usize = 16384;

impl Default for SixPack {
    fn default() -> Self {
        let mut os = StereoOversampler::new();
        os.set_factor(1, MIN_MAX_BLOCK);
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
            os,
            max_block: MIN_MAX_BLOCK,
            spectrum: SpectrumAnalyzer::new(rand_seed()),
            spectrum_wet: SpectrumAnalyzer::new(rand_seed().wrapping_add(0x5A5A_5A5A)),
            band_activity: Arc::new(std::array::from_fn(|_| {
                std::sync::atomic::AtomicU32::new(0)
            })),
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
    /// Update each band's coefficients from the smoothed parameter values.
    ///
    /// `smoother_steps` is the number of native-rate samples the parameter
    /// smoothers should advance in this call. Pass the block size from
    /// `process()` so a 20 ms ramp settles in 20 ms wall-clock time;
    /// `Smoother::next()` only advances by 1 sample, which when called
    /// once per block makes the apparent ramp time `block_size`× longer.
    /// Pass 0 from `initialize()` (no advance, just read current value).
    fn recompute_band_coefs_for_os(&mut self, factor: usize, smoother_steps: u32) {
        let effective_sr = self.sample_rate * factor as f32;
        let p = &self.params;
        for (i, band) in self.bands.iter_mut().enumerate() {
            let bp = &p.bands[i];
            band.shape = BAND_SHAPES[i];
            band.algo = bp.algo.value().into();
            band.mode = bp.channel.value().into();
            band.freq_hz = if smoother_steps == 0 {
                bp.freq.value()
            } else {
                bp.freq.smoothed.next_step(smoother_steps)
            };
            band.q = if smoother_steps == 0 {
                bp.q.value()
            } else {
                bp.q.smoothed.next_step(smoother_steps)
            };
            band.gain_db = if smoother_steps == 0 {
                bp.gain.value()
            } else {
                bp.gain.smoothed.next_step(smoother_steps)
            };
            band.enable = if bp.enable.value() { 1.0 } else { 0.0 };
            band.recompute_coefs(effective_sr);
        }
    }

    /// Backward-compatible wrapper used by tests that don't go through the
    /// oversampling path.
    #[cfg(test)]
    fn recompute_band_coefs(&mut self) {
        self.recompute_band_coefs_for_os(1, 0);
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

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.spectrum.bins.clone(),
            self.spectrum_wet.bins.clone(),
            self.band_activity.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        ctx: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        // Some standalone backends report `max_buffer_size == 0`; fall back to
        // `MIN_MAX_BLOCK` so the oversampler scratch is never sized below a
        // realistic JACK period.
        self.max_block = (buffer_config.max_buffer_size as usize).max(MIN_MAX_BLOCK);
        let factor = self.params.quality.value().factor();
        self.os.set_factor(factor, self.max_block);
        // 0 step count: don't advance any smoothers, just take their current
        // value (defaults at first construction, persisted values on session
        // load).
        self.recompute_band_coefs_for_os(factor, 0);
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

        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }
        let smoother_steps = num_samples as u32;

        // Handle Quality changes: re-allocate scratch (only at OS-factor boundary,
        // which is a rare user-initiated event), reset filter state to avoid
        // clicks, and report the new latency to the host.
        let new_factor = p.quality.value().factor();
        if new_factor != self.os.factor() {
            self.os.set_factor(new_factor, self.max_block);
            ctx.set_latency_samples(self.os.latency_samples() as u32);
            // Don't advance smoothers here — the per-band recompute below
            // owns the per-block step.
            self.recompute_band_coefs_for_os(new_factor, 0);
            for band in self.bands.iter_mut() {
                band.reset();
            }
        }

        // Advance smoothers by the actual block size so 20–50 ms ramp times
        // settle in 20–50 ms wall-clock. `Smoother::next()` only advances
        // by 1 sample per call; calling it once per block makes the apparent
        // ramp time (block_size)× longer, so a dot drag back to 0 dB would
        // leave audible (and visible) residue for many seconds.
        let input_gain = p.input_gain.smoothed.next_step(smoother_steps);
        let mix = p.mix.smoothed.next_step(smoother_steps);
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
            p.output_gain.smoothed.next_step(smoother_steps)
        };

        // Update per-band state every block (parameter automation/smoothing).
        // Use the OS-effective sample rate so SVF coefficients see the full
        // oversampled bandwidth.
        self.recompute_band_coefs_for_os(self.os.factor(), smoother_steps);

        let dry_amp_v = dry_amp(mix);
        let wet_amp_v = wet_amp(mix);

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

        // Capture the OS factor before the upsample call returns mutable
        // refs into `self.os.scratch_*` — those refs hold the borrow open
        // through the rest of the loop, blocking any later self.os reads.
        let factor = self.os.factor().max(1);

        // Upsample to the OS scratch.
        let (os_l, os_r) = self.os.upsample_block(l, r);
        let len_os = os_l.len();

        // Per-band post-saturation sum-of-squares for the GUI activity glow.
        // Stored at block end; not used by the audio path itself.
        let mut band_sumsq = [0.0f32; 6];

        // Per-oversampled-sample DSP loop.
        for i in 0..len_os {
            let dry_l = os_l[i];
            let dry_r = os_r[i];

            let mut wet_l = 0.0f32;
            let mut wet_r = 0.0f32;
            let mut boost_l = 0.0f32;
            let mut boost_r = 0.0f32;
            for (b, band) in self.bands.iter_mut().enumerate() {
                let out = band.process_sample(dry_l, dry_r, drive_k);
                wet_l += out.sat_l;
                wet_r += out.sat_r;
                boost_l += out.boost_l;
                boost_r += out.boost_r;
                band_sumsq[b] += out.sat_l * out.sat_l + out.sat_r * out.sat_r;
            }
            if deemph {
                // Subtract the *linear EQ boost* (Spectre's original
                // formulation). At drive=Color (k=1.0) this is an exact
                // analytical cancellation, leaving only saturator harmonics.
                // At drive=Carve (k<1) the wet under-shoots boost and the
                // subtraction over-shoots → a notch is carved in the band.
                // At drive=Crush (k>1) the wet over-shoots boost and the
                // subtraction under-shoots → boost stays in plus harmonics.
                // The three drives become structurally different effects
                // rather than three intensities of the same effect; the
                // label set is named to reflect that.
                wet_l -= boost_l;
                wet_r -= boost_r;
            }

            // Feed the "harmonics added" spectrum at native rate. Sampling
            // every Nth OS sample without an explicit anti-alias filter is
            // intentional for a visualization — the polyphase chain has
            // already band-limited dry, and the saturator's harmonics live
            // mostly within native Nyquist; any tiny residual aliasing in
            // the display is not audible (the audio path stays oversampled).
            if i % factor == 0 {
                self.spectrum_wet.push_sample((wet_l + wet_r) * 0.5);
            }

            os_l[i] = dry_amp_v * dry_l + wet_amp_v * wet_l;
            os_r[i] = dry_amp_v * dry_r + wet_amp_v * wet_r;
        }

        // Publish per-band RMS for the GUI to read.
        if len_os > 0 {
            let inv = 1.0 / (2.0 * len_os as f32);
            for (b, sumsq) in band_sumsq.iter().enumerate() {
                let rms = (sumsq * inv).sqrt();
                self.band_activity[b].store(rms.to_bits(), std::sync::atomic::Ordering::Relaxed);
            }
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
    fn sample_rate_sweep_stable() {
        // Drive a chirp through the plugin at multiple sample rates; verify NaN-free.
        for sr in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
            let mut plugin = SixPack::default();
            plugin.sample_rate = sr;
            plugin.recompute_band_coefs_for_os(1, 0);
            // Set band 4 to peak +12 dB at 1 kHz (just to get harmonics)
            plugin.bands[3].gain_db = 12.0;
            plugin.bands[3].recompute_coefs(sr);
            for i in 0..(sr as usize / 10) {
                let phase = (i as f32) / sr * std::f32::consts::TAU;
                let dry = (phase * 1_000.0).sin() * 0.3;
                let mut wet_l = 0.0;
                let mut wet_r = 0.0;
                let mut boost_l = 0.0;
                let mut boost_r = 0.0;
                for band in plugin.bands.iter_mut() {
                    let out = band.process_sample(dry, dry, 1.0);
                    wet_l += out.sat_l;
                    wet_r += out.sat_r;
                    boost_l += out.boost_l;
                    boost_r += out.boost_r;
                }
                assert!(wet_l.is_finite(), "sr={sr} i={i} wet_l={wet_l}");
                assert!(wet_r.is_finite(), "sr={sr} i={i} wet_r={wet_r}");
                assert!(boost_l.is_finite() && boost_r.is_finite());
            }
        }
    }

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
