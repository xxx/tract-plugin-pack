//! Spectral Corrupt: zero the quietest or loudest fraction of bins.
//!
//! Amount > 0 zeros the |Amount|% quietest bins (creative noise-gate).
//! Amount < 0 zeros the loudest (inverted -- emphasises noise floor).
//! Amount == 0 is passthrough (modulo Decay carry).
//!
//! Decay is a one-pole carry of last frame's per-bin gain toward the new
//! target. Decay=0 -> instant gate switch (clicks/glitch). Decay=1 -> the
//! previous frame's gain holds forever (gate never re-evaluates). In
//! between: a smooth fade.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Maximum half-spectrum entries we'll ever need to hold -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralCorruptChannel {
    /// Scratch for the sort -- indices 0..count of the active spectrum.
    bin_indices: Vec<u16>,
    /// Per-bin gain memory -- the previous frame's applied gain. Decay
    /// lerps toward the new target.
    bin_gains: Vec<f32>,
    /// Pre-allocated target buffer (write 1.0/0.0 per bin, then apply).
    target_buf: Vec<f32>,
}

impl SpectralCorruptChannel {
    fn new() -> Self {
        Self {
            bin_indices: vec![0u16; MAX_HALF_PLUS_ONE],
            // Start at gain=1.0 so a fresh effect with Amount=0 is exact passthrough.
            bin_gains: vec![1.0; MAX_HALF_PLUS_ONE],
            target_buf: vec![1.0; MAX_HALF_PLUS_ONE],
        }
    }

    fn reset(&mut self) {
        self.bin_gains.fill(1.0);
        self.target_buf.fill(1.0);
        // Don't bother re-zeroing bin_indices -- they're overwritten per frame.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
    decay_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
            decay_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralCorruptChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let count = half + 1;
        let amt = self.params.amount_pct * 0.01;
        let d = (self.params.decay_pct * 0.01).clamp(0.0, 0.99);

        if amt.abs() < 1e-3 {
            // No gating this frame; just decay any held gain back toward 1.0.
            for k in 0..count {
                self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + (1.0 - d);
                spectrum[k].re *= self.chan.bin_gains[k];
                spectrum[k].im *= self.chan.bin_gains[k];
                if k != 0 && k != half {
                    spectrum[fft_size - k] = spectrum[k].conj();
                }
            }
            return;
        }

        // Rank bins by magnitude using the pre-allocated index scratch.
        for k in 0..count {
            self.chan.bin_indices[k] = k as u16;
        }
        self.chan.bin_indices[..count].sort_unstable_by(|&a, &b| {
            let ma = spectrum[a as usize].norm();
            let mb = spectrum[b as usize].norm();
            ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build the target_buf: 0 for bins in the zero-set, 1 for the rest.
        let cut = ((amt.abs() * count as f32).round() as usize).min(count);
        self.chan.target_buf[..count].fill(1.0);
        let zero_range = if amt > 0.0 {
            // Quietest |amt|*count are zeroed.
            &self.chan.bin_indices[..cut]
        } else {
            // Loudest |amt|*count are zeroed.
            &self.chan.bin_indices[count - cut..count]
        };
        for &k in zero_range {
            self.chan.target_buf[k as usize] = 0.0;
        }

        // Apply: per-bin gain = lerp(last, target, 1 - d).
        for k in 0..count {
            let t = self.chan.target_buf[k];
            self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + t * (1.0 - d);
            spectrum[k].re *= self.chan.bin_gains[k];
            spectrum[k].im *= self.chan.bin_gains[k];
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralCorruptEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralCorruptChannel,
    chan_r: SpectralCorruptChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralCorruptEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Amount",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Decay",
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

impl Default for SpectralCorruptEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralCorruptChannel::new(),
            chan_r: SpectralCorruptChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralCorruptEffect {
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
            0 => self.params.amount_pct = value.clamp(-100.0, 100.0),
            1 => self.params.decay_pct = value.clamp(0.0, 100.0),
            2 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
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

    fn drive(e: &mut SpectralCorruptEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralCorruptEffect::default().parameters().len(), 3);
    }

    #[test]
    fn amount_zero_with_decay_zero_is_passthrough() {
        let mut e = SpectralCorruptEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.0); // Amount = 0 (slot 0)
        e.set_param(1, 0.0); // Decay = 0 -> bin_gains snap straight to 1.0 (slot 1)
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
        let mut e = SpectralCorruptEffect::default();
        e.set_param(0, 50.0); // arbitrary non-zero Amount (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
