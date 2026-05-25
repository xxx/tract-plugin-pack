//! Spectral Bandpass: FFT-based brickwall bandpass filter.
//!
//! Zeros every bin outside [Freq * 2^(-bw/2), Freq * 2^(bw/2)]. No smoothing
//! at the edges -- Infiltrator calls this "brickwall".

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
        let low_hz = self.freq_hz * (-(self.bw_oct * 0.5)).exp2();
        let high_hz = self.freq_hz * (self.bw_oct * 0.5).exp2();
        let low_bin = (low_hz / bin_hz).floor() as i32;
        let high_bin = (high_hz / bin_hz).ceil() as i32;
        for k in 0..=half {
            if (k as i32) < low_bin || (k as i32) > high_bin {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

pub struct SpectralBandpassEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralBandpassEffect {
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

impl Default for SpectralBandpassEffect {
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

impl Effect for SpectralBandpassEffect {
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

    fn drive(e: &mut SpectralBandpassEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralBandpassEffect::default();
        e.set_param(1, 5000.0);
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn narrow_band_kills_out_of_band_content() {
        let sr = 48_000.0;
        let mut e = SpectralBandpassEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 1000.0);
        // Half-octave passband centred at 1 kHz.
        e.set_param(2, 0.5);
        // Drive a 5 kHz sine -- well outside the passband.
        let out = drive(&mut e, 4096, |i| {
            (2.0 * std::f32::consts::PI * 5000.0 * i as f32 / sr).sin()
        });
        let tail: Vec<f32> = out[2 * e.latency_samples()..].into();
        let peak = tail.iter().cloned().fold(0.0_f32, f32::max);
        assert!(
            peak < 0.1,
            "expected out-of-band 5 kHz to be attenuated to ~0, got peak {peak}"
        );
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralBandpassEffect::default().parameters().len(), 3);
    }
}
