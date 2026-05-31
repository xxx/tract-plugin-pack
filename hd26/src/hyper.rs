//! HD26 "Hyper" stage: a multi-voice modulated fractional-delay chorus.
//! Per-channel delay lines; N read taps (voices), each swept by its own LFO so
//! its pitch oscillates sharp/flat (Doppler). Voice detune spread follows a
//! Szabo-style asymmetric JP-8000 shape. Width decorrelates the two channels
//! for stereo spread (mono-safe at 0). Retrig zeros the voice LFO phases.

use crate::delay::DelayLine;
use crate::lfo::Phasor;

pub const MAX_VOICES: usize = 7;

const BASE_DELAY_MS: f32 = 8.0;
const VOICE_SPACING_MS: f32 = 0.5;
const MAX_DEPTH_MS: f32 = 25.0;
const MAX_CENTS: f32 = 50.0;
/// 1200 / ln(2) ≈ 1731 — cents per unit fractional pitch deviation.
const CENTS_PER_RATIO: f32 = 1731.234;

/// Normalized Szabo JP-8000 voice offsets in [-1, 1] (index 3 = center 0.0).
const VOICE_SHAPE: [f32; MAX_VOICES] =
    [-1.0, -0.5715, -0.1774, 0.0, 0.1810, 0.5651, 0.9766];

#[derive(Clone, Copy)]
pub struct HyperParams {
    pub voices: usize, // 0..=MAX_VOICES (0 = stage bypass)
    pub detune: f32,   // 0..1
    pub rate_hz: f32,
    pub width: f32, // 0..1
    pub mix: f32,   // 0..1
}

pub struct Hyper {
    delay_l: DelayLine,
    delay_r: DelayLine,
    phasors: [Phasor; MAX_VOICES],
    sample_rate: f32,
}

impl Hyper {
    pub fn new(sample_rate: f32) -> Self {
        let cap = Self::ring_capacity(sample_rate);
        Self {
            delay_l: DelayLine::new(cap),
            delay_r: DelayLine::new(cap),
            phasors: Self::initial_phasors(),
            sample_rate,
        }
    }

    fn initial_phasors() -> [Phasor; MAX_VOICES] {
        let mut p = [Phasor::new(0.0); MAX_VOICES];
        for (i, ph) in p.iter_mut().enumerate() {
            *ph = Phasor::new(i as f32 / MAX_VOICES as f32);
        }
        p
    }

    fn ring_capacity(sample_rate: f32) -> usize {
        let max_ms =
            BASE_DELAY_MS + (MAX_VOICES as f32 - 1.0) * VOICE_SPACING_MS + MAX_DEPTH_MS;
        ((max_ms / 1000.0 * sample_rate).ceil() as usize) + 8
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let cap = Self::ring_capacity(sample_rate);
        self.delay_l = DelayLine::new(cap);
        self.delay_r = DelayLine::new(cap);
        self.sample_rate = sample_rate;
    }

    pub fn reset(&mut self) {
        self.delay_l.reset();
        self.delay_r.reset();
        self.phasors = Self::initial_phasors();
    }

    /// Reset all voice LFO phases to their initial stagger — the Retrig "zap".
    pub fn retrig(&mut self) {
        self.phasors = Self::initial_phasors();
    }

    /// Which VOICE_SHAPE entry voice `i` of `n` uses (samples the 7-pt shape so
    /// the spread stays Szabo-asymmetric for any n; center for n==1).
    #[inline]
    fn shape_index(i: usize, n: usize) -> usize {
        if n <= 1 {
            return 3;
        }
        let u = i as f32 / (n as f32 - 1.0); // 0..1
        (u * (MAX_VOICES as f32 - 1.0)).round() as usize
    }

    /// Per-voice LFO modulation depth in samples, derived from the target peak
    /// pitch deviation `cents` via the Doppler relation
    /// `cents = CENTS_PER_RATIO * 2π * rate * depth_seconds`, then clamped so the
    /// sinusoidal sweep's trough (`voice_base - depth`) never undershoots
    /// `read_cubic`'s valid range (>= 2 samples). Without the `voice_base - 2`
    /// clamp, slow rates + high detune push the trough below 2 samples, where
    /// `read_cubic` silently clamps and flattens the bottom of the sweep,
    /// producing asymmetric/incorrect detune. `rate` must be > 0 (the caller
    /// floors it at 0.01 Hz).
    #[inline]
    fn voice_depth(cents: f32, rate: f32, sr: f32, voice_base: f32, max_depth: f32) -> f32 {
        let raw = (cents.abs() / (CENTS_PER_RATIO * std::f32::consts::TAU * rate)) * sr;
        raw.min(max_depth).min((voice_base - 2.0).max(0.0))
    }

    #[inline]
    pub fn process_sample(&mut self, l: f32, r: f32, p: &HyperParams) -> (f32, f32) {
        let n = p.voices.min(MAX_VOICES);
        if n == 0 {
            return (l, r); // stage bypass
        }

        self.delay_l.write(l);
        self.delay_r.write(r);

        let sr = self.sample_rate;
        let base = BASE_DELAY_MS / 1000.0 * sr;
        let spacing = VOICE_SPACING_MS / 1000.0 * sr;
        let max_depth = MAX_DEPTH_MS / 1000.0 * sr;
        let detune_curve = p.detune * p.detune; // subtle low, steeper high
        let rate = p.rate_hz.max(0.01);
        let gain = 1.0 / (n as f32).sqrt();
        let width_shift = 0.25 * p.width.clamp(0.0, 1.0); // up to ¼-cycle R offset

        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for i in 0..n {
            let shape = VOICE_SHAPE[Self::shape_index(i, n)];
            let cents = shape * MAX_CENTS * detune_curve;
            let voice_base = base + i as f32 * spacing;
            let depth = Self::voice_depth(cents, rate, sr, voice_base, max_depth);
            let ph = &self.phasors[i];
            let lfo_l = ph.sine();
            let lfo_r = ph.sine_at_offset(width_shift);
            wet_l += self.delay_l.read_cubic(voice_base + depth * lfo_l);
            wet_r += self.delay_r.read_cubic(voice_base + depth * lfo_r);
        }
        wet_l *= gain;
        wet_r *= gain;

        // Advance phasors; tiny per-voice rate spread decorrelates the wobble.
        let center = (n as f32 - 1.0) * 0.5;
        for (i, ph) in self.phasors.iter_mut().enumerate().take(n) {
            let rate_i = rate * (1.0 + 0.03 * (i as f32 - center));
            ph.advance(rate_i, sr);
        }

        let mix = p.mix.clamp(0.0, 1.0);
        (l + (wet_l - l) * mix, r + (wet_r - r) * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(voices: usize, detune: f32, width: f32, mix: f32) -> HyperParams {
        HyperParams {
            voices,
            detune,
            rate_hz: 1.0,
            width,
            mix,
        }
    }

    #[test]
    fn zero_voices_is_exact_passthrough() {
        let mut h = Hyper::new(48_000.0);
        let p = params(0, 0.5, 0.5, 1.0);
        for n in 0..256 {
            let x = (0.1 * n as f32).sin();
            let (ol, or) = h.process_sample(x, -x, &p);
            assert_eq!(ol, x);
            assert_eq!(or, -x);
        }
    }

    #[test]
    fn mono_input_stays_mono_at_zero_width() {
        let mut h = Hyper::new(48_000.0);
        let p = params(7, 0.8, 0.0, 1.0);
        for n in 0..2000 {
            let x = (0.07 * n as f32).sin() * 0.5;
            let (ol, or) = h.process_sample(x, x, &p);
            assert!((ol - or).abs() < 1e-5, "channels diverged at {n}: {ol} vs {or}");
        }
    }

    #[test]
    fn width_decorrelates_channels() {
        let mut h = Hyper::new(48_000.0);
        let p = params(7, 0.8, 1.0, 1.0);
        let mut max_diff = 0.0f32;
        for n in 0..4000 {
            let x = (0.07 * n as f32).sin() * 0.5;
            let (ol, or) = h.process_sample(x, x, &p);
            max_diff = max_diff.max((ol - or).abs());
        }
        assert!(max_diff > 1e-3, "width should decorrelate L/R, max_diff={max_diff}");
    }

    #[test]
    fn output_is_finite() {
        let mut h = Hyper::new(48_000.0);
        let p = params(7, 1.0, 1.0, 1.0);
        for n in 0..8000 {
            let x = (0.13 * n as f32).sin() * 0.9;
            let (ol, or) = h.process_sample(x, x * 0.8, &p);
            assert!(ol.is_finite() && or.is_finite(), "non-finite at {n}");
        }
    }

    #[test]
    fn retrig_resets_voice_phases() {
        let mut h = Hyper::new(48_000.0);
        let p = params(7, 0.5, 0.5, 1.0);
        for _ in 0..500 {
            h.process_sample(0.3, 0.3, &p);
        }
        h.retrig();
        // Voice 0's initial phase is 0.0.
        assert_eq!(h.phasors[0].phase(), 0.0);
        // Voice 1's initial phase is 1/7.
        assert!((h.phasors[1].phase() - 1.0 / 7.0).abs() < 1e-6);
    }

    #[test]
    fn modulation_trough_stays_in_valid_range() {
        // Regression: at slow rates + high detune the derived depth must be
        // clamped so the LFO trough (voice_base - depth) never undershoots
        // read_cubic's 2-sample floor (which would silently flatten the sweep).
        let sr = 48_000.0;
        let base = BASE_DELAY_MS / 1000.0 * sr;
        let spacing = VOICE_SPACING_MS / 1000.0 * sr;
        let max_depth = MAX_DEPTH_MS / 1000.0 * sr;
        for &rate in &[0.01f32, 0.05, 0.1, 0.3, 0.5, 1.0, 5.0, 10.0] {
            for i in 0..MAX_VOICES {
                let voice_base = base + i as f32 * spacing;
                let cents = MAX_CENTS; // worst case |shape| ~= 1.0, detune_curve = 1.0
                let depth = Hyper::voice_depth(cents, rate, sr, voice_base, max_depth);
                assert!(
                    voice_base - depth >= 2.0 - 1e-3,
                    "trough underflow at rate={rate} voice={i}: base={voice_base} depth={depth}"
                );
            }
        }
    }
}
