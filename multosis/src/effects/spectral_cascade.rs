//! Spectral Cascade: per-bin delay ramped linearly around a centre frequency.
//!
//! delay_for_bin[k] = round(Length_hops * (k - centre_bin) / (N/2)), clamped
//! to [0, MAX_DELAY_HOPS - 1]. Bins ABOVE Centre get positive delay; bins
//! at or below Centre read "now" (delay 0). Centre low -> low frequencies
//! arrive on time and highs slide in later (upward slide). Centre high ->
//! highs arrive on time and lows slide in (downward slide).
//!
//! Feedback recycles the delayed output back into the ring so taps come
//! around again, building cascading echoes per bin.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

/// Max delay depth in HOPS. At 48 kHz with the largest FFT (4096-pt,
/// hop = 2048), 128 hops = 128 * 2048 / 48000 = 5.4 s of buffer, which
/// safely covers the 2000 ms Length parameter even at small FFT hop
/// sizes (the hop scales with FFT, so smaller FFT = more hops to cover
/// the same wall-clock; we clamp Length_hops to MAX_DELAY_HOPS - 1).
const MAX_DELAY_HOPS: usize = 128;
/// Largest half-spectrum size we ever need (4096 / 2 + 1).
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralCascadeChannel {
    /// Per-bin ring of complex history. Flattened 2D as
    /// `ring[hop_index * MAX_HALF_PLUS_ONE + bin_k]`. Pre-allocated to
    /// the worst case so the audio thread is allocation-free even when
    /// FFT size is modulated.
    ring: Vec<Complex<f32>>,
    /// Current write head (hop index into `ring`).
    write_pos: usize,
}

impl SpectralCascadeChannel {
    fn new() -> Self {
        Self {
            ring: vec![Complex::default(); MAX_DELAY_HOPS * MAX_HALF_PLUS_ONE],
            write_pos: 0,
        }
    }
    fn reset(&mut self) {
        self.ring.fill(Complex::default());
        self.write_pos = 0;
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    length_ms: f32,
    feedback_pct: f32,
    centre_hz: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            length_ms: 200.0,
            feedback_pct: 0.0,
            centre_hz: 1000.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralCascadeChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let hop_samples = (fft_size / 2) as f32;
        // Length in hops -- one hop is `fft_size / 2` samples.
        let length_hops = ((self.params.length_ms * 0.001 * sample_rate) / hop_samples)
            .round()
            .clamp(0.0, (MAX_DELAY_HOPS - 1) as f32) as i32;
        let centre_bin = (self.params.centre_hz / bin_hz).round() as i32;
        let feedback = (self.params.feedback_pct * 0.01).clamp(0.0, 0.95);

        let h_now = self.chan.write_pos;
        for k in 1..=half {
            // Linear ramp: bins above Centre delayed by up to length_hops;
            // bins at/below Centre read "now" (clamped to 0).
            let dk_raw = (k as i32 - centre_bin) * length_hops / (half.max(1) as i32);
            let dk = dk_raw.max(0);

            // Read the delayed tap from `dk` hops back. For dk==0 we
            // want this cycle's input to come straight through, so first
            // write THEN read -- the read slot for dk=0 is the slot we
            // just wrote (the current input). For dk>0 the write doesn't
            // shadow the read because read_h != h_now.
            let pre_input = spectrum[k];
            // Tap to mix into the ring write -- read from `dk` hops back
            // BEFORE we overwrite h_now, so dk>0 sees the historical value.
            // dk=0 reads what's in h_now from MAX_DELAY_HOPS hops ago
            // (zero on the first cycle), but we ignore that value for the
            // feedback path when dk=0 since the output is the input itself.
            let feedback_tap = if dk > 0 {
                let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
                self.chan.ring[read_h * MAX_HALF_PLUS_ONE + k]
            } else {
                Complex::default()
            };

            // Ring stores input + feedback * delayed so each tap recycles.
            let written = Complex::new(
                pre_input.re + feedback * feedback_tap.re,
                pre_input.im + feedback * feedback_tap.im,
            );
            self.chan.ring[h_now * MAX_HALF_PLUS_ONE + k] = written;

            // Output = the value at `dk` hops back. For dk=0 that's the
            // slot we just wrote, i.e. the input -- passthrough.
            let delayed = if dk == 0 {
                written
            } else {
                let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
                self.chan.ring[read_h * MAX_HALF_PLUS_ONE + k]
            };
            spectrum[k] = delayed;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        self.chan.write_pos = (h_now + 1) % MAX_DELAY_HOPS;
    }
}

pub struct SpectralCascadeEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralCascadeChannel,
    chan_r: SpectralCascadeChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralCascadeEffect {
    pub const PARAMS: [ParamSpec; 4] = [
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
            name: "Centre",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
    ];
}

impl Default for SpectralCascadeEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralCascadeChannel::new(),
            chan_r: SpectralCascadeChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralCascadeEffect {
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
            1 => self.params.length_ms = value.clamp(10.0, 2000.0),
            2 => self.params.feedback_pct = value.clamp(0.0, 95.0),
            3 => self.params.centre_hz = value.clamp(20.0, 20_000.0),
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

    fn drive(e: &mut SpectralCascadeEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralCascadeEffect::default().parameters().len(), 4);
    }

    #[test]
    fn length_zero_feedback_zero_is_passthrough() {
        // With Length so small that length_hops rounds to 0, every bin gets
        // delay=0 and feedback=0 -- the ring stores the input verbatim and
        // outputs it unchanged. Effectively passthrough.
        let mut e = SpectralCascadeEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 10.0); // 10 ms -> round(10*48/512) = 1 hop, but the
                              // ramp around Centre keeps most bins at delay 0
        e.set_param(2, 0.0); // Feedback = 0
        e.set_param(3, 20_000.0); // Centre at Nyquist -> nearly every bin is
                                  // BELOW centre -> dk clamped to 0 -> read
                                  // current frame -> passthrough.
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
        let mut e = SpectralCascadeEffect::default();
        e.set_param(2, 50.0); // arbitrary feedback (still bounded < 0.95)
        let out = drive(&mut e, 8192, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
