//! Spectral Scatter: per-bin random delays, refreshed at Rate Hz.
//!
//! Each bin gets an independent random delay in [0, Length_hops). The delays
//! are reassigned every `1 / Rate` seconds. Feedback recycles delayed values
//! back into the ring so taps re-fire (with the new random delays each cycle).
//!
//! Implementation mirrors SpectralCascade's per-bin delay ring -- depth
//! capped at MAX_DELAY_HOPS hops so memory is bounded. Unlike Cascade's
//! linear-ramp delay map, Scatter assigns each bin an INDEPENDENT random
//! delay drawn from [0, Length_hops). Same dk=0 special-case applies:
//! when a bin's random delay rounds to zero we read the just-written
//! slot (i.e. the input), not the stale ring contents.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Per-channel max delay depth in HOPS. Matches SpectralCascade.
const MAX_DELAY_HOPS: usize = 128;
/// Largest half-spectrum size we ever need (4096 / 2 + 1).
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralScatterChannel {
    /// Flattened 2D ring: `ring[hop_index * MAX_HALF_PLUS_ONE + bin_k]`.
    ring: Vec<Complex<f32>>,
    write_pos: usize,
    /// Per-bin random delay assignment in [0, Length_hops).
    delay_per_bin: Vec<u16>,
    hop_counter: u32,
    rng_state: u32,
}

impl SpectralScatterChannel {
    fn new(seed: u32) -> Self {
        Self {
            ring: vec![Complex::default(); MAX_DELAY_HOPS * MAX_HALF_PLUS_ONE],
            write_pos: 0,
            // Default all delays to 0 -- passthrough until the first refresh.
            delay_per_bin: vec![0u16; MAX_HALF_PLUS_ONE],
            hop_counter: 0,
            rng_state: seed.max(1),
        }
    }
    fn reset(&mut self) {
        self.ring.fill(Complex::default());
        self.write_pos = 0;
        self.delay_per_bin.fill(0);
        self.hop_counter = 0;
        // Keep rng_state across reset so successive presses don't give
        // identical delay sequences.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    length_ms: f32,
    feedback_pct: f32,
    rate_hz: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            length_ms: 200.0,
            feedback_pct: 0.0,
            rate_hz: 1.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralScatterChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_hops = (((self.params.length_ms * 0.001 * sample_rate) / hop_samples).round()
            as usize)
            .clamp(1, MAX_DELAY_HOPS - 1);
        let feedback = (self.params.feedback_pct * 0.01).clamp(0.0, 0.95);
        let rate = self.params.rate_hz.max(0.001);
        let hop_rate_hz = sample_rate / hop_samples;
        let hops_per_refresh = (hop_rate_hz / rate).round().max(1.0) as u32;

        // Refresh delay_per_bin every `hops_per_refresh` hops.
        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= hops_per_refresh {
            self.chan.hop_counter = 0;
            for k in 1..=half {
                self.chan.rng_state = xorshift(self.chan.rng_state);
                self.chan.delay_per_bin[k] = (self.chan.rng_state as usize % length_hops) as u16;
            }
        }

        let h_now = self.chan.write_pos;
        for k in 1..=half {
            let dk = self.chan.delay_per_bin[k] as i32;
            // Compute the output value FIRST so we can choose between
            // historical-tap (dk > 0) and just-written (dk == 0).
            let delayed = if dk == 0 {
                // dk == 0 reads "now". The ring at slot h_now hasn't been
                // written for this frame yet; the right answer is the input
                // value (no historical recycling for a zero-delay bin).
                spectrum[k]
            } else {
                let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
                self.chan.ring[read_h * MAX_HALF_PLUS_ONE + k]
            };
            // Write input + feedback * delayed into the ring at h_now.
            let written = Complex::new(
                spectrum[k].re + feedback * delayed.re,
                spectrum[k].im + feedback * delayed.im,
            );
            self.chan.ring[h_now * MAX_HALF_PLUS_ONE + k] = written;
            spectrum[k] = delayed;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        self.chan.write_pos = (h_now + 1) % MAX_DELAY_HOPS;
    }
}

pub struct SpectralScatterEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralScatterChannel,
    chan_r: SpectralScatterChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralScatterEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Length",
            min: 10.0,
            max: 2000.0,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " ms",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Rate",
            min: 0.1,
            max: 10.0,
            default: 1.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
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

impl Default for SpectralScatterEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralScatterChannel::new(0x5CA1_7E01),
            chan_r: SpectralScatterChannel::new(0x5CA1_7E02),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralScatterEffect {
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
            0 => self.params.length_ms = value.clamp(10.0, 2000.0),
            1 => self.params.feedback_pct = value.clamp(0.0, 95.0),
            2 => self.params.rate_hz = value.clamp(0.1, 10.0),
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

    fn drive(e: &mut SpectralScatterEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralScatterEffect::default().parameters().len(), 4);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralScatterEffect::default();
        e.set_param(1, 50.0); // arbitrary Feedback < 95 (slot 1)
        let out = drive(&mut e, 8192, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn xorshift_never_returns_zero() {
        let mut s = 1u32;
        for _ in 0..1000 {
            s = xorshift(s);
            assert!(s != 0);
        }
    }
}
