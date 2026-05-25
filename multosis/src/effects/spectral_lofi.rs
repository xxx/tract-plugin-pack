//! Spectral Lofi: bin decimation with optional randomisation.
//!
//! A bitmask of which bins to keep is refreshed every `Slow` hops. At
//! Randomise=0 we keep every Nth bin (regular decimation, N = 1/(1-Factor)).
//! At Randomise=100 we keep each bin with independent probability (1-Factor).
//! In between we lerp the keep probability between the two rules. Bins not
//! kept are zeroed.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Maximum half-spectrum entries we'll ever need to mask -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// xorshift32 -- audio-thread-safe (no allocation, no transcendentals).
/// Returns the next state; the caller threads it through.
fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralLofiChannel {
    keep_mask: Vec<bool>,
    hop_counter: u32,
    rng_state: u32,
}

impl SpectralLofiChannel {
    fn new(seed: u32) -> Self {
        Self {
            // Start with all-true so a fresh effect at Factor=0 is passthrough.
            keep_mask: vec![true; MAX_HALF_PLUS_ONE],
            hop_counter: 0,
            // Seed must be non-zero for xorshift to do anything useful.
            rng_state: seed.max(1),
        }
    }

    fn reset(&mut self) {
        self.keep_mask.fill(true);
        self.hop_counter = 0;
        // Don't reset rng_state -- otherwise reset would produce
        // identical masks every time.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    factor_pct: f32,
    randomise_pct: f32,
    slow: i32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            factor_pct: 0.0,
            randomise_pct: 0.0,
            slow: 1,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralLofiChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let factor = (self.params.factor_pct * 0.01).clamp(0.0, 1.0);
        if factor <= 1e-6 {
            return; // Factor=0 -> keep everything
        }
        let randomise = (self.params.randomise_pct * 0.01).clamp(0.0, 1.0);
        let slow = self.params.slow.clamp(1, 100) as u32;

        // Refresh keep_mask every `slow` hops.
        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= slow {
            self.chan.hop_counter = 0;
            // Regular vote spacing: keep iff (k mod step) < 0.5.
            // step >= 1.0; e.g. factor=0.5 -> step=2 (keep every 2nd bin).
            let step = (1.0 / (1.0 - factor)).max(1.0);
            for k in 0..=half {
                let regular = ((k as f32) % step) < 0.5;
                self.chan.rng_state = xorshift(self.chan.rng_state);
                let r = (self.chan.rng_state as f32) / (u32::MAX as f32);
                let random_vote = r > factor;
                // Lerp the two binary votes into a probability and threshold at 0.5.
                let p = (1.0 - randomise) * (regular as i32 as f32)
                    + randomise * (random_vote as i32 as f32);
                self.chan.keep_mask[k] = p > 0.5;
            }
        }
        // Apply the mask.
        for k in 0..=half {
            if !self.chan.keep_mask[k] {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

pub struct SpectralLofiEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralLofiChannel,
    chan_r: SpectralLofiChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralLofiEffect {
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
            name: "Factor",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Random",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Slow",
            min: 1.0,
            max: 100.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " hops",
            },
        },
    ];
}

impl Default for SpectralLofiEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        // Seed L and R with different non-zero values so their masks
        // are independent.
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralLofiChannel::new(0xC001_C0DE),
            chan_r: SpectralLofiChannel::new(0xDEAD_BEEF),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralLofiEffect {
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
            1 => self.params.factor_pct = value.clamp(0.0, 100.0),
            2 => self.params.randomise_pct = value.clamp(0.0, 100.0),
            3 => self.params.slow = value.round().clamp(1.0, 100.0) as i32,
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

    fn drive(e: &mut SpectralLofiEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralLofiEffect::default().parameters().len(), 4);
    }

    #[test]
    fn factor_zero_is_passthrough() {
        let mut e = SpectralLofiEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 0.0); // Factor = 0 -> return early
        let f = 1000.0;
        let sr = 48_000.0;
        let n = 4096;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralLofiEffect::default();
        e.set_param(1, 50.0); // arbitrary Factor
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
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
