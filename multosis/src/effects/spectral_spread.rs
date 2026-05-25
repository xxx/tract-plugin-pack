//! Spectral Spread: box-blur the magnitude spectrum across bins.
//!
//! Per-bin magnitude is replaced by the mean magnitude within +/- radius bins,
//! where radius scales linearly with Amount (0-100% -> 0-16 bins). Phase
//! preserved per-bin -- output bin is the original complex value rescaled to
//! the new magnitude. Kernel cap (16 bins) keeps Amount=100% as
//! detail-softening rather than spectrum-smashing.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Maximum half-spectrum size we ever need to scratch -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// Per-channel mutable state. Pre-allocated to the largest possible
/// half-spectrum size so no allocation ever happens on the audio thread.
struct SpectralSpreadChannel {
    mags: Vec<f32>,
}

impl SpectralSpreadChannel {
    fn new() -> Self {
        Self {
            mags: vec![0.0; MAX_HALF_PLUS_ONE],
        }
    }

    fn reset(&mut self) {
        self.mags.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
        }
    }
}

/// Carrier passed into `SpectralEngine::process_sample`. Borrows the
/// per-channel scratch AND a copy of the params snapshot.
struct TransformCtx<'a> {
    chan: &'a mut SpectralSpreadChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let radius = ((self.params.amount_pct * 0.01) * 16.0).round() as i32;
        if radius == 0 {
            return;
        }
        // Snapshot magnitudes of the positive half (DC + ... + Nyquist).
        let mags = &mut self.chan.mags[..=half];
        for k in 0..=half {
            mags[k] = spectrum[k].norm();
        }
        // Box-blur: each bin's new magnitude is the mean within +/- radius.
        // Apply the scaling factor (new / old) to the original complex value
        // to preserve phase.
        for k in 0..=half {
            let lo = (k as i32 - radius).max(0) as usize;
            let hi = (k as i32 + radius).min(half as i32) as usize;
            let sum: f32 = mags[lo..=hi].iter().sum();
            let new_mag = sum / (hi - lo + 1) as f32;
            let old_mag = mags[k].max(1e-20);
            let ratio = new_mag / old_mag;
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralSpreadEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralSpreadChannel,
    chan_r: SpectralSpreadChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralSpreadEffect {
    pub const PARAMS: [ParamSpec; 2] = [
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
            name: "Amount",
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

impl Default for SpectralSpreadEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralSpreadChannel::new(),
            chan_r: SpectralSpreadChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralSpreadEffect {
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
            1 => self.params.amount_pct = value.clamp(0.0, 100.0),
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

    fn drive(e: &mut SpectralSpreadEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_two() {
        assert_eq!(SpectralSpreadEffect::default().parameters().len(), 2);
    }

    #[test]
    fn amount_zero_is_passthrough() {
        let mut e = SpectralSpreadEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 0.0); // Amount = 0 -> radius=0 -> return early
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
        let mut e = SpectralSpreadEffect::default();
        e.set_param(1, 50.0); // arbitrary non-zero amount
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
