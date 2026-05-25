//! Spectral Mirror: flips a portion of the spectrum around a centre frequency.
//!
//! Within +/- Width/2 octaves of Freq, the bin at offset +d above the centre
//! is swapped with the bin at offset -d below. Conjugate-mirror so the
//! reconstructed real-output preserves the right phase relationships.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    freq_hz: f32,
    bw_oct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            freq_hz: 1000.0,
            bw_oct: 1.0,
        }
    }
}

struct TransformCtx {
    freq_hz: f32,
    bw_oct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins =
            ((self.freq_hz * (self.bw_oct * 0.5).exp2() - self.freq_hz) / bin_hz).ceil() as i32;
        // For each offset d in [1..=band_bins], swap centre+d with centre-d
        // (conjugate-swap so phase reflects, not just copies).
        for d in 1..=band_bins {
            let kp = centre_bin + d;
            let kn = centre_bin - d;
            if kn > 0 && (kp as usize) < half {
                let a = spectrum[kp as usize];
                let b = spectrum[kn as usize];
                spectrum[kp as usize] = b.conj();
                spectrum[kn as usize] = a.conj();
            }
        }
        // Rebuild negative half from conjugates of (now-rewritten) positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralMirrorEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralMirrorEffect {
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
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Width",
            min: 0.1,
            max: 4.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: " oct",
            },
        },
    ];
}

impl Default for SpectralMirrorEffect {
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

impl Effect for SpectralMirrorEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            freq_hz: self.params.freq_hz,
            bw_oct: self.params.bw_oct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            freq_hz: self.params.freq_hz,
            bw_oct: self.params.bw_oct,
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
            1 => self.params.freq_hz = value.clamp(20.0, 20_000.0),
            2 => self.params.bw_oct = value.clamp(0.1, 4.0),
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

    fn drive(e: &mut SpectralMirrorEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralMirrorEffect::default();
        e.set_param(1, 5000.0);
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralMirrorEffect::default().parameters().len(), 3);
    }

    #[test]
    fn mirror_swaps_test_tones() {
        // A 500 Hz sine mirrored around 1000 Hz with 2-octave Width should
        // emerge centred near 2000 Hz (since 500 and 2000 are equidistant
        // from 1000 in log-frequency space, but we mirror linearly in bins
        // -- so the centre_bin reflection sends a peak at bin_500 to
        // bin_centre + (bin_centre - bin_500). Verify by forward-FFT of the
        // tail and checking energy is at the mirror position, not the source.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralMirrorEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 1000.0); // centre at 1 kHz
        e.set_param(2, 2.0); // 2 octaves wide (covers 500 Hz .. 2 kHz)
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        // Energy at the source frequency (500 Hz) should drop, energy at the
        // linear-bin mirror (~1500 Hz: 1000 + (1000 - 500)) should rise.
        let bin_500 = (500.0 * 2048.0 / sr).round() as usize;
        let bin_1500 = (1500.0 * 2048.0 / sr).round() as usize;
        let e_500: f32 = tail[bin_500.saturating_sub(2)..=bin_500 + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1500: f32 = tail[bin_1500.saturating_sub(2)..=bin_1500 + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_1500 > e_500,
            "expected mirror energy at 1500 Hz > source 500 Hz; got 1500={e_1500} 500={e_500}"
        );
    }
}
