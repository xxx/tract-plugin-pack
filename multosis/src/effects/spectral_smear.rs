//! Spectral Smear: per-bin magnitude envelope hold + Chaos phase randomisation.
//!
//! Each bin runs through a one-pole envelope follower: instant attack
//! (mag_out = max(mag_in, mag_held_decayed)) so transients punch through, but
//! a release tau equal to `Length` so the magnitude lingers after the source
//! drops. The decay coefficient is computed per hop -- one analysis hop is
//! `fft_size / 2` samples, so decay = exp(-hop_samples / length_samples). Chaos
//! adds a random rotation to each bin's phase, breaking inter-bin coherence
//! the longer the smear runs.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Maximum half-spectrum entries we'll ever need to hold -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// xorshift32 -- audio-thread-safe (no allocation, no transcendentals).
/// Returns the next state; the caller threads it through.
fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralSmearChannel {
    last_mag: Vec<f32>,
    rng_state: u32,
}

impl SpectralSmearChannel {
    fn new(seed: u32) -> Self {
        Self {
            last_mag: vec![0.0; MAX_HALF_PLUS_ONE],
            // Seed must be non-zero for xorshift to do anything useful.
            rng_state: seed.max(1),
        }
    }

    fn reset(&mut self) {
        self.last_mag.fill(0.0);
        // Don't reset rng_state -- otherwise repeated resets produce
        // identical phase rotations.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    length_ms: f32,
    chaos_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            length_ms: 200.0,
            chaos_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralSmearChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_samples = (self.params.length_ms * 0.001 * sample_rate).max(1.0);
        // Decay per hop = exp(-hop / length).
        let decay = (-hop_samples / length_samples).exp();
        let chaos = (self.params.chaos_pct * 0.01).clamp(0.0, 1.0);

        for k in 0..=half {
            let mag_in = spectrum[k].norm();
            let mag_held = self.chan.last_mag[k] * decay;
            let mag = mag_in.max(mag_held);
            self.chan.last_mag[k] = mag;

            // Reconstruct bin: original phase + Chaos-amount random rotation.
            let phase_in = spectrum[k].im.atan2(spectrum[k].re);
            self.chan.rng_state = xorshift(self.chan.rng_state);
            let r = (self.chan.rng_state as f32) / (u32::MAX as f32) - 0.5; // -0.5..0.5
            let phase = phase_in + chaos * std::f32::consts::TAU * r;
            spectrum[k] = Complex::new(mag * phase.cos(), mag * phase.sin());
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralSmearEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralSmearChannel,
    chan_r: SpectralSmearChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralSmearEffect {
    pub const PARAMS: [ParamSpec; 3] = [
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
            name: "Length",
            min: 10.0,
            max: 2000.0,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " ms",
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

impl Default for SpectralSmearEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        // Seed L and R with different non-zero values so their phase
        // rotations are independent.
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralSmearChannel::new(0xC0FF_EE01),
            chan_r: SpectralSmearChannel::new(0xC0FF_EE02),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralSmearEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            chan: &mut self.chan_l,
            params: self.params,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            chan: &mut self.chan_r,
            params: self.params,
        };
        let ro = self.engine_r.process_sample(right, &mut ctx_r);
        (lo, ro)
    }
    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.engine_l = SpectralEngine::new(sample_rate);
        self.engine_r = SpectralEngine::new(sample_rate);
        let fft_size = FFT_SIZES[self.params.fft_param.round().clamp(0.0, 3.0) as usize];
        self.engine_l.set_fft_size(fft_size);
        self.engine_r.set_fft_size(fft_size);
    }
    fn reset(&mut self) {
        self.engine_l.reset();
        self.engine_r.reset();
        self.chan_l.reset();
        self.chan_r.reset();
    }
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
            1 => self.params.length_ms = value.clamp(10.0, 2000.0),
            2 => self.params.chaos_pct = value.clamp(0.0, 100.0),
            _ => {}
        }
    }
    fn latency_samples(&self) -> usize {
        self.engine_l.latency_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(e: &mut SpectralSmearEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralSmearEffect::default().parameters().len(), 3);
    }

    #[test]
    fn silence_in_silence_out() {
        // Note: smear holds magnitudes; for a clean test the smear state must
        // start zeroed. Default state is zeroed.
        let mut e = SpectralSmearEffect::default();
        e.set_param(1, 500.0); // Length = 500 ms
        let out = drive(&mut e, 8192, |_| 0.0);
        // After warm-up, with no input there should be no held magnitude.
        let tail = &out[2 * e.latency_samples()..];
        assert!(tail.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn smear_tail_extends_past_input() {
        // Drive ~50 ms of noise, then silence. With Length=500 ms, the smear
        // tail should still produce non-zero output 200 ms after the source
        // drops to silence -- but the energy should be lower than the source.
        let sr = 48_000.0_f32;
        let mut e = SpectralSmearEffect::default();
        e.set_param(0, 1.0); // FFT = 1024, latency = 512 samples
        e.set_param(1, 500.0); // Length = 500 ms
        let burst_samples = (0.050 * sr) as usize;
        let tail_samples = (0.200 * sr) as usize;
        // Pre-generate noise into a buffer so the `drive` closure stays Fn.
        let mut rng = 1u32;
        let noise: Vec<f32> = (0..burst_samples)
            .map(|_| {
                rng = xorshift(rng);
                (rng as f32) / (u32::MAX as f32) - 0.5
            })
            .collect();
        let out = drive(&mut e, burst_samples + tail_samples, |i| {
            if i < burst_samples {
                noise[i]
            } else {
                0.0
            }
        });
        // Burst-region energy (input is ~ N * (1/12) for uniform [-0.5, 0.5]).
        let burst_e: f32 = out[..burst_samples].iter().map(|x| x * x).sum();
        // Tail-region energy after the burst.
        let tail_e: f32 = out[burst_samples + 2 * e.latency_samples()..]
            .iter()
            .map(|x| x * x)
            .sum();
        assert!(
            tail_e > 0.001,
            "smear tail should hold energy past source drop; got {tail_e}"
        );
        // Sanity: tail shouldn't exceed the burst (energy decays).
        assert!(
            tail_e < burst_e * 2.0,
            "tail {tail_e} unexpectedly larger than burst {burst_e}"
        );
    }

    #[test]
    fn xorshift_never_returns_zero() {
        // Sanity check on the RNG -- a zero state would lock xorshift forever.
        let mut s = 1u32;
        for _ in 0..1000 {
            s = xorshift(s);
            assert!(s != 0);
        }
    }
}
