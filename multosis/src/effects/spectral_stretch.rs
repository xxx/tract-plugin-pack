//! Spectral Stretch: creative phase-vocoder pitch shifter with chaos.
//!
//! Unlike the other spectral effects (which use the shared SpectralEngine at
//! 50% overlap), Stretch holds its own analyzer per FFT size at 75% overlap
//! (`hop = fft_size / 4`). The synthesis loop captures source magnitudes and
//! per-bin phase deviations at Tempo-throttled boundaries, then remaps each
//! source bin k_src to target bin round(k_src * Speed) with max-magnitude
//! collisions, and finally synthesises a complex spectrum from
//! per-target-bin accumulated output phase. Speed therefore acts as a pitch
//! shifter in disguise; the name 'Stretch' tracks Infiltrator's creative
//! terminology rather than literal time-stretching.
//!
//! Each FFT-size slot pre-allocates its own analyzer + IFFT + output ring
//! AND its own per-bin phase + remap scratch (`last_input_phase`,
//! `accumulated_output_phase`, `captured_mags`, `captured_phase_dev`,
//! `remap_mag`, `remap_dev`), so switching FFT size in flight is
//! allocation-free.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;
use tract_dsp::stft_analysis::StftAnalyzer;

fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

fn wrap_pi(x: f32) -> f32 {
    // Wrap `x` into approximately [-pi, pi]. Range reduction via
    // x - TAU * round(x / TAU) is constant-time -- previously this was a
    // pair of while-loops, which for high-k bins iterated hundreds of times
    // (the input `phase - last_input_phase - expected` is dominated by
    // `expected = TAU * k * hop / n`, far outside [-pi, pi]). The while-loop
    // form was the top hot spot in the SpectralStretch profile (~50% of
    // total cycles).
    let tau = std::f32::consts::TAU;
    x - (x * (1.0 / tau)).round() * tau
}

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
    /// Captured magnitudes from the last re-analyze; the synthesizer reuses
    /// these between captures so Tempo<100% holds magnitude content longer.
    captured_mags: Vec<f32>,
    /// Per-bin phase deviation (true_freq - expected_per_hop) captured at
    /// the last re-analyze. The synthesizer reuses this between captures.
    captured_phase_dev: Vec<f32>,
    /// Per-target-bin scratch for Speed bin-remap: magnitude written to bin
    /// k_dst = round(k_src * speed), max-wins on collisions. Length is
    /// fft_size/2 + 1 (positive half-spectrum, inclusive of DC and Nyquist).
    remap_mag: Vec<f32>,
    /// Companion to remap_mag -- the phase deviation of the source bin that
    /// won the max-magnitude vote for each target bin.
    remap_dev: Vec<f32>,
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
            captured_mags: vec![0.0; fft_size],
            captured_phase_dev: vec![0.0; fft_size],
            remap_mag: vec![0.0; fft_size / 2 + 1],
            remap_dev: vec![0.0; fft_size / 2 + 1],
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
        self.captured_mags.fill(0.0);
        self.captured_phase_dev.fill(0.0);
        self.remap_mag.fill(0.0);
        self.remap_dev.fill(0.0);
        self.analyze_throttle = 0;
    }
}

/// Per-channel state: four pre-allocated FFT-size slots plus the active
/// index. Mirrors SpectralEngine's switching pattern at 75% overlap.
struct StretchChannel {
    slots: [Slot; 4],
    active: usize,
    pending: Option<usize>,
    rng_state: u32,
}

impl StretchChannel {
    fn new(seed: u32) -> Self {
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
            rng_state: seed.max(1),
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

    /// Drive one sample through the active slot. PHASE B: target-bin magnitude
    /// remap from source bin k_src = k_dst / speed (linear position, max-wins
    /// on collisions), then per-target-bin phase advance (`expected + dev`,
    /// NOT scaled by Speed -- the pitch shift comes from the remap, not from
    /// scaling the residual). Tempo throttles re-analysis; Chaos rotates each
    /// target bin's synthesized phase per hop.
    fn process_sample(&mut self, input: f32, params: ParamsCache, _sample_rate: f32) -> f32 {
        let slot = &mut self.slots[self.active];

        // Output read first.
        let out = slot.output_ring[slot.output_pos];
        slot.output_ring[slot.output_pos] = 0.0;
        slot.output_pos = (slot.output_pos + 1) % slot.fft_size;

        slot.analyzer.write(input);
        slot.hop_counter += 1;

        if slot.hop_counter >= slot.hop_size {
            slot.hop_counter = 0;

            let n = slot.fft_size;
            let half = n / 2;
            let hop = slot.hop_size;
            let speed = params.speed.clamp(0.25, 4.0);
            let chaos = (params.chaos_pct * 0.01).clamp(0.0, 1.0);
            let tempo = (params.tempo_pct * 0.01).clamp(0.01, 1.0);
            let analyze_period = (1.0 / tempo).round().max(1.0) as u32;

            // Always call analyze() to keep the analyzer's internal ring state
            // advancing; only USE its output at Tempo-throttled boundaries.
            let frame = slot.analyzer.analyze();
            slot.analyze_throttle = slot.analyze_throttle.wrapping_add(1);
            if slot.analyze_throttle >= analyze_period {
                slot.analyze_throttle = 0;
                let inv_n = 1.0 / n as f32;
                let tau_n = std::f32::consts::TAU * inv_n;
                for k in 0..=half {
                    let c = frame.spectrum[k];
                    let mag = (c.re * c.re + c.im * c.im).sqrt();
                    let phase = c.im.atan2(c.re);
                    let expected = tau_n * (k as f32) * (hop as f32);
                    let dev = wrap_pi(phase - slot.last_input_phase[k] - expected);
                    slot.captured_mags[k] = mag;
                    slot.captured_phase_dev[k] = dev;
                    slot.last_input_phase[k] = phase;
                }
            }

            // Bin remap: for each source bin k_src, write to target bin
            // round(k_src * speed). Max-magnitude-wins on collisions
            // (mirrors tract-dsp::spectral_shifter). Source out of range
            // is dropped; targets without a winning source stay zero.
            for k in 0..=half {
                slot.remap_mag[k] = 0.0;
                slot.remap_dev[k] = 0.0;
            }
            for k_src in 0..=half {
                let mag = slot.captured_mags[k_src];
                if mag <= 0.0 {
                    continue;
                }
                let k_dst_f = k_src as f32 * speed;
                let k_dst = k_dst_f.round() as i32;
                if k_dst < 0 || (k_dst as usize) > half {
                    continue;
                }
                let k_dst = k_dst as usize;
                if mag > slot.remap_mag[k_dst] {
                    slot.remap_mag[k_dst] = mag;
                    slot.remap_dev[k_dst] = slot.captured_phase_dev[k_src];
                }
            }

            // Synthesize from remap_mag + accumulated_output_phase. Phase
            // advance is per-TARGET-bin: expected_increment + remap_dev.
            let inv_n = 1.0 / n as f32;
            let tau_n = std::f32::consts::TAU * inv_n;
            for k in 0..=half {
                let expected = tau_n * (k as f32) * (hop as f32);
                let advance = expected + slot.remap_dev[k];
                slot.accumulated_output_phase[k] += advance;
                if slot.accumulated_output_phase[k].abs() > 1e6 {
                    slot.accumulated_output_phase[k] = wrap_pi(slot.accumulated_output_phase[k]);
                }
                let phase = if chaos > 1e-6 {
                    self.rng_state = xorshift(self.rng_state);
                    let r = (self.rng_state as f32) / (u32::MAX as f32) - 0.5;
                    slot.accumulated_output_phase[k] + chaos * std::f32::consts::TAU * r
                } else {
                    slot.accumulated_output_phase[k]
                };
                let mag = slot.remap_mag[k];
                slot.spectrum_scratch[k] = Complex::new(mag * phase.cos(), mag * phase.sin());
                // Hermitian mirror for the negative-frequency half.
                if k != 0 && k != half {
                    slot.spectrum_scratch[n - k] = slot.spectrum_scratch[k].conj();
                }
            }

            slot.ifft
                .process_with_scratch(&mut slot.spectrum_scratch, &mut slot.ifft_scratch);

            // 1/N normalisation + synthesis window + overlap-add.
            let synth = frame.synthesis_window;
            let pos = slot.output_pos;
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
        // FFT in the LAST slot so it isn't the first dial users grab to modulate.
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
    ];
}

impl Default for SpectralStretchEffect {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            params: ParamsCache::default(),
            chan_l: StretchChannel::new(0x5717_E001),
            chan_r: StretchChannel::new(0x5717_E002),
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
        self.chan_l = StretchChannel::new(0x5717_E001);
        self.chan_r = StretchChannel::new(0x5717_E002);
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
            0 => self.params.speed = value.clamp(0.25, 4.0),
            1 => self.params.tempo_pct = value.clamp(1.0, 100.0),
            2 => self.params.chaos_pct = value.clamp(0.0, 100.0),
            3 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.chan_l.set_fft_size(fft_size);
                self.chan_r.set_fft_size(fft_size);
            }
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
    fn default_params_preserve_sine_amplitude() {
        // Speed=1 + Tempo=100% + Chaos=0 reconstructs the sine amplitude
        // (phase is not preserved -- the output's phase relationship to the
        // input is arbitrary because accumulated_output_phase starts at 0).
        let sr = 48_000.0_f32;
        let mut e = SpectralStretchEffect::default();
        e.set_param(3, 1.0); // FFT = 1024 (slot 3)
        let f = 1000.0_f32;
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let warmup = 4 * e.latency_samples();
        let peak = out[warmup..].iter().cloned().fold(0.0_f32, f32::max);
        let trough = out[warmup..].iter().cloned().fold(0.0_f32, |a, x| a.min(x));
        let amp = (peak - trough) / 2.0;
        // Allow 6 dB tolerance -- phase vocoder is creative, not surgical.
        assert!(
            amp > 0.5 && amp < 2.0,
            "default-params sine amp {amp} outside +/- 6 dB"
        );
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralStretchEffect::default();
        e.set_param(3, 1.0); // FFT (slot 3)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn speed_half_lowers_output_pitch() {
        // Speed=0.5 remaps source bin k -> target bin round(k * 0.5), so a
        // 2 kHz input sine should produce most output energy near 1 kHz.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralStretchEffect::default();
        e.set_param(3, 1.0); // FFT = 1024 (slot 3)
        e.set_param(0, 0.5); // Speed = 0.5 (slot 0)
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / sr).sin()
        });
        let tail_start = 4 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_2k = (2000.0 * 2048.0 / sr).round() as usize;
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let e_2k: f32 = tail[bin_2k.saturating_sub(2)..=bin_2k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1k: f32 = tail[bin_1k.saturating_sub(2)..=bin_1k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_1k > e_2k,
            "Speed=0.5 should move 2k energy down toward 1k; got 1k={e_1k} 2k={e_2k}"
        );
    }
}
