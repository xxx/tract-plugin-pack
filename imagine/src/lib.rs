//! Imagine: multiband stereo imager modeled on Ozone Imager.
//!
//! Per-band signal flow:
//!   L,R → M/S encode → 4-band crossover (IIR or FIR by Quality)
//!       → per-band Width + Stereoize → recombine M_sum, S_sum
//!       → S_sum + recover · hilbert(S_removed_total)
//!       → M/S decode → L_out, R_out
//!
//! Solo bypasses the Recover Sides injection and uses the un-delayed band's
//! M_out, S_out directly so the user hears the band's own contribution.

#![feature(portable_simd)]

use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

pub mod bands;
pub mod crossover;
pub mod decorrelator;
pub mod hilbert;
pub mod midside;
pub mod spectrum;
pub mod vectorscope;

use crate::bands::{Band, StereoizeMode};
use crate::crossover::{CrossoverFir, CrossoverIir};
use crate::hilbert::HilbertFir;
use crate::spectrum::{Analyzer, SpectrumDisplay};
use crate::vectorscope::{ring_pair, VectorConsumer, VectorProducer};

// ── Constants ────────────────────────────────────────────────────────────────

pub const NUM_BANDS: usize = 4;
pub const FIR_CROSSOVER_LENGTH: usize = 511;
pub const FIR_HILBERT_LENGTH: usize = 65;
pub const HAAS_DEFAULT_MS: f32 = 12.0;
pub const HAAS_MAX_MS: f32 = 25.0;
pub const MAX_SAMPLE_RATE: f32 = 192_000.0;
pub const FIR_CROSSFADE_DEFAULT: usize = 1024;

// ── Param-side enums ─────────────────────────────────────────────────────────

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    #[id = "linear"]
    #[name = "Linear"]
    Linear,
    #[id = "iir"]
    #[name = "IIR"]
    Iir,
}

#[derive(Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StereoizeModeParam {
    #[id = "i"]
    #[name = "Mode I"]
    I,
    #[id = "ii"]
    #[name = "Mode II"]
    Ii,
}

impl From<StereoizeModeParam> for StereoizeMode {
    fn from(m: StereoizeModeParam) -> Self {
        match m {
            StereoizeModeParam::I => StereoizeMode::ModeI,
            StereoizeModeParam::Ii => StereoizeMode::ModeII,
        }
    }
}

// ── Per-band params ──────────────────────────────────────────────────────────

#[derive(Params)]
pub struct BandParams {
    #[id = "width"]
    pub width: FloatParam,
    #[id = "stz"]
    pub stz: FloatParam,
    #[id = "mode"]
    pub mode: EnumParam<StereoizeModeParam>,
    #[id = "solo"]
    pub solo: BoolParam,
}

fn make_band_params() -> BandParams {
    BandParams {
        width: FloatParam::new(
            "Width",
            0.0,
            FloatRange::Linear {
                min: -100.0,
                max: 100.0,
            },
        )
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_rounded(0)),

        stz: FloatParam::new(
            "Stereoize",
            0.0,
            FloatRange::Linear {
                min: 0.0,
                max: 100.0,
            },
        )
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit(" %")
        .with_value_to_string(formatters::v2s_f32_rounded(0)),

        mode: EnumParam::new("Mode", StereoizeModeParam::I),
        solo: BoolParam::new("Solo", false),
    }
}

// ── Plugin params ────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct ImagineParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<tiny_skia_widgets::EditorState>,
    /// Vectorscope display mode: 0 = Polar, 1 = Lissajous. Persisted across sessions.
    #[persist = "vector-mode"]
    pub vector_mode: Arc<AtomicU32>,

    #[nested(array, group = "Band")]
    pub bands: [BandParams; NUM_BANDS],

    #[id = "xover_1"]
    pub xover_1: FloatParam,
    #[id = "xover_2"]
    pub xover_2: FloatParam,
    #[id = "xover_3"]
    pub xover_3: FloatParam,

    #[id = "recover"]
    pub recover_sides: FloatParam,

    #[id = "link_bands"]
    pub link_bands: BoolParam,

    /// Quality is non-automatable: latency is set once at `initialize()`, and
    /// switching variants mid-stream would change the reported latency without
    /// the host re-querying. Users change it once when loading the plugin.
    #[id = "quality"]
    pub quality: EnumParam<Quality>,
}

impl Default for ImagineParams {
    fn default() -> Self {
        Self {
            editor_state: tiny_skia_widgets::EditorState::from_size(960, 640),
            vector_mode: Arc::new(AtomicU32::new(0)),

            bands: [
                make_band_params(),
                make_band_params(),
                make_band_params(),
                make_band_params(),
            ],

            xover_1: FloatParam::new(
                "Crossover 1",
                120.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

            xover_2: FloatParam::new(
                "Crossover 2",
                1_000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

            xover_3: FloatParam::new(
                "Crossover 3",
                8_000.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 20_000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(1))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

            recover_sides: FloatParam::new(
                "Recover Sides",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            link_bands: BoolParam::new("Link Bands", false),

            quality: EnumParam::new("Quality", Quality::Linear).non_automatable(),
        }
    }
}

// ── Throttled meter accumulator ──────────────────────────────────────────────

/// Block-windowed Pearson correlation + balance accumulator. Publishes to the
/// `correlation` and `balance` atomics every `WINDOW` samples and resets.
#[derive(Default)]
struct MeterAccum {
    sum_lr: f32,
    sum_ll: f32,
    sum_rr: f32,
    samples: usize,
}

impl MeterAccum {
    const WINDOW: usize = 1024;

    /// Push one (L, R) sample. Returns `Some((correlation, balance))` once the
    /// window has filled, after which the accumulator is reset.
    fn push(&mut self, l: f32, r: f32) -> Option<(f32, f32)> {
        self.sum_lr += l * r;
        self.sum_ll += l * l;
        self.sum_rr += r * r;
        self.samples += 1;
        if self.samples >= Self::WINDOW {
            let denom = (self.sum_ll * self.sum_rr).sqrt() + 1e-12;
            let correlation = (self.sum_lr / denom).clamp(-1.0, 1.0);
            let total = self.sum_ll + self.sum_rr + 1e-12;
            let balance = ((self.sum_rr - self.sum_ll) / total).clamp(-1.0, 1.0);
            *self = Self::default();
            Some((correlation, balance))
        } else {
            None
        }
    }
}

// ── Plugin struct ────────────────────────────────────────────────────────────

pub struct Imagine {
    params: Arc<ImagineParams>,

    // Crossovers (one pair per channel for both variants — IIR is filter
    // state, FIR is delay-line state; both are cheap enough to keep resident).
    crossover_iir_m: CrossoverIir,
    crossover_iir_s: CrossoverIir,
    crossover_fir_m: CrossoverFir,
    crossover_fir_s: CrossoverFir,

    bands: [Band; NUM_BANDS],

    /// Hilbert FIR for the recover-sides injection path. Adds
    /// `latency_samples()` of group delay, which the dry_delay below
    /// compensates for so the recover residue lines up phase-correctly with
    /// `M_sum + S_sum`.
    hilbert: HilbertFir,

    /// Delay line for `M_sum` aligned with the Hilbert injection latency.
    dry_delay_m: Vec<f32>,
    /// Delay line for `S_sum` aligned with the Hilbert injection latency.
    dry_delay_s: Vec<f32>,
    dry_delay_idx: usize,

    /// Throttled correlation/balance accumulator, published to `params.correlation` /
    /// `params.balance` every `MeterAccum::WINDOW` samples.
    meter_accumulator: MeterAccum,

    /// Vectorscope SPSC ring (audio-side producer). Allocated in `initialize`.
    vector_producer: Option<VectorProducer>,
    /// Consumer end stored on the plugin so `Plugin::editor()` (Task 12) can
    /// hand it to the editor. `Arc` because the editor takes ownership.
    pub vector_consumer: Option<Arc<VectorConsumer>>,

    /// FFT analyzer. Reset in `initialize` to update sample-rate-dependent log
    /// bin tables.
    spectrum: Option<Analyzer>,
    /// Display sink shared with the editor (Task 12).
    pub spectrum_display: Arc<SpectrumDisplay>,

    /// Lock-free correlation publish (audio thread → GUI). Bit pattern of f32.
    pub correlation: Arc<AtomicU32>,
    /// Lock-free balance publish (audio thread → GUI). Bit pattern of f32.
    pub balance: Arc<AtomicU32>,

    sample_rate: f32,

    /// Frozen at `initialize()` since Quality is non-automatable.
    active_quality: Quality,
}

impl Default for Imagine {
    fn default() -> Self {
        let hilbert = HilbertFir::new(FIR_HILBERT_LENGTH);
        let hilbert_lat = hilbert.latency_samples();
        Self {
            params: Arc::new(ImagineParams::default()),

            crossover_iir_m: CrossoverIir::default(),
            crossover_iir_s: CrossoverIir::default(),
            crossover_fir_m: CrossoverFir::new(FIR_CROSSOVER_LENGTH),
            crossover_fir_s: CrossoverFir::new(FIR_CROSSOVER_LENGTH),

            bands: std::array::from_fn(|_| Band::new(HAAS_MAX_MS, MAX_SAMPLE_RATE)),

            hilbert,
            dry_delay_m: vec![0.0; hilbert_lat],
            dry_delay_s: vec![0.0; hilbert_lat],
            dry_delay_idx: 0,

            meter_accumulator: MeterAccum::default(),

            vector_producer: None,
            vector_consumer: None,

            spectrum: None,
            spectrum_display: SpectrumDisplay::new(),

            correlation: Arc::new(AtomicU32::new(0)),
            balance: Arc::new(AtomicU32::new(0)),

            sample_rate: 48_000.0,
            active_quality: Quality::Linear,
        }
    }
}

impl Imagine {
    /// Total latency in samples for the active Quality variant.
    fn latency_samples_total(&self) -> u32 {
        let h = self.hilbert.latency_samples() as u32;
        match self.active_quality {
            Quality::Linear => self.crossover_fir_m.latency_samples() as u32 + h,
            Quality::Iir => h,
        }
    }

    /// Push (m_sum, s_sum) into the dry delay and read the value from
    /// `latency` samples ago. Standard read-then-write ring pattern.
    #[inline]
    fn dry_delay_step(&mut self, m_sum: f32, s_sum: f32) -> (f32, f32) {
        let n = self.dry_delay_m.len();
        if n == 0 {
            return (m_sum, s_sum);
        }
        let idx = self.dry_delay_idx;
        let m_d = self.dry_delay_m[idx];
        let s_d = self.dry_delay_s[idx];
        self.dry_delay_m[idx] = m_sum;
        self.dry_delay_s[idx] = s_sum;
        self.dry_delay_idx = if idx + 1 == n { 0 } else { idx + 1 };
        (m_d, s_d)
    }
}

impl Plugin for Imagine {
    const NAME: &'static str = "Imagine";
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
        // Editor wiring is Task 12.
        None
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        ctx: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.active_quality = self.params.quality.value();

        // Read non-smoothed initial crossover values (smoothers haven't run yet
        // at initialize time — `.value()` returns the persisted/default value).
        let f1 = self.params.xover_1.value();
        let f2 = self.params.xover_2.value();
        let f3 = self.params.xover_3.value();

        self.crossover_iir_m.redesign(f1, f2, f3, self.sample_rate);
        self.crossover_iir_s.redesign(f1, f2, f3, self.sample_rate);
        self.crossover_fir_m
            .initialize(f1, f2, f3, self.sample_rate);
        self.crossover_fir_s
            .initialize(f1, f2, f3, self.sample_rate);

        for band in &mut self.bands {
            band.set_sample_rate(self.sample_rate, HAAS_DEFAULT_MS);
            band.reset();
        }

        self.hilbert.reset();
        self.dry_delay_m.fill(0.0);
        self.dry_delay_s.fill(0.0);
        self.dry_delay_idx = 0;
        self.meter_accumulator = MeterAccum::default();

        // Rebuild the spectrum analyzer at the current sample rate so its
        // log-bin table is correct.
        self.spectrum = Some(Analyzer::new(
            self.sample_rate,
            self.spectrum_display.clone(),
        ));

        // Create the vectorscope ring fresh on every initialize so the GUI
        // doesn't see stale data after host buffer-size changes.
        let (prod, cons) = ring_pair();
        self.vector_producer = Some(prod);
        self.vector_consumer = Some(Arc::new(cons));

        ctx.set_latency_samples(self.latency_samples_total());
        true
    }

    fn reset(&mut self) {
        self.crossover_iir_m.reset();
        self.crossover_iir_s.reset();
        self.crossover_fir_m.reset();
        self.crossover_fir_s.reset();
        for band in &mut self.bands {
            band.reset();
        }
        self.hilbert.reset();
        self.dry_delay_m.fill(0.0);
        self.dry_delay_s.fill(0.0);
        self.dry_delay_idx = 0;
        self.meter_accumulator = MeterAccum::default();
        if let Some(spec) = &mut self.spectrum {
            spec.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        // ── Per-block param reads ────────────────────────────────────────
        let smoother_steps = num_samples as u32;
        let f1 = self.params.xover_1.smoothed.next_step(smoother_steps);
        let f2 = self.params.xover_2.smoothed.next_step(smoother_steps);
        let f3 = self.params.xover_3.smoothed.next_step(smoother_steps);

        // Redesign only the active crossover variant.
        match self.active_quality {
            Quality::Linear => {
                self.crossover_fir_m
                    .redesign(f1, f2, f3, self.sample_rate, FIR_CROSSFADE_DEFAULT);
                self.crossover_fir_s
                    .redesign(f1, f2, f3, self.sample_rate, FIR_CROSSFADE_DEFAULT);
            }
            Quality::Iir => {
                self.crossover_iir_m.redesign(f1, f2, f3, self.sample_rate);
                self.crossover_iir_s.redesign(f1, f2, f3, self.sample_rate);
            }
        }

        // Per-band smoothed Width / Stereoize / mode / solo.
        let mut widths = [0.0_f32; NUM_BANDS];
        let mut stz_amounts = [0.0_f32; NUM_BANDS];
        let mut modes = [StereoizeMode::ModeI; NUM_BANDS];
        let mut solos = [false; NUM_BANDS];
        for i in 0..NUM_BANDS {
            widths[i] = self.params.bands[i]
                .width
                .smoothed
                .next_step(smoother_steps);
            stz_amounts[i] = self.params.bands[i].stz.smoothed.next_step(smoother_steps);
            modes[i] = self.params.bands[i].mode.value().into();
            solos[i] = self.params.bands[i].solo.value();
        }

        let recover = self.params.recover_sides.smoothed.next_step(smoother_steps) * 0.01;

        // First-solo-wins (mirrors most DAWs' solo semantics).
        let mut any_solo = false;
        let mut solo_idx = 0_usize;
        for (i, &s) in solos.iter().enumerate() {
            if s {
                any_solo = true;
                solo_idx = i;
                break;
            }
        }

        // Stereo only.
        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (l_chan, r_chan) = channel_slices.split_at_mut(1);
        let l = &mut l_chan[0][..num_samples];
        let r = &mut r_chan[0][..num_samples];

        // ── Per-sample DSP loop ───────────────────────────────────────────
        for i in 0..num_samples {
            let l_in = l[i];
            let r_in = r[i];

            let (m, s) = midside::encode(l_in, r_in);

            // Crossover into 4 bands per channel.
            let (m_bands, s_bands) = match self.active_quality {
                Quality::Linear => (
                    self.crossover_fir_m.process(m),
                    self.crossover_fir_s.process(s),
                ),
                Quality::Iir => (
                    self.crossover_iir_m.process(m),
                    self.crossover_iir_s.process(s),
                ),
            };

            let mut m_outs = [0.0_f32; NUM_BANDS];
            let mut s_outs = [0.0_f32; NUM_BANDS];
            let mut m_sum = 0.0_f32;
            let mut s_sum = 0.0_f32;
            let mut s_removed_total = 0.0_f32;
            for b in 0..NUM_BANDS {
                let (m_o, s_o, s_rem) = self.bands[b].process(
                    m_bands[b],
                    s_bands[b],
                    widths[b],
                    stz_amounts[b],
                    modes[b],
                );
                m_outs[b] = m_o;
                s_outs[b] = s_o;
                m_sum += m_o;
                s_sum += s_o;
                s_removed_total += s_rem;
            }

            // Recover-sides: rotate the accumulated removed-sides by 90° and
            // mix it back. Because Hilbert adds latency, we must read M_sum,
            // S_sum from `latency` samples ago to align with the rotation.
            let (m_d, s_d) = self.dry_delay_step(m_sum, s_sum);
            let recover_inject = self.hilbert.process(s_removed_total) * recover;

            let (m_final, s_final) = if any_solo {
                // Solo bypasses Recover Sides — just hand the soloed band's
                // outs through. No dry_delay (the soloed band is *the* signal,
                // not a delayed sum).
                (m_outs[solo_idx], s_outs[solo_idx])
            } else {
                (m_d + recover_inject, s_d)
            };

            let (l_out, r_out) = midside::decode(m_final, s_final);
            l[i] = l_out;
            r[i] = r_out;

            // Display sinks.
            if let Some(prod) = &self.vector_producer {
                prod.push(l_out, r_out);
            }
            if let Some((correlation, balance)) = self.meter_accumulator.push(l_out, r_out) {
                self.correlation
                    .store(correlation.to_bits(), Ordering::Relaxed);
                self.balance.store(balance.to_bits(), Ordering::Relaxed);
            }
            if let Some(spec) = &mut self.spectrum {
                let (m_post, s_post) = midside::encode(l_out, r_out);
                spec.push(m_post, s_post);
            }
        }

        match self.active_quality {
            Quality::Linear => ProcessStatus::Tail(self.crossover_fir_m.latency_samples() as u32),
            Quality::Iir => ProcessStatus::Normal,
        }
    }
}

impl ClapPlugin for Imagine {
    const CLAP_ID: &'static str = "com.mpd.imagine";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Multiband stereo imager");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Imagine {
    const VST3_CLASS_ID: [u8; 16] = *b"mpdImagine0001AB";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Stereo];
}

nih_export_clap!(Imagine);
nih_export_vst3!(Imagine);

#[cfg(test)]
mod plugin_tests {
    use super::*;

    /// Drive the per-sample DSP loop by hand (no nih-plug Buffer mock).
    /// Mirrors `Plugin::process()` but with explicit param values rather than
    /// reading from smoothed params.
    #[allow(clippy::too_many_arguments)]
    fn run_sample_loop(
        plugin: &mut Imagine,
        l: &mut [f32],
        r: &mut [f32],
        widths: [f32; NUM_BANDS],
        stz_amounts: [f32; NUM_BANDS],
        modes: [StereoizeMode; NUM_BANDS],
        solos: [bool; NUM_BANDS],
        recover_norm: f32,
    ) {
        let mut any_solo = false;
        let mut solo_idx = 0_usize;
        for (i, &s) in solos.iter().enumerate() {
            if s {
                any_solo = true;
                solo_idx = i;
                break;
            }
        }
        let n = l.len();
        for i in 0..n {
            let (m, s) = midside::encode(l[i], r[i]);
            let (m_bands, s_bands) = match plugin.active_quality {
                Quality::Linear => (
                    plugin.crossover_fir_m.process(m),
                    plugin.crossover_fir_s.process(s),
                ),
                Quality::Iir => (
                    plugin.crossover_iir_m.process(m),
                    plugin.crossover_iir_s.process(s),
                ),
            };
            let mut m_outs = [0.0_f32; NUM_BANDS];
            let mut s_outs = [0.0_f32; NUM_BANDS];
            let mut m_sum = 0.0;
            let mut s_sum = 0.0;
            let mut s_removed_total = 0.0;
            for b in 0..NUM_BANDS {
                let (m_o, s_o, s_rem) = plugin.bands[b].process(
                    m_bands[b],
                    s_bands[b],
                    widths[b],
                    stz_amounts[b],
                    modes[b],
                );
                m_outs[b] = m_o;
                s_outs[b] = s_o;
                m_sum += m_o;
                s_sum += s_o;
                s_removed_total += s_rem;
            }
            let (m_d, s_d) = plugin.dry_delay_step(m_sum, s_sum);
            let recover_inject = plugin.hilbert.process(s_removed_total) * recover_norm;
            let (m_final, s_final) = if any_solo {
                (m_outs[solo_idx], s_outs[solo_idx])
            } else {
                (m_d + recover_inject, s_d)
            };
            let (l_o, r_o) = midside::decode(m_final, s_final);
            l[i] = l_o;
            r[i] = r_o;
        }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn sine(f: f32, n: usize, sr: f32) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin())
            .collect()
    }

    fn noise_seeded(seed: u32, n: usize) -> Vec<f32> {
        let mut state = seed;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    fn make_plugin(quality: Quality, sr: f32) -> Imagine {
        let mut p = Imagine::default();
        p.sample_rate = sr;
        p.active_quality = quality;
        p.crossover_iir_m.redesign(120.0, 1000.0, 8000.0, sr);
        p.crossover_iir_s.redesign(120.0, 1000.0, 8000.0, sr);
        p.crossover_fir_m.initialize(120.0, 1000.0, 8000.0, sr);
        p.crossover_fir_s.initialize(120.0, 1000.0, 8000.0, sr);
        for band in &mut p.bands {
            band.set_sample_rate(sr, HAAS_DEFAULT_MS);
            band.reset();
        }
        p.hilbert.reset();
        p
    }

    /// At default settings (Width=0, Stereoize=0, Recover=0) the plugin is
    /// transparent: output RMS ≈ input RMS.
    #[test]
    fn no_op_default_settings_passes_signal() {
        let sr = 48_000.0;
        let mut plugin = make_plugin(Quality::Iir, sr);
        let l_in = sine(1_000.0, 8_192, sr);
        let r_in = sine(1_000.0, 8_192, sr);
        let mut l = l_in.clone();
        let mut r = r_in.clone();
        run_sample_loop(
            &mut plugin,
            &mut l,
            &mut r,
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [StereoizeMode::ModeI; NUM_BANDS],
            [false; NUM_BANDS],
            0.0,
        );
        // Skip transient (IIR + hilbert).
        let skip = 512;
        let in_rms = rms(&l_in[skip..]);
        let out_rms = rms(&l[skip..]);
        assert!(
            (out_rms / in_rms - 1.0).abs() < 0.05,
            "ratio {:.4}",
            out_rms / in_rms
        );
    }

    /// All bands at Width=-100 → S_gain=0 → output should be mono (L ≈ R)
    /// after the Hilbert latency settles. We use Recover=0 to make the test
    /// deterministic: with Recover>0 the recover-rotated S would re-inject and
    /// the equality wouldn't hold.
    #[test]
    fn full_mono_at_minus_100_width_zeros_side() {
        let sr = 48_000.0;
        let mut plugin = make_plugin(Quality::Iir, sr);
        let l_in = noise_seeded(0xdead_beef, 4_096);
        let r_in = noise_seeded(0xfeed_face, 4_096);
        let mut l = l_in.clone();
        let mut r = r_in.clone();
        run_sample_loop(
            &mut plugin,
            &mut l,
            &mut r,
            [-100.0, -100.0, -100.0, -100.0],
            [0.0; NUM_BANDS],
            [StereoizeMode::ModeI; NUM_BANDS],
            [false; NUM_BANDS],
            0.0,
        );
        let skip = 512;
        for i in skip..l.len() {
            assert!(
                (l[i] - r[i]).abs() < 1e-3,
                "i={i} L={} R={} diff={}",
                l[i],
                r[i],
                l[i] - r[i]
            );
        }
    }

    /// Solo on band 1 with width=+50: output is finite and non-zero.
    #[test]
    fn solo_one_band_isolates() {
        let sr = 48_000.0;
        let mut plugin = make_plugin(Quality::Iir, sr);
        let l_in = noise_seeded(0xdead_beef, 4_096);
        let r_in = noise_seeded(0xfeed_face, 4_096);
        let mut l = l_in.clone();
        let mut r = r_in.clone();
        let mut widths = [0.0_f32; NUM_BANDS];
        widths[1] = 50.0;
        let mut solos = [false; NUM_BANDS];
        solos[1] = true;
        run_sample_loop(
            &mut plugin,
            &mut l,
            &mut r,
            widths,
            [0.0; NUM_BANDS],
            [StereoizeMode::ModeI; NUM_BANDS],
            solos,
            0.0,
        );
        let skip = 512;
        let mut total = 0.0_f32;
        for i in skip..l.len() {
            assert!(l[i].is_finite() && r[i].is_finite(), "i={i}");
            total += l[i].abs() + r[i].abs();
        }
        assert!(total > 0.1, "soloed output is silent: total {total}");
    }

    /// All widths positive → no S_removed → recover-amount has no effect.
    /// Output at Recover=100 must equal Recover=0 sample-for-sample.
    #[test]
    fn recover_sides_bypass_when_all_widths_positive() {
        let sr = 48_000.0;
        let l_in = noise_seeded(0xdead_beef, 4_096);
        let r_in = noise_seeded(0xfeed_face, 4_096);

        let mut p_a = make_plugin(Quality::Iir, sr);
        let mut l_a = l_in.clone();
        let mut r_a = r_in.clone();
        run_sample_loop(
            &mut p_a,
            &mut l_a,
            &mut r_a,
            [50.0, 50.0, 50.0, 50.0],
            [0.0; NUM_BANDS],
            [StereoizeMode::ModeI; NUM_BANDS],
            [false; NUM_BANDS],
            0.0,
        );

        let mut p_b = make_plugin(Quality::Iir, sr);
        let mut l_b = l_in.clone();
        let mut r_b = r_in.clone();
        run_sample_loop(
            &mut p_b,
            &mut l_b,
            &mut r_b,
            [50.0, 50.0, 50.0, 50.0],
            [0.0; NUM_BANDS],
            [StereoizeMode::ModeI; NUM_BANDS],
            [false; NUM_BANDS],
            1.0,
        );

        for i in 0..l_a.len() {
            assert!(
                (l_a[i] - l_b[i]).abs() < 1e-6,
                "L[{i}]: {} vs {}",
                l_a[i],
                l_b[i]
            );
            assert!(
                (r_a[i] - r_b[i]).abs() < 1e-6,
                "R[{i}]: {} vs {}",
                r_a[i],
                r_b[i]
            );
        }
    }

    #[test]
    fn meter_accum_publishes_after_window() {
        let mut acc = MeterAccum::default();
        // Push 1023 samples — should not emit.
        for _ in 0..1023 {
            assert!(acc.push(0.5, 0.5).is_none());
        }
        let (corr, bal) = acc.push(0.5, 0.5).expect("expected publish at WINDOW=1024");
        assert!((corr - 1.0).abs() < 1e-3, "corr {corr}");
        assert!(bal.abs() < 1e-3, "bal {bal}");
    }

    #[test]
    fn meter_accum_resets_between_windows() {
        let mut acc = MeterAccum::default();
        for _ in 0..MeterAccum::WINDOW {
            let _ = acc.push(0.5, 0.5);
        }
        // Now push an anti-correlated window.
        for _ in 0..(MeterAccum::WINDOW - 1) {
            assert!(acc.push(0.5, -0.5).is_none());
        }
        let (corr, _) = acc.push(0.5, -0.5).unwrap();
        assert!((corr + 1.0).abs() < 1e-3, "corr {corr} should be ≈ -1");
    }
}
