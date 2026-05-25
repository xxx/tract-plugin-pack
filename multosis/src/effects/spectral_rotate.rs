//! Spectral Rotate: circular shift of the spectrum.
//!
//! Bins are rotated by `shift_bins = round(shift_pct/100 * N/2)`. Unlike
//! SpectralShift (which zeros out-of-range bins), Rotate wraps modulo N/2
//! so nothing is lost. See the spec doc for the full DSP outline.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy, Default)]
struct ParamsCache {
    fft_param: f32,
    shift_pct: f32,
}

struct TransformCtx {
    shift_pct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sample_rate: f32) {
        // Real-input spectrum is conjugate-symmetric around bin N/2.
        // Operate on the positive-frequency half [1..half) and mirror.
        let half = fft_size / 2;
        let shift_bins = ((self.shift_pct * 0.01) * half as f32).round() as i32;
        if shift_bins == 0 {
            return;
        }
        let n = fft_size;
        // Stash positive-half bins into negative-half slots.
        for k in 1..half {
            spectrum[n - k] = spectrum[k];
        }
        // Rotate write.
        for k in 1..half as i32 {
            let src = (((k - shift_bins).rem_euclid(half as i32 - 1)) + 1) as usize;
            // Read from the stashed mirror: stashed[m] is at spectrum[n - m].
            spectrum[k as usize] = spectrum[n - src];
        }
        // Rebuild negative half from conjugates of rotated positive half.
        for k in 1..half {
            spectrum[n - k] = spectrum[k].conj();
        }
        // DC (k=0) and Nyquist (k=half) are untouched.
    }
}

pub struct SpectralRotateEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralRotateEffect {
    pub const PARAMS: [ParamSpec; 2] = [
        // Shift is intentionally narrow (+/-15%). Outside this range the
        // bin rotation moves nearly all musical energy out of the audible
        // low-bin band and replaces it with high-bin near-silence -- the
        // output collapses to noise/silence. Keep the dial in the useful
        // textural range.
        ParamSpec {
            name: "Shift",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        // FFT in the LAST slot so it isn't the first dial users grab to
        // modulate. Modulating FFT mid-stream causes audible artefacts;
        // putting it where it's clearly a setting-not-a-knob keeps the
        // accidental-modulation rate down.
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

impl Default for SpectralRotateEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache {
                fft_param: 2.0,
                shift_pct: 0.0,
            },
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralRotateEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            shift_pct: self.params.shift_pct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            shift_pct: self.params.shift_pct,
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
        // Re-apply the FFT-size param so the new engines pick it up.
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
            0 => self.params.shift_pct = value.clamp(-15.0, 15.0),
            1 => {
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

    fn drive(e: &mut SpectralRotateEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_lists_shift_then_fft() {
        let e = SpectralRotateEffect::default();
        let p = e.parameters();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "Shift");
        assert_eq!(p[1].name, "FFT");
    }

    #[test]
    fn shift_zero_is_passthrough() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(1, 1.0); // FFT = 1024 for shorter warm-up
        e.set_param(0, 0.0); // Shift = 0
        let f = 1000.0;
        let sr = 48_000.0;
        let n = 4096_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        // After warm-up, output should retain non-trivial energy.
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(0, 10.0); // arbitrary non-zero shift within +/-15% range
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn fft_size_param_changes_latency() {
        // The latency reported to the host changes IMMEDIATELY when the FFT
        // param moves -- no audio needs to flow first. The audio path's
        // active slot still latches the switch until the next hop boundary,
        // but `latency_samples()` reports the pending slot's hop so the host
        // can re-align PDC right away.
        let mut e = SpectralRotateEffect::default();
        for (i, expected) in [(0.0, 256), (1.0, 512), (2.0, 1024), (3.0, 2048)] {
            e.set_param(1, i); // FFT is the LAST param (slot 1)
            assert_eq!(e.latency_samples(), expected);
        }
    }

    #[test]
    fn shift_positive_moves_energy_up() {
        // A 1 kHz sine shifted +50% should produce most of its energy
        // above 1 kHz. Tested via a forward FFT of the output.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let f = 1000.0_f32;
        let mut e = SpectralRotateEffect::default();
        e.set_param(1, 1.0); // FFT = 1024 (slot 1)
        e.set_param(0, 15.0); // Shift = 15% (slot 0) -- max in the new range
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let lo: f32 = tail[..bin_1k].iter().map(|c| c.norm_sqr()).sum();
        let hi: f32 = tail[bin_1k..1024].iter().map(|c| c.norm_sqr()).sum();
        assert!(
            hi > lo,
            "expected hi-band energy > lo-band; got hi={hi} lo={lo}"
        );
    }
}
