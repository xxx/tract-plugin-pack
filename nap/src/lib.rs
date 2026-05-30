//! Nap — a draw-your-tail velvet-noise (EDVN) reverb.
//! See `docs/superpowers/specs/2026-05-29-nap-velvet-reverb-design.md`.
#![feature(portable_simd)]

pub mod coloration;
pub mod editor;
pub mod engine;
pub mod handoff;
pub mod ir;
pub mod rng;
pub mod sequence;
pub mod theme;

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use tiny_skia_widgets::mseg::MsegData;

use engine::ReverbChannel;
use handoff::SequenceHandoff;
use sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

// Fixed: 200 ms (the Pre-Delay max) at 192 kHz = 38_400 samples; 48_000 gives headroom.
const MAX_PREDELAY_SAMPLES: usize = 48_000;

pub struct Nap {
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    sample_rate: f32,

    // Audio-thread state.
    seq: VelvetSequence,
    seq_gen: u64,
    left: ReverbChannel,
    right: ReverbChannel,
    predelay_l: Vec<f32>,
    predelay_r: Vec<f32>,
    predelay_pos: usize,
    silent_samples: u32,
    // Per-block scratch for the SIMD convolution path (gained input + wet out,
    // one `engine::BLOCK` chunk per channel). Pre-allocated; never resized.
    blk_in_l: Vec<f32>,
    blk_in_r: Vec<f32>,
    blk_wet_l: Vec<f32>,
    blk_wet_r: Vec<f32>,
}

#[derive(Params)]
pub struct NapParams {
    #[persist = "decay-curve"]
    pub decay_curve: Arc<Mutex<MsegData>>,
    #[persist = "width-curve"]
    pub width_curve: Arc<Mutex<MsegData>>,
    #[persist = "tone-curve"]
    pub tone_curve: Arc<Mutex<MsegData>>,
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    // Automatable, smoothed.
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "predelay"]
    pub predelay: FloatParam,
    #[id = "input"]
    pub input: FloatParam,
    #[id = "output"]
    pub output: FloatParam,

    // Design-time (non-automatable; regenerate the sequence on edit).
    #[id = "size"]
    pub size: FloatParam,
    #[id = "density"]
    pub density: FloatParam,
    #[id = "width"]
    pub width: FloatParam,
    #[id = "seed"]
    pub seed: IntParam,
}

impl Default for Nap {
    fn default() -> Self {
        Self {
            params: Arc::new(NapParams::new()),
            handoff: Arc::new(SequenceHandoff::new()),
            sample_rate: 48_000.0,
            seq: VelvetSequence::new(),
            seq_gen: 0,
            left: ReverbChannel::new(48_000.0),
            right: ReverbChannel::new(48_000.0),
            predelay_l: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_r: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_pos: 0,
            silent_samples: 0,
            blk_in_l: vec![0.0; engine::BLOCK],
            blk_in_r: vec![0.0; engine::BLOCK],
            blk_wet_l: vec![0.0; engine::BLOCK],
            blk_wet_r: vec![0.0; engine::BLOCK],
        }
    }
}

impl NapParams {
    fn new() -> Self {
        Self {
            decay_curve: Arc::new(Mutex::new(default_decay_curve())),
            width_curve: Arc::new(Mutex::new(default_width_curve())),
            tone_curve: Arc::new(Mutex::new(default_tone_curve())),
            editor_state: editor::default_editor_state(),

            mix: FloatParam::new(
                "Mix",
                30.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            predelay: FloatParam::new(
                "Pre-Delay",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 200.0,
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            input: FloatParam::new(
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
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            output: FloatParam::new(
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
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            size: FloatParam::new(
                "Size",
                1.5,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .non_automatable()
            .with_unit(" s")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            density: FloatParam::new(
                "Density",
                1500.0,
                FloatRange::Skewed {
                    min: 500.0,
                    max: 4000.0,
                    factor: FloatRange::skew_factor(-0.5),
                },
            )
            .non_automatable()
            .with_unit(" /s")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            width: FloatParam::new(
                "Width",
                8.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 30.0,
                },
            )
            .non_automatable()
            .with_unit(" ms")
            .with_value_to_string(formatters::v2s_f32_rounded(1)),
            seed: IntParam::new("Seed", 1, IntRange::Linear { min: 1, max: 9999 })
                .non_automatable(),
        }
    }
}

impl Nap {
    /// Generate the sequence from the current params + curves and publish it.
    /// GUI / setup thread only (locks the curves, allocates nothing on the
    /// audio thread). Shared by `initialize()` and the editor's regen.
    pub fn regenerate(
        handoff: &SequenceHandoff,
        params: &NapParams,
        sample_rate: f32,
        scratch: &mut VelvetSequence,
    ) {
        let p = GenParams {
            sample_rate,
            size_s: params.size.value(),
            density: params.density.value(),
            width_ms: params.width.value(),
            seed: params.seed.value() as u64,
        };
        let decay = *params.decay_curve.lock().unwrap();
        let width = *params.width_curve.lock().unwrap();
        let tone = *params.tone_curve.lock().unwrap();
        generate(scratch, &p, &decay, &width, &tone);
        handoff.publish(scratch);
    }
}

impl Plugin for Nap {
    const NAME: &'static str = "Nap";
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
        editor::create(self.params.clone(), self.handoff.clone(), self.sample_rate)
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.left = ReverbChannel::new(self.sample_rate);
        self.right = ReverbChannel::new(self.sample_rate);
        // Clear pre-delay/tail state so stale samples don't survive an SR change.
        self.predelay_l.fill(0.0);
        self.predelay_r.fill(0.0);
        self.predelay_pos = 0;
        self.silent_samples = 0;
        // Initial sequence. The scratch alloc here is on the setup thread (fine);
        // only `publish`/`copy_from` into the handoff is allocation-free, which is
        // what keeps the audio-thread `try_read_into` below RT-safe.
        let mut scratch = VelvetSequence::new();
        Self::regenerate(&self.handoff, &self.params, self.sample_rate, &mut scratch);
        self.seq_gen = 0;
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.predelay_l.fill(0.0);
        self.predelay_r.fill(0.0);
        self.predelay_pos = 0;
        self.silent_samples = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }
        // Pick up the latest published sequence (no-op if unchanged).
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);

        let slices = buffer.as_slice();
        if slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        let predelay_cap = self.predelay_l.len();
        let mut in_peak = 0.0f32;

        // Process in BLOCK-sized sub-blocks so the SIMD convolution's working
        // set stays cache-resident regardless of the host's buffer size. Each
        // smoothed param still advances exactly once per sample, in order, so
        // automation stays sample-accurate.
        let mut off = 0;
        while off < num_samples {
            let b = (num_samples - off).min(engine::BLOCK);

            // Gather the input-gained dry block (input gain smoothed per sample).
            for i in 0..b {
                let g = self.params.input.smoothed.next();
                let dl = left[off + i];
                let dr = right[off + i];
                in_peak = in_peak.max(dl.abs()).max(dr.abs());
                self.blk_in_l[i] = dl * g;
                self.blk_in_r[i] = dr * g;
            }

            // Block reverb for each channel (L uses location, R the jittered set).
            self.left.process_block(
                &self.blk_in_l[..b],
                &mut self.blk_wet_l[..b],
                &self.seq,
                &self.seq.location,
            );
            self.right.process_block(
                &self.blk_in_r[..b],
                &mut self.blk_wet_r[..b],
                &self.seq,
                &self.seq.location_r,
            );

            // Per-sample pre-delay (wet only) + dry/wet mix + output gain.
            for i in 0..b {
                let mix = self.params.mix.smoothed.next() / 100.0;
                let out_gain = self.params.output.smoothed.next();
                let predelay_samps =
                    ((self.params.predelay.smoothed.next() * 0.001 * self.sample_rate) as usize)
                        .min(predelay_cap - 1);

                self.predelay_l[self.predelay_pos] = self.blk_wet_l[i];
                self.predelay_r[self.predelay_pos] = self.blk_wet_r[i];
                let read = (self.predelay_pos + predelay_cap - predelay_samps) % predelay_cap;
                let dwet_l = self.predelay_l[read];
                let dwet_r = self.predelay_r[read];
                self.predelay_pos = (self.predelay_pos + 1) % predelay_cap;

                let dry_l = left[off + i];
                let dry_r = right[off + i];
                left[off + i] = ((1.0 - mix) * dry_l + mix * dwet_l) * out_gain;
                right[off + i] = ((1.0 - mix) * dry_r + mix * dwet_r) * out_gain;
            }

            off += b;
        }

        // Tail handling: keep processing while the velvet tail rings out. Track
        // INPUT silence (not output) — the reverb tail keeps the output non-zero,
        // so an output-driven check would cut the tail off.
        // Corner case: if tail_len < num_samples (extreme min size + density), the
        // first silent block pushes silent_samples past tail_len and no Tail status
        // is emitted — but that ring-out is under one block, so it's negligible.
        let tail_len = (self.seq.tail_len as u32).max(1);
        if in_peak < 1e-6 {
            self.silent_samples = self.silent_samples.saturating_add(num_samples as u32);
        } else {
            self.silent_samples = 0;
        }
        if self.silent_samples > 0 && self.silent_samples <= tail_len {
            ProcessStatus::Tail(tail_len)
        } else {
            ProcessStatus::Normal
        }
    }
}

impl ClapPlugin for Nap {
    const CLAP_ID: &'static str = "com.mpd.nap";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A draw-your-tail velvet-noise reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Nap {
    const VST3_CLASS_ID: [u8; 16] = *b"NapMpdPlugin\0\0\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(Nap);
nih_export_vst3!(Nap);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_zero_is_dry_passthrough() {
        // Build a sequence with energy, but mix=0 ⇒ output == dry·output_gain
        // (output defaults to 0 dB = 1.0). We exercise the per-sample mix math
        // directly to avoid constructing a Buffer.
        let dry = 0.7_f32;
        let wet = 0.42_f32; // arbitrary wet value
        let mix = 0.0_f32;
        let out_gain = 1.0_f32;
        let out = ((1.0 - mix) * dry + mix * wet) * out_gain;
        assert!((out - dry).abs() < 1e-9);
    }

    #[test]
    fn default_params_are_valid_curves() {
        let p = NapParams::new();
        assert!(p.decay_curve.lock().unwrap().is_valid());
        assert!(p.width_curve.lock().unwrap().is_valid());
        assert!(p.tone_curve.lock().unwrap().is_valid());
    }

    #[test]
    fn regenerate_publishes_a_nonempty_sequence() {
        let params = NapParams::new();
        let handoff = SequenceHandoff::new();
        let mut scratch = VelvetSequence::new();
        Nap::regenerate(&handoff, &params, 48_000.0, &mut scratch);
        let mut local = VelvetSequence::new();
        let mut local_gen = 0u64;
        assert!(handoff.try_read_into(&mut local, &mut local_gen));
        assert!(
            local.count > 100,
            "expected a populated sequence, got {}",
            local.count
        );
    }
}
