//! Spectral Reverb: per-bin one-pole feedback with frequency-dependent T60.
//!
//! Each bin maintains a running tail: tail_k = tail_k * g_k + input_k.
//! The per-bin gain g_k is derived from a target T60 (time to drop 60 dB),
//! which is shaped by Tone: at Tone=0 ("dark") HF bins decay fast (T60 = 0.1
//! \* Time); at Tone=1 ("bright") LF bins decay fast. Tone=0.5 is roughly
//! uniform decay at Time across the spectrum.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralReverbChannel {
    /// Per-bin running tail (complex, so phase advances naturally each hop).
    tail: Vec<Complex<f32>>,
}

impl SpectralReverbChannel {
    fn new() -> Self {
        Self {
            tail: vec![Complex::default(); MAX_HALF_PLUS_ONE],
        }
    }
    fn reset(&mut self) {
        self.tail.fill(Complex::default());
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    time_s: f32,
    tone_pct: f32, // 0..100
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            time_s: 2.0,
            tone_pct: 50.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralReverbChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let hop_samples = (fft_size / 2) as f32;
        let time_s = self.params.time_s.max(0.001);
        let tone = (self.params.tone_pct * 0.01).clamp(0.0, 1.0);

        for k in 1..=half {
            // Bin centre frequency, log-normalised 0..1 from 20 Hz to 20 kHz.
            let f_hz = (k as f32 * bin_hz).max(1.0);
            let f_norm = ((f_hz / 20.0).ln() / (20_000.0_f32 / 20.0).ln()).clamp(0.0, 1.0);
            // Damping factor per bin: 1.0 = full Time, 0.1 = 10% of Time.
            // bright curve: LF damped (low f_norm -> 0.1), HF preserved -> 1.0.
            let bright_damping = 1.0 - 0.9 * (1.0 - f_norm);
            // dark curve: HF damped (high f_norm -> 0.1), LF preserved -> 1.0.
            let dark_damping = 1.0 - 0.9 * f_norm;
            // Lerp between dark (tone=0) and bright (tone=1).
            let damp = (1.0 - tone) * dark_damping + tone * bright_damping;
            let t60_k = time_s * damp;
            // Per-hop decay coefficient: by definition T60 is the time for
            // amplitude to fall by 60 dB = 10^-3. Solve g^N = 10^-3 with
            // N = t60_samples / hop_samples, giving
            // g = 10^(-3 * hop_samples / (t60_k * sample_rate)).
            let g = 10.0_f32.powf(-3.0 * hop_samples / (t60_k * sample_rate));
            self.chan.tail[k].re = self.chan.tail[k].re * g + spectrum[k].re;
            self.chan.tail[k].im = self.chan.tail[k].im * g + spectrum[k].im;
            spectrum[k] = self.chan.tail[k];
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        // DC (k=0) is untouched.
    }
}

pub struct SpectralReverbEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralReverbChannel,
    chan_r: SpectralReverbChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralReverbEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Time",
            min: 0.1,
            max: 20.0,
            default: 2.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 2,
                unit: " s",
            },
        },
        ParamSpec {
            name: "Tone",
            min: 0.0,
            max: 100.0,
            default: 50.0,
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

impl Default for SpectralReverbEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralReverbChannel::new(),
            chan_r: SpectralReverbChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralReverbEffect {
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
            0 => self.params.time_s = value.clamp(0.1, 20.0),
            1 => self.params.tone_pct = value.clamp(0.0, 100.0),
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

    fn drive(e: &mut SpectralReverbEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralReverbEffect::default().parameters().len(), 3);
    }

    #[test]
    fn silence_in_eventually_silence_out() {
        // Reverb has an indefinite tail by design, so the FIRST samples after
        // input dies aren't zero -- we drive a long silence and check that
        // the tail decays toward zero over Time-many seconds. With Time=0.1 s
        // (the minimum) and ~50 ms of "pulse" input, after ~1 s of silence
        // the tail should be very quiet.
        let mut e = SpectralReverbEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.1); // Time = 100 ms (slot 0)
        e.set_param(1, 50.0); // Tone = neutral (slot 1)
        let sr = 48_000.0;
        let burst = (0.050 * sr) as usize;
        let tail = (1.0 * sr) as usize; // 1 s of silence
        let out = drive(&mut e, burst + tail, |i| if i < burst { 0.5 } else { 0.0 });
        // Last 100 ms of tail should be very quiet (well below burst level).
        let final_100ms = &out[out.len() - (0.1 * sr) as usize..];
        let max_late: f32 = final_100ms
            .iter()
            .cloned()
            .fold(0.0_f32, |a, x| a.max(x.abs()));
        assert!(
            max_late < 0.05,
            "100 ms reverb should be ~quiet 1 s after burst; got max={max_late}"
        );
    }

    #[test]
    fn reverb_tail_outlives_input() {
        // Drive a 50 ms burst at Time = 2 s; observe non-zero output 200 ms
        // after the burst ends.
        let mut e = SpectralReverbEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 2.0); // Time = 2 s (slot 0)
        let sr = 48_000.0_f32;
        let burst = (0.050 * sr) as usize;
        let tail = (0.200 * sr) as usize;
        let out = drive(&mut e, burst + tail, |i| {
            if i < burst {
                (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin() * 0.5
            } else {
                0.0
            }
        });
        let tail_energy: f32 = out[burst + 2 * e.latency_samples()..]
            .iter()
            .map(|x| x * x)
            .sum();
        assert!(
            tail_energy > 0.001,
            "expected non-zero reverb tail; got {tail_energy}"
        );
    }
}
