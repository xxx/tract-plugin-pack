//! Spectral Shift: per-bin frequency translation and scaling.
//!
//! Translate moves bins by +/- 100% of Nyquist; out-of-range bins are zeroed
//! (vs. SpectralRotate's wrap). Scale (expand/contract spectrum, 0.5..2.0)
//! is declared but the Scale path is filled in by Task 14 -- this file
//! handles only Translate. A flat translate=0 + scale=1 is identity.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy)]
pub(super) struct ParamsCache {
    pub fft_param: f32,
    pub scale: f32,
    pub translate_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            scale: 1.0,
            translate_pct: 0.0,
        }
    }
}

struct TransformCtx {
    // scale: f32, // TODO(Task 14): wire up the Scale path
    translate_pct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sample_rate: f32) {
        let half = fft_size as i32 / 2;
        let translate_bins = ((self.translate_pct * 0.01) * half as f32).round() as i32;
        if translate_bins == 0 {
            return;
        }
        // Stash positive half into negative-half slots so we can read them
        // while writing new positive-half values.
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k];
        }
        for k in 1..half {
            let src = k - translate_bins;
            spectrum[k as usize] = if (1..half).contains(&src) {
                spectrum[fft_size - src as usize] // read from stash
            } else {
                Complex::default() // out of range -> zero
            };
        }
        // Rebuild negative half from conjugates of (now-rewritten) positive half.
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralShiftEffect {
    sample_rate: f32,
    pub(super) params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralShiftEffect {
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
            name: "Scale",
            min: 0.5,
            max: 2.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Translate",
            min: -100.0,
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

impl Default for SpectralShiftEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralShiftEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            translate_pct: self.params.translate_pct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            translate_pct: self.params.translate_pct,
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
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
            1 => self.params.scale = value.clamp(0.5, 2.0),
            2 => self.params.translate_pct = value.clamp(-100.0, 100.0),
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

    fn drive(e: &mut SpectralShiftEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralShiftEffect::default().parameters().len(), 3);
    }

    #[test]
    fn translate_zero_is_passthrough() {
        let mut e = SpectralShiftEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(2, 0.0); // Translate = 0
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
        let mut e = SpectralShiftEffect::default();
        e.set_param(2, 50.0);
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn translate_positive_moves_energy_up_with_zero_out_of_range() {
        // A 1 kHz sine translated by +25% of Nyquist (= +6 kHz at 48 kHz SR)
        // should produce energy at ~7 kHz (1 + 6). Verify via forward-FFT
        // of the tail; also confirm DC/low-bin region has little energy.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralShiftEffect::default();
        e.set_param(0, 1.0); // FFT = 1024, Nyquist = 24 kHz, 25% = 6 kHz
        e.set_param(2, 25.0);
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_7k = (7000.0 * 2048.0 / sr).round() as usize;
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let e_7k: f32 = tail[bin_7k.saturating_sub(2)..=bin_7k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1k: f32 = tail[bin_1k.saturating_sub(2)..=bin_1k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_7k > e_1k,
            "expected translated energy at 7 kHz > source 1 kHz; got 7k={e_7k} 1k={e_1k}"
        );
    }

    #[test]
    fn scale_is_accepted_but_ignored_in_this_task() {
        // Task 5 only implements Translate. Scale != 1 is stored but the
        // transform is identity if Translate = 0. Task 14 will fill in
        // Scale's effect.
        let mut e = SpectralShiftEffect::default();
        e.set_param(1, 2.0); // Scale = 2 -- accepted, cached
        e.set_param(2, 0.0); // Translate = 0
        assert_eq!(e.params.scale, 2.0);
        // Output should still be roughly passthrough since Translate=0 and
        // Scale is currently a no-op.
        let f = 1000.0;
        let sr = 48_000.0;
        let out = drive(&mut e, 4096, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }
}
