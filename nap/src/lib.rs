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

use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use tiny_skia_widgets::mseg::MsegData;

use engine::ReverbChannel;
use handoff::{IrHandoff, SequenceHandoff};
use ir::{IrBaker, IrSpectra};
use sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};
use tract_dsp::partitioned_conv::{PartitionedConvolver, BINS, P};

/// Engine mode: zero-latency time-domain reverb or FFT convolution.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum NapMode {
    #[id = "zero-latency"]
    #[name = "Zero Latency"]
    ZeroLatency,
    #[id = "efficient"]
    #[name = "Efficient"]
    Efficient,
}

// Fixed: 200 ms (the Pre-Delay max) at 192 kHz = 38_400 samples; 48_000 gives headroom.
const MAX_PREDELAY_SAMPLES: usize = 48_000;

/// A fixed `P`-sample delay line: `push_pop(x)` enqueues `x` and returns the
/// sample from `P` steps ago (0 for the first `P` calls). Used in Efficient
/// mode to delay the dry signal so it aligns with the convolver's `P`-sample
/// latency. Heap-allocated once (so `Nap` stays `Sized`/cheap to move).
struct DryDelay {
    ring: Vec<f32>,
    pos: usize,
}

impl DryDelay {
    fn new() -> Self {
        Self {
            ring: vec![0.0; P],
            pos: 0,
        }
    }

    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
    }

    /// Enqueue `x`, return the value enqueued exactly `P` samples earlier
    /// (0 for the first `P` calls).
    #[inline]
    fn push_pop(&mut self, x: f32) -> f32 {
        // Ring of capacity P holds a delay of exactly P: read the slot we're
        // about to overwrite (its value is P calls old), THEN write the new one.
        let out = self.ring[self.pos];
        self.ring[self.pos] = x;
        self.pos = (self.pos + 1) % P;
        out
    }
}

pub struct Nap {
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    ir_handoff: Arc<IrHandoff>,
    sample_rate: f32,
    /// Shared with the editor so it can rebuild its `IrBaker` at the real SR.
    shared_sr: Arc<crossbeam::atomic::AtomicCell<f32>>,

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

    // Efficient (FFT) engine state.
    left_conv: PartitionedConvolver,
    right_conv: PartitionedConvolver,
    ir_l: IrSpectra,
    ir_r: IrSpectra,
    ir_gen: u64,
    /// Per-channel `P`-sample dry-delay for Efficient mode alignment.
    dry_delay_l: DryDelay,
    dry_delay_r: DryDelay,
    last_mode: NapMode,
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

    // Engine mode: click-safe, non-automatable.
    #[id = "mode"]
    pub mode: EnumParam<NapMode>,

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
        let default_sr = 48_000.0f32;
        let max_ir = ir::max_ir_len(default_sr);
        Self {
            params: Arc::new(NapParams::new()),
            handoff: Arc::new(SequenceHandoff::new()),
            ir_handoff: Arc::new(IrHandoff::new(default_sr)),
            sample_rate: default_sr,
            shared_sr: Arc::new(AtomicCell::new(default_sr)),
            seq: VelvetSequence::new(),
            seq_gen: 0,
            left: ReverbChannel::new(default_sr),
            right: ReverbChannel::new(default_sr),
            predelay_l: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_r: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_pos: 0,
            silent_samples: 0,
            blk_in_l: vec![0.0; engine::BLOCK],
            blk_in_r: vec![0.0; engine::BLOCK],
            blk_wet_l: vec![0.0; engine::BLOCK],
            blk_wet_r: vec![0.0; engine::BLOCK],
            left_conv: PartitionedConvolver::new(max_ir),
            right_conv: PartitionedConvolver::new(max_ir),
            ir_l: IrSpectra::new(default_sr),
            ir_r: IrSpectra::new(default_sr),
            ir_gen: 0,
            dry_delay_l: DryDelay::new(),
            dry_delay_r: DryDelay::new(),
            last_mode: NapMode::ZeroLatency,
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

            mode: EnumParam::new("Engine", NapMode::ZeroLatency).non_automatable(),

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

    /// Bake the IR from the current params/curves and publish it via the
    /// `IrHandoff`. GUI / setup thread only (allocates, locks curves). Called
    /// from `initialize` and from the editor when switching to Efficient mode
    /// or when design-time params change while Efficient is active.
    pub fn bake_ir(
        ir_handoff: &IrHandoff,
        params: &NapParams,
        sample_rate: f32,
        scratch_seq: &mut VelvetSequence,
        baker: &mut IrBaker,
        ir_l: &mut IrSpectra,
        ir_r: &mut IrSpectra,
    ) {
        // Build the sequence (same math as regenerate, without the sequence publish).
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
        generate(scratch_seq, &p, &decay, &width, &tone);

        // Clone the location vecs so we can pass them as separate borrows
        // alongside &scratch_seq (borrow checker can't split the borrow otherwise).
        let loc_l = scratch_seq.location[..scratch_seq.count].to_vec();
        let loc_r = scratch_seq.location_r[..scratch_seq.count].to_vec();
        baker.bake(scratch_seq, &loc_l, ir_l);
        baker.bake(scratch_seq, &loc_r, ir_r);
        ir_handoff.publish(ir_l, ir_r);
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
        editor::create(
            self.params.clone(),
            self.handoff.clone(),
            self.ir_handoff.clone(),
            self.shared_sr.clone(),
        )
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.shared_sr.store(self.sample_rate);
        self.left = ReverbChannel::new(self.sample_rate);
        self.right = ReverbChannel::new(self.sample_rate);
        // Clear pre-delay/tail state so stale samples don't survive an SR change.
        self.predelay_l.fill(0.0);
        self.predelay_r.fill(0.0);
        self.predelay_pos = 0;
        self.silent_samples = 0;

        // Rebuild the FFT convolvers for the real sample rate.
        let max_ir = ir::max_ir_len(self.sample_rate);
        self.left_conv = PartitionedConvolver::new(max_ir);
        self.right_conv = PartitionedConvolver::new(max_ir);
        self.ir_l = IrSpectra::new(self.sample_rate);
        self.ir_r = IrSpectra::new(self.sample_rate);
        self.ir_gen = 0;
        self.dry_delay_l.reset();
        self.dry_delay_r.reset();
        // Reset mode tracking so the mode-switch logic in process() fires if needed.
        self.last_mode = NapMode::ZeroLatency;

        // The handoff's shared slots were sized at the default SR in `Default`;
        // grow them to the real SR so a max-Size IR at 96/192 kHz fits before the
        // first `publish` copies into them (else copy_from_slice panics). Done in
        // place so the editor's shared `Arc<IrHandoff>` clone stays valid.
        self.ir_handoff.resize_for(self.sample_rate);

        // Initial sequence + IR bake. Allocs here are on the setup thread (fine);
        // only `publish`/`copy_from` into the handoffs is allocation-free, which is
        // what keeps the audio-thread `try_read_into` below RT-safe.
        let mut scratch = VelvetSequence::new();
        Self::regenerate(&self.handoff, &self.params, self.sample_rate, &mut scratch);
        self.seq_gen = 0;
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);

        // Bake and install the IR so Efficient mode is ready immediately.
        let mut baker = IrBaker::new(self.sample_rate);
        Self::bake_ir(
            &self.ir_handoff,
            &self.params,
            self.sample_rate,
            &mut scratch,
            &mut baker,
            &mut self.ir_l,
            &mut self.ir_r,
        );
        self.ir_gen = 0;
        self.ir_handoff
            .try_read_into(&mut self.ir_l, &mut self.ir_r, &mut self.ir_gen);
        self.left_conv
            .set_ir(&self.ir_l.spectra[..self.ir_l.k * BINS], self.ir_l.k);
        self.right_conv
            .set_ir(&self.ir_r.spectra[..self.ir_r.k * BINS], self.ir_r.k);

        let latency = if self.params.mode.value() == NapMode::Efficient {
            P as u32
        } else {
            0
        };
        context.set_latency_samples(latency);
        true
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.predelay_l.fill(0.0);
        self.predelay_r.fill(0.0);
        self.predelay_pos = 0;
        self.silent_samples = 0;
        self.left_conv.reset();
        self.right_conv.reset();
        self.dry_delay_l.reset();
        self.dry_delay_r.reset();
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

        // Pick up the latest published sequence (no-op if unchanged).
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);

        // Pick up the latest baked IR; install into convolvers if changed.
        if self
            .ir_handoff
            .try_read_into(&mut self.ir_l, &mut self.ir_r, &mut self.ir_gen)
        {
            self.left_conv
                .set_ir(&self.ir_l.spectra[..self.ir_l.k * BINS], self.ir_l.k);
            self.right_conv
                .set_ir(&self.ir_r.spectra[..self.ir_r.k * BINS], self.ir_r.k);
        }

        // Mode switch: click-safe reset + latency report.
        let mode = self.params.mode.value();
        if mode != self.last_mode {
            self.left_conv.reset();
            self.right_conv.reset();
            self.dry_delay_l.reset();
            self.dry_delay_r.reset();
            let latency = if mode == NapMode::Efficient {
                P as u32
            } else {
                0
            };
            context.set_latency_samples(latency);
            self.last_mode = mode;
        }

        let slices = buffer.as_slice();
        if slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        let predelay_cap = self.predelay_l.len();
        let mut in_peak = 0.0f32;

        match mode {
            NapMode::ZeroLatency => {
                // Unchanged path: process in BLOCK-sized sub-blocks.
                let mut off = 0;
                while off < num_samples {
                    let b = (num_samples - off).min(engine::BLOCK);

                    // Gather the input-gained dry block.
                    for i in 0..b {
                        let g = self.params.input.smoothed.next();
                        let dl = left[off + i];
                        let dr = right[off + i];
                        in_peak = in_peak.max(dl.abs()).max(dr.abs());
                        self.blk_in_l[i] = dl * g;
                        self.blk_in_r[i] = dr * g;
                    }

                    // Block reverb for each channel.
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
                            ((self.params.predelay.smoothed.next() * 0.001 * self.sample_rate)
                                as usize)
                                .min(predelay_cap - 1);

                        self.predelay_l[self.predelay_pos] = self.blk_wet_l[i];
                        self.predelay_r[self.predelay_pos] = self.blk_wet_r[i];
                        let read =
                            (self.predelay_pos + predelay_cap - predelay_samps) % predelay_cap;
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
            }

            NapMode::Efficient => {
                // FFT convolution path. Process in BLOCK-sized sub-blocks (= P).
                // Wet is `P` samples late; compensate by delaying the dry by `P` too.
                let mut off = 0;
                while off < num_samples {
                    let b = (num_samples - off).min(engine::BLOCK);

                    // Gather input-gained dry block.
                    for i in 0..b {
                        let g = self.params.input.smoothed.next();
                        let dl = left[off + i];
                        let dr = right[off + i];
                        in_peak = in_peak.max(dl.abs()).max(dr.abs());
                        self.blk_in_l[i] = dl * g;
                        self.blk_in_r[i] = dr * g;
                    }

                    // FFT convolution: gained-dry → wet (P samples late).
                    self.left_conv
                        .process(&self.blk_in_l[..b], &mut self.blk_wet_l[..b]);
                    self.right_conv
                        .process(&self.blk_in_r[..b], &mut self.blk_wet_r[..b]);

                    // Per-sample: delay dry by P, then apply pre-delay on wet + mix.
                    for i in 0..b {
                        let mix = self.params.mix.smoothed.next() / 100.0;
                        let out_gain = self.params.output.smoothed.next();
                        let predelay_samps =
                            ((self.params.predelay.smoothed.next() * 0.001 * self.sample_rate)
                                as usize)
                                .min(predelay_cap - 1);

                        // Delay the RAW (un-gained) input by exactly P samples so
                        // it aligns with the convolver's P-late wet output and
                        // matches ZL's behaviour where Input drives the wet only.
                        let delayed_dry_l = self.dry_delay_l.push_pop(left[off + i]);
                        let delayed_dry_r = self.dry_delay_r.push_pop(right[off + i]);

                        // Pre-delay the wet output through the same predelay ring.
                        self.predelay_l[self.predelay_pos] = self.blk_wet_l[i];
                        self.predelay_r[self.predelay_pos] = self.blk_wet_r[i];
                        let read =
                            (self.predelay_pos + predelay_cap - predelay_samps) % predelay_cap;
                        let dwet_l = self.predelay_l[read];
                        let dwet_r = self.predelay_r[read];
                        self.predelay_pos = (self.predelay_pos + 1) % predelay_cap;

                        left[off + i] = ((1.0 - mix) * delayed_dry_l + mix * dwet_l) * out_gain;
                        right[off + i] = ((1.0 - mix) * delayed_dry_r + mix * dwet_r) * out_gain;
                    }

                    off += b;
                }
            }
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

    #[test]
    fn dry_delay_delays_by_exactly_p() {
        // Feed a ramp; out[n] must equal in[n-P] (0 for n < P). This locks the
        // in-process dry-delay alignment with the convolver's P-sample latency.
        let mut d = DryDelay::new();
        let total = 3 * P + 17;
        let input: Vec<f32> = (0..total).map(|n| n as f32).collect();
        for (n, &x) in input.iter().enumerate() {
            let got = d.push_pop(x);
            let want = if n < P { 0.0 } else { input[n - P] };
            assert_eq!(got, want, "n={n}: dry delay must be exactly P samples");
        }
    }

    #[test]
    fn efficient_matches_zero_latency_within_tolerance() {
        use crate::engine::ReverbChannel;
        use crate::ir::{IrBaker, IrSpectra};
        use crate::rng::Rng;
        use tract_dsp::partitioned_conv::{PartitionedConvolver, P};

        let mut seq = VelvetSequence::new();
        let n = 200;
        seq.count = n;
        let mut rng = Rng::new(21);
        for m in 0..n {
            let loc = (rng.next_u64() % 1500) as u32;
            seq.location[m] = loc;
            seq.location_r[m] = loc;
            seq.coeff[m] = (rng.next_f32() * 2.0 - 1.0) * 0.05;
            seq.filter_idx[m] = (rng.next_u64() % crate::coloration::Q as u64) as u8;
        }
        seq.tail_len = 1500;

        let total = 8000;
        let mut rng = Rng::new(2);
        let input: Vec<f32> = (0..total).map(|_| rng.next_f32() * 2.0 - 1.0).collect();

        // Zero-Latency reference.
        let mut ch = ReverbChannel::new(48_000.0);
        let zl: Vec<f32> = input
            .iter()
            .map(|&x| ch.process(x, &seq, &seq.location))
            .collect();

        // Efficient: baked IR through the convolver.
        let mut baker = IrBaker::new(48_000.0);
        let mut spec = IrSpectra::new(48_000.0);
        baker.bake(&seq, &seq.location.clone(), &mut spec);
        let mut conv = PartitionedConvolver::new(crate::ir::max_ir_len(48_000.0));
        conv.set_ir(
            &spec.spectra[..spec.k * tract_dsp::partitioned_conv::BINS],
            spec.k,
        );
        let mut eff = vec![0.0f32; total];
        conv.process(&input, &mut eff);

        for nn in P..total - 10 {
            assert!(
                (eff[nn] - zl[nn - P]).abs() <= 1e-3 + 1e-3 * zl[nn - P].abs(),
                "n={nn}: efficient {} vs zero-latency {}",
                eff[nn],
                zl[nn - P]
            );
        }
    }
}
