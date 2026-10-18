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

    /// Algorithmic latency in samples -- one full FFT window (the STFT-OLA
    /// standard), reported for the slot the engine *will* be using once a
    /// pending switch settles. Mirrors `tract_dsp::SpectralEngine`:
    ///
    /// * report `fft_size`, not `hop_size` -- the OLA output ring is a full
    ///   window deep (the old `hop_size` under-reported PDC by 4x);
    /// * report the *pending* slot when a switch is latched, so the host gets
    ///   the final latency immediately instead of the pre-switch value;
    ///
    /// Both matter because the channel constructs at the default slot
    /// (`active = 2`) and `set_effects` then latches the user's saved FFT as a
    /// `pending` switch. Reporting `active.hop_size` made `latency_samples`
    /// change the moment the switch applied at the next hop -- a moving PDC
    /// value that Bitwig chased with repeated `Plugin::reset` / transport
    /// re-aligns, zeroing every MSEG phase many times a second.
    fn latency_samples(&self) -> usize {
        let idx = self.pending.unwrap_or(self.active);
        self.slots[idx].fft_size
    }

    fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.reset();
        }
        // `pending` / `active` encode the user's FFT-size selection (intent),
        // not audio accumulator state, so they survive a reset. Clearing
        // `pending` here would let `latency_samples` swing back to the default
        // slot whenever the host calls reset -- the Bitwig PDC feedback loop
        // (see tract-dsp SpectralEngine::reset, f6d92f5).
    }

    /// Drive one sample through the active slot. PHASE B: target-bin magnitude
    /// remap from source bin k_src = k_dst / speed (linear position, max-wins
    /// on collisions), then per-target-bin phase advance (`expected + dev`,
    /// NOT scaled by Speed -- the pitch shift comes from the remap, not from
    /// scaling the residual). Tempo throttles re-analysis; Chaos rotates each
    /// target bin's synthesized phase per hop.
    fn process_sample(&mut self, input: f32, params: ParamsCache, _sample_rate: f32) -> f32 {
        let slot = &mut self.slots[self.active];

        // Output read first. `fft_size` is power-of-2 (one of FFT_SIZES) so
        // the wrap is a bitmask -- avoids a per-sample modulo in the hot path.
        let out = slot.output_ring[slot.output_pos];
        slot.output_ring[slot.output_pos] = 0.0;
        slot.output_pos = (slot.output_pos + 1) & (slot.fft_size - 1);

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

            // 1/N normalisation + synthesis window + overlap-add. The write
            // starts at `pos` and wraps; split the ring at `pos` so each of
            // the two contiguous halves is mod-free and LLVM can vectorise.
            let synth = frame.synthesis_window;
            let pos = slot.output_pos;
            let first = n - pos;
            let (ring_head, ring_tail) = slot.output_ring.split_at_mut(pos);
            let (synth_a, synth_b) = synth.split_at(first);
            let (scratch_a, scratch_b) = slot.spectrum_scratch.split_at(first);
            for ((dst, &w), c) in ring_tail.iter_mut().zip(synth_a).zip(scratch_a) {
                *dst += c.re * inv_n * w;
            }
            for ((dst, &w), c) in ring_head.iter_mut().zip(synth_b).zip(scratch_b) {
                *dst += c.re * inv_n * w;
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

    fn configured(fft: f32, chaos: f32) -> SpectralStretchEffect {
        let mut effect = SpectralStretchEffect::default();
        for (slot, value) in [0.75, 25.0, chaos, fft].into_iter().enumerate() {
            effect.set_param(slot, value);
        }
        effect
    }

    fn input_at(sample: usize) -> (f32, f32) {
        let t = sample as f32;
        (
            0.25 * (t * 0.071).sin() + 0.08 * (t * 0.73).cos(),
            -0.18 * (t * 0.13).cos() + 0.06 * (t * 1.17).sin(),
        )
    }

    fn run_hop(channel: &mut StretchChannel, params: ParamsCache) {
        let hop = channel.slots[channel.active].hop_size;
        for _ in 0..hop {
            channel.process_sample(0.0, params, 48_000.0);
        }
    }

    fn assert_near(actual: f32, expected: f64, tolerance: f64) {
        assert!(
            (actual as f64 - expected).abs() < tolerance,
            "{actual} vs {expected}, tolerance {tolerance}"
        );
    }

    #[test]
    fn xorshift_matches_known_vectors_and_repairs_zero_seeds() {
        let mut state = 1;
        for expected in [
            0x0004_2021,
            0x0408_0601,
            0x9DCC_A8C5,
            0x1255_994F,
            0x8EF9_17D1,
            0x2C6F_5BD0,
            0x25B2_331A,
            0x19F9_1CB2,
            0x7787_7125,
            0xADD0_2374,
            0x9E60_02CB,
            0x591C_9737,
        ] {
            state = xorshift(state);
            assert_eq!(state, expected);
        }
        assert_eq!(xorshift(0), 1);
        assert_eq!(StretchChannel::new(0).rng_state, 1);
    }

    #[test]
    fn phase_wrap_preserves_angles_and_returns_a_principal_phase() {
        for value in [
            0.0,
            0.25,
            -0.25,
            std::f32::consts::PI,
            -std::f32::consts::PI,
            std::f32::consts::TAU + 0.25,
            -std::f32::consts::TAU - 0.25,
            1_234.5,
            -1_234.5,
        ] {
            let actual = wrap_pi(value);
            assert!(actual.abs() <= std::f32::consts::PI + 2e-4);
            assert_near(actual.cos(), (value as f64).cos(), 2e-4);
            assert_near(actual.sin(), (value as f64).sin(), 2e-4);
        }
    }

    #[test]
    fn held_bin_remapping_and_overlap_add_match_an_analytic_real_idft() {
        for (index, size) in FFT_SIZES.into_iter().enumerate() {
            let half = size / 2;
            for speed in [0.25, 0.5, 1.0, 2.0, 4.0] {
                let (sources, winners) = match speed {
                    0.25 => (
                        vec![
                            (0, 0.03, 0.0),
                            (1, 0.02, 0.25),
                            (2, 0.06, -0.25),
                            (3, 0.04, 0.375),
                            (4, 0.04, -0.5),
                            (half, 0.05, 0.2),
                        ],
                        vec![(0, 0), (1, 2), (half / 4, 5)],
                    ),
                    0.5 => (
                        vec![
                            (0, 0.125, 0.0),
                            (1, 0.03, 0.125),
                            (2, 0.06, -0.25),
                            (3, 0.04, 0.375),
                            (4, 0.04, -0.5),
                            (half, 0.05, 0.2),
                        ],
                        vec![(0, 0), (1, 2), (2, 3), (half / 2, 5)],
                    ),
                    1.0 => (
                        vec![(0, 0.125, 0.0), (3, 0.06, 0.25), (half, 0.05, -0.125)],
                        vec![(0, 0), (3, 1), (half, 2)],
                    ),
                    2.0 => (
                        vec![
                            (0, 0.05, 0.0),
                            (3, 0.03, -0.375),
                            (half / 2, 0.04, 0.25),
                            (half, 0.1, -0.25),
                        ],
                        vec![(0, 0), (6, 1), (half, 2)],
                    ),
                    _ => (
                        vec![
                            (0, 0.05, -0.1),
                            (half / 4, 0.07, 0.25),
                            (half / 4 + 1, 0.1, -0.375),
                            (half, 0.2, 0.5),
                        ],
                        vec![(0, 0), (half, 1)],
                    ),
                };
                let mut channel = StretchChannel::new(1);
                channel.active = index;
                let slot = &mut channel.slots[index];
                for &(bin, magnitude, deviation) in &sources {
                    slot.captured_mags[bin] = magnitude * size as f32;
                    slot.captured_phase_dev[bin] = deviation;
                }
                for bin in 0..=half {
                    slot.accumulated_output_phase[bin] = (bin % 7) as f32 * 0.125;
                }
                let old_phases = slot.accumulated_output_phase.clone();
                for (sample, output) in slot.output_ring.iter_mut().enumerate() {
                    *output = 0.1 + sample as f32 * 0.0001;
                }
                let old_output = slot.output_ring.clone();
                let params = ParamsCache {
                    speed,
                    tempo_pct: 1.0,
                    ..ParamsCache::default()
                };
                let hop = size / 4;
                for &expected in &old_output[..hop] {
                    assert_eq!(channel.process_sample(0.0, params, 48_000.0), expected);
                }
                let slot = &channel.slots[index];
                assert_eq!(slot.output_pos, hop);
                for sample in 0..size {
                    let mut wave = 0.0;
                    for &(destination, source) in &winners {
                        let (_, magnitude, deviation) = sources[source];
                        let phase = old_phases[destination] as f64
                            + std::f64::consts::TAU * destination as f64 / 4.0
                            + deviation as f64;
                        let angle = phase
                            + std::f64::consts::TAU * destination as f64 * sample as f64
                                / size as f64;
                        let multiplier = if destination == 0 || destination == half {
                            1.0
                        } else {
                            2.0
                        };
                        wave += multiplier * magnitude as f64 * angle.cos();
                    }
                    let window = (0.5
                        - 0.5 * (std::f64::consts::TAU * sample as f64 / size as f64).cos())
                        / 1.5;
                    let position = (hop + sample) % size;
                    let carry = if position < hop {
                        0.0
                    } else {
                        old_output[position] as f64
                    };
                    assert_near(slot.output_ring[position], carry + wave * window, 2e-4);
                }
            }
        }
    }

    fn drive(e: &mut SpectralStretchEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn captured_hann_spectra_match_analytic_tones_and_hold_between_tempo_boundaries() {
        for (index, size) in FFT_SIZES.into_iter().enumerate() {
            let mut channel = StretchChannel::new(1);
            channel.active = index;
            let params = ParamsCache::default();
            for sample in 0..size + size / 4 {
                let input = 0.25
                    + 0.5 * (std::f64::consts::TAU * 7.0 * sample as f64 / size as f64 + 0.3).cos()
                    + if sample.is_multiple_of(2) {
                        0.125
                    } else {
                        -0.125
                    };
                channel.process_sample(input as f32, params, 48_000.0);
            }
            let slot = &channel.slots[index];
            for (bin, magnitude, phase, deviation) in [
                (0, 0.125, 0.0, 0.0),
                (
                    1,
                    0.0625,
                    std::f64::consts::PI,
                    -std::f64::consts::FRAC_PI_2,
                ),
                (
                    6,
                    0.0625,
                    0.3 + 7.0 * std::f64::consts::FRAC_PI_2 + std::f64::consts::PI,
                    std::f64::consts::FRAC_PI_2,
                ),
                (7, 0.125, 0.3 + 7.0 * std::f64::consts::FRAC_PI_2, 0.0),
                (
                    8,
                    0.0625,
                    0.3 + 7.0 * std::f64::consts::FRAC_PI_2 + std::f64::consts::PI,
                    -std::f64::consts::FRAC_PI_2,
                ),
                (
                    size / 2 - 1,
                    0.03125,
                    std::f64::consts::PI,
                    std::f64::consts::FRAC_PI_2,
                ),
                (size / 2, 0.0625, 0.0, 0.0),
            ] {
                assert_near(slot.captured_mags[bin] / size as f32, magnitude, 2e-6);
                assert_near(slot.last_input_phase[bin].cos(), phase.cos(), 2e-5);
                assert_near(slot.last_input_phase[bin].sin(), phase.sin(), 2e-5);
                assert_near(slot.captured_phase_dev[bin], deviation, 4e-4);
            }
            let magnitudes = slot.captured_mags.clone();
            let deviations = slot.captured_phase_dev.clone();
            let phases = slot.last_input_phase.clone();
            let held = ParamsCache {
                tempo_pct: 25.0,
                ..params
            };
            for hop in 1..=4 {
                run_hop(&mut channel, held);
                let slot = &channel.slots[index];
                if hop < 4 {
                    assert_eq!(slot.captured_mags, magnitudes);
                    assert_eq!(slot.captured_phase_dev, deviations);
                    assert_eq!(slot.last_input_phase, phases);
                } else {
                    assert!(
                        slot.captured_mags[..=size / 2]
                            .iter()
                            .all(|&mag| mag == 0.0)
                    );
                }
            }
        }
    }

    #[test]
    fn tempo_rounding_captures_on_the_nth_hop_and_holds_until_the_next_boundary() {
        for (tempo, period) in [
            (1.0, 100),
            (20.0, 5),
            (25.0, 4),
            (40.0, 3),
            (40.01, 2),
            (50.0, 2),
            (100.0, 1),
        ] {
            let mut channel = StretchChannel::new(1);
            channel.active = 0;
            let params = ParamsCache {
                tempo_pct: tempo,
                ..ParamsCache::default()
            };
            let mut held = channel.slots[0].captured_mags.clone();
            for hop in 1..=2 * period {
                for _ in 0..128 {
                    channel.process_sample(
                        if hop <= period { 0.25 } else { 0.5 },
                        params,
                        48_000.0,
                    );
                }
                let slot = &channel.slots[0];
                assert_eq!(slot.analyze_throttle, (hop % period) as u32);
                if hop % period == 0 {
                    assert_ne!(slot.captured_mags, held);
                    held.clone_from(&slot.captured_mags);
                } else {
                    assert_eq!(slot.captured_mags, held);
                }
            }
        }
    }

    #[test]
    fn chaos_uses_known_random_phases_without_accumulating_the_perturbation() {
        for (index, size, final_rng) in [
            (0, 512, 0xF980_7A79),
            (1, 1_024, 0x4DF7_F391),
            (2, 2_048, 0x61AE_4B59),
            (3, 4_096, 0x1E43_CF97),
        ] {
            let mut channel = StretchChannel::new(1);
            channel.active = index;
            channel.slots[index].captured_mags[1] = size as f32 * 0.125;
            run_hop(
                &mut channel,
                ParamsCache {
                    tempo_pct: 1.0,
                    chaos_pct: 50.0,
                    ..ParamsCache::default()
                },
            );
            assert_eq!(channel.rng_state, final_rng);
            assert_near(
                channel.slots[index].accumulated_output_phase[1],
                std::f64::consts::FRAC_PI_2,
                1e-6,
            );
            // Seed 1's second draw belongs to bin 1; bin 0 consumes the first draw.
            let random = 0x0408_0601_u32 as f64 / u32::MAX as f64 - 0.5;
            let phase = std::f64::consts::FRAC_PI_2 + 0.5 * std::f64::consts::TAU * random;
            for sample in 0..size {
                let window =
                    (0.5 - 0.5 * (std::f64::consts::TAU * sample as f64 / size as f64).cos()) / 1.5;
                let wave =
                    0.25 * (phase + std::f64::consts::TAU * sample as f64 / size as f64).cos();
                assert_near(
                    channel.slots[index].output_ring[(size / 4 + sample) % size],
                    wave * window,
                    2e-5,
                );
            }
        }
    }

    #[test]
    fn chaos_deadband_preserves_rng_continuation_at_the_exact_threshold() {
        let mut channels: Vec<_> = [0.0, 0.0001, 0.0001001]
            .into_iter()
            .map(|chaos| {
                let mut channel = StretchChannel::new(1);
                channel.active = 0;
                channel.slots[0].captured_mags[1] = 64.0;
                run_hop(
                    &mut channel,
                    ParamsCache {
                        tempo_pct: 1.0,
                        chaos_pct: chaos,
                        ..ParamsCache::default()
                    },
                );
                channel
            })
            .collect();
        assert_eq!(channels[0].rng_state, 1);
        assert_eq!(channels[1].rng_state, 1);
        assert_eq!(channels[2].rng_state, 0xF980_7A79);
        for channel in &mut channels {
            run_hop(
                channel,
                ParamsCache {
                    tempo_pct: 1.0,
                    chaos_pct: 50.0,
                    ..ParamsCache::default()
                },
            );
        }
        assert_eq!(
            channels[0].slots[0].output_ring,
            channels[1].slots[0].output_ring
        );
        assert_ne!(
            channels[0].slots[0].output_ring,
            channels[2].slots[0].output_ring
        );
    }

    #[test]
    fn phase_limit_wraps_only_above_the_exact_boundary() {
        let limit = 1_000_000.0_f32;
        for (target, expected_phase) in [
            (limit - 0.125, limit - 0.125),
            (limit, limit),
            (limit + 0.125, -0.25),
        ] {
            let mut channel = StretchChannel::new(1);
            channel.active = 0;
            channel.slots[0].captured_mags[1] = 64.0;
            channel.slots[0].accumulated_output_phase[1] = target - std::f32::consts::FRAC_PI_2;
            assert_eq!(
                channel.slots[0].accumulated_output_phase[1] + std::f32::consts::FRAC_PI_2,
                target
            );
            run_hop(
                &mut channel,
                ParamsCache {
                    tempo_pct: 1.0,
                    ..ParamsCache::default()
                },
            );
            assert_eq!(channel.slots[0].accumulated_output_phase[1], expected_phase);
            for sample in 0..512 {
                let window =
                    (0.5 - 0.5 * (std::f64::consts::TAU * sample as f64 / 512.0).cos()) / 1.5;
                let wave = 0.25
                    * (expected_phase as f64 + std::f64::consts::TAU * sample as f64 / 512.0).cos();
                assert_near(
                    channel.slots[0].output_ring[(128 + sample) % 512],
                    wave * window,
                    2e-5,
                );
            }
        }
    }

    fn storage(channel: &StretchChannel) -> Vec<[usize; 6]> {
        channel
            .slots
            .iter()
            .map(|slot| {
                [
                    slot.output_ring.as_ptr() as usize,
                    slot.spectrum_scratch.as_ptr() as usize,
                    slot.last_input_phase.as_ptr() as usize,
                    slot.captured_mags.as_ptr() as usize,
                    slot.remap_mag.as_ptr() as usize,
                    slot.remap_dev.as_ptr() as usize,
                ]
            })
            .collect()
    }

    #[test]
    fn fft_selection_switches_at_the_outgoing_hop_and_reuses_all_slots() {
        let mut channel = StretchChannel::new(1);
        let allocations = storage(&channel);
        for (index, size) in FFT_SIZES.into_iter().enumerate() {
            let slot = &channel.slots[index];
            assert_eq!(slot.fft_size, size);
            assert_eq!(slot.hop_size, size / 4);
            assert!(
                slot.output_ring.len() >= size
                    && slot.spectrum_scratch.len() >= size
                    && slot.last_input_phase.len() >= size
                    && slot.captured_mags.len() >= size
                    && slot.remap_mag.len() > size / 2
                    && slot.remap_dev.len() > size / 2
            );
        }
        for index in [0, 3, 1, 2, 0] {
            let old = channel.active;
            let hop = channel.slots[old].hop_size;
            channel.set_fft_size(FFT_SIZES[index]);
            assert_eq!(channel.latency_samples(), FFT_SIZES[index]);
            for _ in 0..hop - 1 {
                channel.process_sample(0.0, ParamsCache::default(), 48_000.0);
                assert_eq!(channel.active, old);
            }
            channel.process_sample(0.0, ParamsCache::default(), 48_000.0);
            assert_eq!(channel.active, index);
            assert_eq!(channel.pending, None);
            assert_eq!(storage(&channel), allocations);
        }
        channel.set_fft_size(4_096);
        channel.set_fft_size(999);
        assert_eq!(channel.pending, Some(3));
        channel.set_fft_size(512);
        assert_eq!(channel.pending, None);
        channel.set_fft_size(2_048);
        channel.reset();
        assert_eq!(
            (channel.active, channel.pending, channel.latency_samples()),
            (0, Some(2), 2_048)
        );
        assert_eq!(storage(&channel), allocations);
    }

    #[test]
    fn reset_clears_every_used_slot_and_replays_with_continuing_stereo_rng() {
        let mut used = configured(0.0, 30.0);
        let allocations = (storage(&used.chan_l), storage(&used.chan_r));
        let mut warm_energy = [0.0; 2];
        let mut sample = 0;
        for index in [0, 3, 1, 2] {
            used.set_param(3, index as f32);
            for _ in 0..FFT_SIZES[index] * 3 {
                let input = input_at(sample);
                let output = used.process_sample(input.0, input.1);
                warm_energy[0] += output.0 * output.0;
                warm_energy[1] += output.1 * output.1;
                sample += 1;
            }
        }
        assert!(warm_energy.into_iter().all(|energy| energy > 0.01));
        let rng = (used.chan_l.rng_state, used.chan_r.rng_state);
        used.set_param(3, 1.0);
        used.reset();
        assert_eq!(
            (
                used.chan_l.active,
                used.chan_l.pending,
                used.latency_samples()
            ),
            (2, Some(1), 1_024)
        );
        assert_eq!((used.chan_l.rng_state, used.chan_r.rng_state), rng);
        let mut fresh = configured(1.0, 30.0);
        fresh.chan_l.rng_state = rng.0;
        fresh.chan_r.rng_state = rng.1;
        let mut energy = [0.0; 2];
        for sample in 0..24_576 {
            let selection = match sample {
                4_000 => Some(3.0),
                8_000 => Some(0.0),
                12_000 => Some(2.0),
                _ => None,
            };
            if let Some(selection) = selection {
                used.set_param(3, selection);
                fresh.set_param(3, selection);
            }
            let input = input_at(sample + 137);
            let actual = used.process_sample(input.0, input.1);
            assert_eq!(
                actual,
                fresh.process_sample(input.0, input.1),
                "sample {sample}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
        assert_eq!((storage(&used.chan_l), storage(&used.chan_r)), allocations);
    }

    #[test]
    fn rate_configuration_rebuilds_and_reseeds_both_channels_with_the_selected_fft() {
        for (index, size) in FFT_SIZES.into_iter().enumerate() {
            let mut used = configured(index as f32, 40.0);
            for sample in 0..size * 5 {
                let input = input_at(sample);
                used.process_sample(input.0, input.1);
            }
            used.set_sample_rate(32_000.0);
            let mut fresh = configured(index as f32, 40.0);
            fresh.set_sample_rate(32_000.0);
            assert_eq!(used.latency_samples(), size);
            assert_eq!(used.chan_r.latency_samples(), size);
            assert_eq!(
                (used.chan_l.rng_state, used.chan_r.rng_state),
                (0x5717_E001, 0x5717_E002)
            );
            let mut energy = [0.0; 2];
            for sample in 0..size * 5 {
                let input = input_at(sample + 333);
                let actual = used.process_sample(input.0, input.1);
                assert_eq!(actual, fresh.process_sample(input.0, input.1));
                energy[0] += actual.0 * actual.0;
                energy[1] += actual.1 * actual.1;
            }
            assert!(energy.into_iter().all(|energy| energy > 0.01));
        }
    }

    #[test]
    fn chaos_preserves_stereo_isolation_and_uses_independent_random_streams() {
        let mut isolated = configured(0.0, 40.0);
        let mut mono = configured(0.0, 0.0);
        let mut isolated_energy = 0.0;
        let mut difference = 0.0;
        for sample in 0..16_384 {
            let input = input_at(sample).0;
            let output = isolated.process_sample(input, 0.0);
            assert_eq!(output.1, 0.0);
            isolated_energy += output.0 * output.0;
            if sample == 8_192 {
                mono.set_param(2, 50.0);
            }
            let output = mono.process_sample(input, input);
            if sample < 8_192 {
                assert_eq!(output.0, output.1);
            }
            if sample >= 10_240 {
                difference += (output.0 - output.1).powi(2);
            }
        }
        assert!(isolated_energy > 0.01);
        assert!(difference > 0.01);
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralStretchEffect::default();
        assert_eq!(
            (
                effect.params.speed,
                effect.params.tempo_pct,
                effect.params.chaos_pct,
                effect.params.fft_param
            ),
            (1.0, 100.0, 0.0, 2.0)
        );
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Speed", 0.25, 4.0, 1.0),
                ("Tempo", 1.0, 100.0, 100.0),
                ("Chaos", 0.0, 100.0, 0.0),
                ("FFT", 0.0, 3.0, 2.0)
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(
            specs[1..]
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 2,
                unit: "x"
            }
        ));
        assert!(specs[1..3].iter().all(|spec| matches!(
            spec.format,
            ParamFormat::Number {
                decimals: 0,
                unit: " %"
            }
        )));
        assert!(
            matches!(specs[3].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, cases) in [
            (0, [(-999.0, 0.25), (1.25, 1.25), (999.0, 4.0)]),
            (1, [(-999.0, 1.0), (37.5, 37.5), (999.0, 100.0)]),
            (2, [(-999.0, 0.0), (37.5, 37.5), (999.0, 100.0)]),
        ] {
            for (value, expected) in cases {
                effect.set_param(slot, value);
                assert_eq!(
                    [
                        effect.params.speed,
                        effect.params.tempo_pct,
                        effect.params.chaos_pct
                    ][slot],
                    expected
                );
            }
        }
        for (value, size) in [
            (-99.0, 512),
            (0.49, 512),
            (0.5, 1_024),
            (1.5, 2_048),
            (99.0, 4_096),
        ] {
            effect.set_param(3, value);
            assert_eq!(effect.latency_samples(), size);
            assert_eq!(effect.chan_r.latency_samples(), size);
        }
        effect.set_param(99, f32::INFINITY);
        assert_eq!(
            (
                effect.params.speed,
                effect.params.tempo_pct,
                effect.params.chaos_pct
            ),
            (4.0, 100.0, 100.0)
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn latency_reports_target_fft_immediately_and_is_stable() {
        // Regression for the Bitwig PDC feedback loop. latency_samples must:
        //  1. report one full FFT window (not hop_size),
        //  2. report the TARGET size the instant the FFT param is set (the
        //     channel constructs at the 2048 default, so a saved non-default
        //     FFT is a pending switch that hasn't applied yet), and
        //  3. NOT move when that pending switch later applies at a hop
        //     boundary, nor when reset() is called.
        // A moving latency made the host re-align repeatedly, firing
        // Plugin::reset / transport edges that zeroed every MSEG phase.
        let mut e = SpectralStretchEffect::default();
        for (v, expected) in [(0.0, 512), (1.0, 1024), (2.0, 2048), (3.0, 4096)] {
            e.set_param(3, v); // FFT is the last param (index 3)
            assert_eq!(
                e.latency_samples(),
                expected,
                "latency must report the target FFT window size immediately"
            );
        }

        // FFT = 1024 (slot 1) while the channel is constructed at slot 2:
        // the value must hold across the hop-boundary switch and a reset.
        let mut e = SpectralStretchEffect::default();
        e.set_param(3, 1.0);
        let reported = e.latency_samples();
        assert_eq!(reported, 1024);
        let _ = drive(&mut e, 4096, |_| 0.1); // past several hop boundaries
        assert_eq!(
            e.latency_samples(),
            reported,
            "latency must not move when the pending FFT switch applies"
        );
        e.reset();
        assert_eq!(
            e.latency_samples(),
            reported,
            "reset must preserve the selected FFT latency (no swing to the default slot)"
        );
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
