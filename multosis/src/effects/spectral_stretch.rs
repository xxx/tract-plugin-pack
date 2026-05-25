//! Spectral Stretch: phase-vocoder time stretching with chaos.
//!
//! Unlike the other spectral effects (which use the shared SpectralEngine at
//! 50% overlap), Stretch holds its own analyzer per FFT size at 75% overlap
//! (`hop = fft_size / 4`). Phase A (this commit) lays the scaffolding with an
//! IDENTITY-PASS transform so the EffectKind variant builds, registers, and
//! routes audio cleanly. Phase B fills in the phase-vocoder math (per-bin
//! phase advance scaled by Speed, Tempo throttle on re-analyze, Chaos
//! injected random phase).
//!
//! Each FFT-size slot pre-allocates its own analyzer + IFFT + output ring
//! AND its own per-bin phase state vectors (`last_input_phase`,
//! `accumulated_output_phase`), so switching FFT size in flight is
//! allocation-free.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use tract_dsp::stft_analysis::StftAnalyzer;

/// Same four FFT sizes selectable on every spectral effect.
const FFT_SIZES: [usize; 4] = [512, 1024, 2048, 4096];

/// One per-channel slot: analyzer + IFFT + output ring + per-bin phase state.
struct Slot {
    fft_size: usize,
    hop_size: usize,
    analyzer: StftAnalyzer,
    ifft: Arc<dyn Fft<f32>>,
    output_ring: Vec<f32>,
    output_pos: usize,
    hop_counter: usize,
    spectrum_scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
    /// Per-bin phase state -- allocated here so Phase B (vocoder math) can use
    /// them without re-allocating. Phase A leaves them zero; the identity
    /// transform doesn't touch phase.
    last_input_phase: Vec<f32>,
    accumulated_output_phase: Vec<f32>,
    /// Tempo throttle counter -- how many hops since the last analyze that
    /// fed the phase tracker. Phase A leaves it at zero (re-analyze every hop).
    analyze_throttle: u32,
}

impl Slot {
    fn new(fft_size: usize, planner: &mut FftPlanner<f32>) -> Self {
        let hop_size = fft_size / 4; // 75% overlap
        let ifft = planner.plan_fft_inverse(fft_size);
        let scratch_len = ifft.get_inplace_scratch_len();
        Self {
            fft_size,
            hop_size,
            analyzer: StftAnalyzer::new(fft_size, hop_size),
            ifft,
            output_ring: vec![0.0; fft_size],
            output_pos: 0,
            hop_counter: 0,
            spectrum_scratch: vec![Complex::default(); fft_size],
            ifft_scratch: vec![Complex::default(); scratch_len],
            last_input_phase: vec![0.0; fft_size],
            accumulated_output_phase: vec![0.0; fft_size],
            analyze_throttle: 0,
        }
    }

    fn reset(&mut self) {
        self.analyzer.reset();
        self.output_ring.fill(0.0);
        self.output_pos = 0;
        self.hop_counter = 0;
        self.last_input_phase.fill(0.0);
        self.accumulated_output_phase.fill(0.0);
        self.analyze_throttle = 0;
    }
}

/// Per-channel state: four pre-allocated FFT-size slots plus the active
/// index. Mirrors SpectralEngine's switching pattern at 75% overlap.
struct StretchChannel {
    slots: [Slot; 4],
    active: usize,
    pending: Option<usize>,
}

impl StretchChannel {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let slots = [
            Slot::new(FFT_SIZES[0], &mut planner),
            Slot::new(FFT_SIZES[1], &mut planner),
            Slot::new(FFT_SIZES[2], &mut planner),
            Slot::new(FFT_SIZES[3], &mut planner),
        ];
        Self {
            slots,
            active: 2,
            pending: None,
        }
    }

    fn set_fft_size(&mut self, fft_size: usize) {
        if let Some(idx) = FFT_SIZES.iter().position(|&s| s == fft_size) {
            if idx == self.active {
                self.pending = None;
            } else {
                self.pending = Some(idx);
            }
        }
    }

    fn latency_samples(&self) -> usize {
        self.slots[self.active].hop_size
    }

    fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.reset();
        }
        self.pending = None;
    }

    /// Drive one sample through the active slot. PHASE A: the per-hop
    /// transform is identity -- it just copies the analyzed spectrum into
    /// the IFFT input, runs the inverse, windows, and overlap-adds.
    fn process_sample(&mut self, input: f32, _params: ParamsCache, _sample_rate: f32) -> f32 {
        let slot = &mut self.slots[self.active];

        // Output read first.
        let out = slot.output_ring[slot.output_pos];
        slot.output_ring[slot.output_pos] = 0.0;
        slot.output_pos = (slot.output_pos + 1) % slot.fft_size;

        slot.analyzer.write(input);
        slot.hop_counter += 1;

        if slot.hop_counter >= slot.hop_size {
            slot.hop_counter = 0;

            let fft_size = slot.fft_size;
            let frame = slot.analyzer.analyze();

            // PHASE A: identity transform -- copy spectrum verbatim.
            // PHASE B will insert per-bin phase advance here.
            slot.spectrum_scratch.copy_from_slice(frame.spectrum);

            slot.ifft
                .process_with_scratch(&mut slot.spectrum_scratch, &mut slot.ifft_scratch);

            // 1/N normalisation + synthesis window + overlap-add.
            let inv_n = 1.0 / fft_size as f32;
            let synth = frame.synthesis_window;
            let pos = slot.output_pos;
            let n = slot.fft_size;
            for (i, (&w, c)) in synth.iter().zip(slot.spectrum_scratch.iter()).enumerate() {
                let ring_idx = (pos + i) % n;
                slot.output_ring[ring_idx] += c.re * inv_n * w;
            }

            // Apply pending FFT-size switch at hop boundary.
            if let Some(new_active) = self.pending.take() {
                self.active = new_active;
            }
        }

        out
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    speed: f32,
    tempo_pct: f32,
    chaos_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            speed: 1.0,
            tempo_pct: 100.0,
            chaos_pct: 0.0,
        }
    }
}

pub struct SpectralStretchEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: StretchChannel,
    chan_r: StretchChannel,
}

impl SpectralStretchEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "FFT",
            min: 0.0,
            max: 3.0,
            default: 2.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: &["512", "1024", "2048", "4096"],
            },
        },
        ParamSpec {
            name: "Speed",
            min: 0.25,
            max: 4.0,
            default: 1.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "x",
            },
        },
        ParamSpec {
            name: "Tempo",
            min: 1.0,
            max: 100.0,
            default: 100.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Chaos",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
    ];
}

impl Default for SpectralStretchEffect {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            params: ParamsCache::default(),
            chan_l: StretchChannel::new(),
            chan_r: StretchChannel::new(),
        }
    }
}

impl Effect for SpectralStretchEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let lo = self
            .chan_l
            .process_sample(left, self.params, self.sample_rate);
        let ro = self
            .chan_r
            .process_sample(right, self.params, self.sample_rate);
        (lo, ro)
    }
    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        // Rebuild channels at the new sample rate.
        self.chan_l = StretchChannel::new();
        self.chan_r = StretchChannel::new();
        let fft_size = FFT_SIZES[self.params.fft_param.round().clamp(0.0, 3.0) as usize];
        self.chan_l.set_fft_size(fft_size);
        self.chan_r.set_fft_size(fft_size);
    }
    fn reset(&mut self) {
        self.chan_l.reset();
        self.chan_r.reset();
    }
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.chan_l.set_fft_size(fft_size);
                self.chan_r.set_fft_size(fft_size);
            }
            1 => self.params.speed = value.clamp(0.25, 4.0),
            2 => self.params.tempo_pct = value.clamp(1.0, 100.0),
            3 => self.params.chaos_pct = value.clamp(0.0, 100.0),
            _ => {}
        }
    }
    fn latency_samples(&self) -> usize {
        self.chan_l.latency_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(e: &mut SpectralStretchEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralStretchEffect::default().parameters().len(), 4);
    }

    #[test]
    fn identity_passes_sine_phase_a() {
        // Phase A is identity-pass; output should reconstruct the input.
        let sr = 48_000.0_f32;
        let mut e = SpectralStretchEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        let f = 1000.0_f32;
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let warmup = 2 * e.latency_samples();
        let peak = out[warmup..].iter().cloned().fold(0.0_f32, f32::max);
        let trough = out[warmup..].iter().cloned().fold(0.0_f32, |a, x| a.min(x));
        let amp = (peak - trough) / 2.0;
        // 75% overlap Hann COLA should reconstruct within tighter than the
        // 3 dB tolerance the engine's 50% test used. Allow 3 dB for safety.
        assert!(
            amp > 0.708 && amp < 1.0 / 0.708,
            "identity sine amp {amp} outside +/- 3 dB"
        );
    }

    #[test]
    fn silence_in_silence_out_phase_a() {
        let mut e = SpectralStretchEffect::default();
        e.set_param(0, 1.0);
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
