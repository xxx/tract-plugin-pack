//! Spectral Twist: fold in-band bins toward (or away from) a centre frequency.
//!
//! Within +/- Bandwidth/2 octaves of Freq, each source bin at offset d from
//! the centre maps to a destination at offset d * (1 - Twist). Twist=+1
//! collapses the band onto Freq (all in-band bins -> centre). Twist=-1
//! doubles their spread. Twist=0 is identity. Out-of-band passes through.
//!
//! Multiple sources can map to the same destination at Twist > 0 -- we
//! accumulate (add) into the destination so energy isn't lost.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    freq_hz: f32,
    twist_pct: f32, // -100..+100
    bw_oct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            freq_hz: 1000.0,
            twist_pct: 0.0,
            bw_oct: 1.0,
        }
    }
}

struct TransformCtx {
    freq_hz: f32,
    twist_pct: f32,
    bw_oct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let twist = (self.twist_pct * 0.01).clamp(-1.0, 1.0);
        if twist.abs() < 1e-3 {
            return;
        }
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins =
            (((self.freq_hz * (self.bw_oct * 0.5).exp2()) - self.freq_hz) / bin_hz).ceil() as i32;
        let scale = 1.0 - twist; // twist=+1 -> scale=0 (collapse), twist=-1 -> scale=2 (spread)

        // Stash positive half into negative-half slots so we can read sources
        // while writing destinations.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k];
        }
        // Zero in-band destinations so we can accumulate cleanly.
        for k in 1..half as i32 {
            let d = k - centre_bin;
            if d.abs() <= band_bins {
                spectrum[k as usize] = Complex::default();
            }
        }
        // Map each in-band source to its scaled destination, accumulating.
        for d_src in -band_bins..=band_bins {
            let src = centre_bin + d_src;
            if src < 1 || (src as usize) >= half {
                continue;
            }
            let d_dst_f = d_src as f32 * scale;
            let dst = centre_bin + d_dst_f.round() as i32;
            if dst < 1 || (dst as usize) >= half {
                continue;
            }
            let src_val = spectrum[fft_size - src as usize]; // from stash
            spectrum[dst as usize].re += src_val.re;
            spectrum[dst as usize].im += src_val.im;
        }
        // Rebuild negative half from conjugates of the rewritten positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralTwistEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralTwistEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Twist",
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

impl Default for SpectralTwistEffect {
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

impl Effect for SpectralTwistEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            freq_hz: self.params.freq_hz,
            twist_pct: self.params.twist_pct,
            bw_oct: self.params.bw_oct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            freq_hz: self.params.freq_hz,
            twist_pct: self.params.twist_pct,
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
            0 => self.params.freq_hz = value.clamp(20.0, 20_000.0),
            1 => self.params.twist_pct = value.clamp(-100.0, 100.0),
            2 => self.params.bw_oct = value.clamp(0.1, 4.0),
            3 => {
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

    fn drive(e: &mut SpectralTwistEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralTwistEffect::default().parameters().len(), 4);
    }

    #[test]
    fn twist_zero_is_passthrough() {
        let mut e = SpectralTwistEffect::default();
        e.set_param(3, 1.0); // FFT = 1024 (slot 3)
        e.set_param(1, 0.0); // Twist = 0 -> early return (slot 1)
        let f = 1000.0;
        let sr = 48_000.0;
        let out = drive(&mut e, 4096, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralTwistEffect::default();
        e.set_param(1, 50.0); // arbitrary twist (slot 1)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
