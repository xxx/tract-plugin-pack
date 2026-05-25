//! Spectral Compress: per-bin compression toward a target spectrum.
//!
//! Each bin's magnitude is tracked by a one-pole envelope follower
//! (~50 ms tau). The compression target is a power-law curve f^tone,
//! normalised at 1 kHz: tone=-1 -> pink (1/f), tone=0 -> flat, tone=+1
//! -> white (f). Per-bin ratio = (target / current)^Amount; the
//! magnitude scaling preserves the bin's phase.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralCompressChannel {
    /// Per-bin one-pole envelope of magnitude. Pre-allocated to the
    /// largest half-spectrum size so the audio thread never reallocates.
    avg_mag: Vec<f32>,
}

impl SpectralCompressChannel {
    fn new() -> Self {
        Self {
            avg_mag: vec![0.0; MAX_HALF_PLUS_ONE],
        }
    }
    fn reset(&mut self) {
        self.avg_mag.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
    tone_pct: f32, // -100..+100
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
            tone_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralCompressChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let amount = (self.params.amount_pct * 0.01).clamp(0.0, 1.0);
        if amount < 1e-3 {
            return;
        }
        let tone = (self.params.tone_pct * 0.01).clamp(-1.0, 1.0);
        let bin_hz = sample_rate / fft_size as f32;
        // One-pole follower coefficient, ~50 ms tau at the hop rate.
        let hop_samples = (fft_size / 2) as f32;
        let tau_samples = 0.050 * sample_rate;
        let alpha = (-hop_samples / tau_samples).exp();

        for k in 1..=half {
            let m = spectrum[k].norm();
            self.chan.avg_mag[k] = self.chan.avg_mag[k] * alpha + m * (1.0 - alpha);
            // Target curve: f^tone with reference at 1 kHz.
            let f_hz = k as f32 * bin_hz;
            let f_norm = (f_hz / 1000.0).max(1e-3);
            let target = f_norm.powf(tone);
            let current = self.chan.avg_mag[k].max(1e-12);
            let ratio = (target / current).powf(amount);
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        // DC (k=0) is untouched.
    }
}

pub struct SpectralCompressEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralCompressChannel,
    chan_r: SpectralCompressChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralCompressEffect {
    pub const PARAMS: [ParamSpec; 3] = [
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
        ParamSpec {
            name: "Tone",
            min: -100.0,
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

impl Default for SpectralCompressEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralCompressChannel::new(),
            chan_r: SpectralCompressChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralCompressEffect {
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
            0 => self.params.amount_pct = value.clamp(0.0, 100.0),
            1 => self.params.tone_pct = value.clamp(-100.0, 100.0),
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

    fn drive(e: &mut SpectralCompressEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralCompressEffect::default().parameters().len(), 3);
    }

    #[test]
    fn amount_zero_is_passthrough() {
        let mut e = SpectralCompressEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.0); // Amount = 0 -> return early (slot 0)
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
        let mut e = SpectralCompressEffect::default();
        e.set_param(0, 50.0); // arbitrary non-zero Amount (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
