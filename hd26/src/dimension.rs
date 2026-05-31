//! HD26 "Dimension" stage: a pseudo-stereo widener.
//!
//! AM mode (Serum-accurate, default): 4 delay taps off the mono sum, each
//! scaled by a slow amplitude LFO, summed out-of-phase, and injected as a pure
//! Side signal (+L / −R). Because the wet is added antisymmetrically, `L+R`
//! returns the dry mono exactly — perfect mono compatibility at every Mix.
//!
//! Pitch mode (Dimension-D flavor): delay-time modulation with opposite-
//! polarity cross-feed (classic BBD vibrato-chorus character; not mono-safe).
//!
//! A one-pole high-pass on the wet/side path keeps the low end out of the
//! stereo field (more mono-solid bass).

use crate::delay::DelayLine;
use crate::lfo::Phasor;

const NUM_TAPS: usize = 4;
const TAP_RATIOS: [f32; NUM_TAPS] = [0.6, 0.85, 1.0, 1.4];
const TAP_SIGNS: [f32; NUM_TAPS] = [1.0, -1.0, 1.0, -1.0];
const AM_RATES: [f32; NUM_TAPS] = [0.18, 0.27, 0.41, 0.55];
const AM_PHASES: [f32; NUM_TAPS] = [0.0, 0.25, 0.5, 0.75];
const AM_DEPTH: f32 = 0.3;

const SIZE_MIN_MS: f32 = 1.0;
const SIZE_MAX_MS: f32 = 20.0;

// Pitch mode
const PITCH_DEPTH_MS: f32 = 2.0;
const PITCH_RATE_HZ: f32 = 0.4;
const PITCH_CROSS: f32 = 0.3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DimMode {
    Am,
    Pitch,
}

#[derive(Clone, Copy)]
pub struct DimParams {
    pub size: f32, // 0..1
    pub mode: DimMode,
    pub hpf_hz: f32,
    pub mix: f32, // 0..1 (0 = stage disabled)
}

/// One-pole RC high-pass on the wet/side path.
#[derive(Clone, Copy)]
struct OnePoleHp {
    x_prev: f32,
    y_prev: f32,
    coeff: f32,
}

impl OnePoleHp {
    fn new() -> Self {
        Self {
            x_prev: 0.0,
            y_prev: 0.0,
            coeff: 0.0,
        }
    }

    fn set_cutoff(&mut self, hz: f32, sr: f32) {
        let rc = 1.0 / (std::f32::consts::TAU * hz.max(1.0));
        let dt = 1.0 / sr.max(1.0);
        self.coeff = rc / (rc + dt);
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.coeff * (self.y_prev + x - self.x_prev);
        self.x_prev = x;
        self.y_prev = y;
        y
    }

    fn reset(&mut self) {
        self.x_prev = 0.0;
        self.y_prev = 0.0;
    }
}

pub struct Dimension {
    delay_mono: DelayLine,
    delay_l: DelayLine,
    delay_r: DelayLine,
    am_phasors: [Phasor; NUM_TAPS],
    pitch_phasor: Phasor,
    hpf_a: OnePoleHp,
    hpf_b: OnePoleHp,
    sample_rate: f32,
}

impl Dimension {
    pub fn new(sample_rate: f32) -> Self {
        let cap = Self::ring_capacity(sample_rate);
        let mut am_phasors = [Phasor::new(0.0); NUM_TAPS];
        for (i, ph) in am_phasors.iter_mut().enumerate() {
            *ph = Phasor::new(AM_PHASES[i]);
        }
        let mut s = Self {
            delay_mono: DelayLine::new(cap),
            delay_l: DelayLine::new(cap),
            delay_r: DelayLine::new(cap),
            am_phasors,
            pitch_phasor: Phasor::new(0.0),
            hpf_a: OnePoleHp::new(),
            hpf_b: OnePoleHp::new(),
            sample_rate,
        };
        s.hpf_a.set_cutoff(120.0, sample_rate);
        s.hpf_b.set_cutoff(120.0, sample_rate);
        s
    }

    fn ring_capacity(sr: f32) -> usize {
        // Longest tap = SIZE_MAX_MS * max(TAP_RATIOS) = 20 * 1.4 = 28 ms; add
        // the pitch-mode mod swing margin.
        let max_ms = SIZE_MAX_MS * 1.4 + PITCH_DEPTH_MS + 4.0;
        ((max_ms / 1000.0 * sr).ceil() as usize) + 8
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        let cap = Self::ring_capacity(sr);
        self.delay_mono = DelayLine::new(cap);
        self.delay_l = DelayLine::new(cap);
        self.delay_r = DelayLine::new(cap);
        self.hpf_a.set_cutoff(120.0, sr);
        self.hpf_b.set_cutoff(120.0, sr);
        self.sample_rate = sr;
    }

    pub fn reset(&mut self) {
        self.delay_mono.reset();
        self.delay_l.reset();
        self.delay_r.reset();
        self.hpf_a.reset();
        self.hpf_b.reset();
        for (i, ph) in self.am_phasors.iter_mut().enumerate() {
            *ph = Phasor::new(AM_PHASES[i]);
        }
        self.pitch_phasor = Phasor::new(0.0);
    }

    #[inline]
    fn size_to_base_samples(&self, size: f32) -> f32 {
        let s = size.clamp(0.0, 1.0);
        let ms = SIZE_MIN_MS * (SIZE_MAX_MS / SIZE_MIN_MS).powf(s);
        ms / 1000.0 * self.sample_rate
    }

    #[inline]
    pub fn process_sample(&mut self, l: f32, r: f32, p: &DimParams) -> (f32, f32) {
        let mix = p.mix.clamp(0.0, 1.0);
        if mix <= 0.0 {
            return (l, r); // stage disabled
        }
        self.hpf_a.set_cutoff(p.hpf_hz, self.sample_rate);
        self.hpf_b.set_cutoff(p.hpf_hz, self.sample_rate);
        match p.mode {
            DimMode::Am => self.process_am(l, r, p.size, mix),
            DimMode::Pitch => self.process_pitch(l, r, p.size, mix),
        }
    }

    #[inline]
    fn process_am(&mut self, l: f32, r: f32, size: f32, mix: f32) -> (f32, f32) {
        let mono = (l + r) * 0.5;
        self.delay_mono.write(mono);
        let base = self.size_to_base_samples(size);

        let mut wet = 0.0;
        for i in 0..NUM_TAPS {
            let d = (base * TAP_RATIOS[i]).max(2.0);
            let lfo = self.am_phasors[i].sine();
            let g = 1.0 - AM_DEPTH * 0.5 * (1.0 - lfo); // tremolo in [1-AM_DEPTH, 1]
            wet += TAP_SIGNS[i] * g * self.delay_mono.read_cubic(d);
        }
        wet *= 1.0 / NUM_TAPS as f32;

        let side = self.hpf_a.process(wet);

        for (i, ph) in self.am_phasors.iter_mut().enumerate() {
            ph.advance(AM_RATES[i], self.sample_rate);
        }

        // Antisymmetric injection: L+R = (l+side)+(r-side) = l+r → mono-safe.
        (l + mix * side, r - mix * side)
    }

    #[inline]
    fn process_pitch(&mut self, l: f32, r: f32, size: f32, mix: f32) -> (f32, f32) {
        self.delay_l.write(l);
        self.delay_r.write(r);
        let base = self.size_to_base_samples(size);
        let depth = PITCH_DEPTH_MS / 1000.0 * self.sample_rate;
        let lfo = self.pitch_phasor.sine();
        let d_l = (base + depth * lfo).max(2.0);
        let d_r = (base - depth * lfo).max(2.0);
        let raw_l = self.delay_l.read_cubic(d_l);
        let raw_r = self.delay_r.read_cubic(d_r);

        // Opposite-polarity cross-feed, high-passed.
        let wet_l = raw_l - PITCH_CROSS * self.hpf_a.process(raw_r);
        let wet_r = raw_r - PITCH_CROSS * self.hpf_b.process(raw_l);

        self.pitch_phasor.advance(PITCH_RATE_HZ, self.sample_rate);

        (l + (wet_l - l) * mix, r + (wet_r - r) * mix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn am(size: f32, mix: f32) -> DimParams {
        DimParams {
            size,
            mode: DimMode::Am,
            hpf_hz: 120.0,
            mix,
        }
    }

    #[test]
    fn mix_zero_is_passthrough() {
        let mut d = Dimension::new(48_000.0);
        let p = am(0.5, 0.0);
        for n in 0..512 {
            let x = (0.1 * n as f32).sin();
            let (ol, or) = d.process_sample(x, x * 0.5, &p);
            assert_eq!(ol, x);
            assert_eq!(or, x * 0.5);
        }
    }

    #[test]
    fn am_mode_is_mono_compatible() {
        // L+R must equal the dry mono sum at every Mix (wet is pure Side).
        for &mix in &[0.25f32, 0.5, 1.0] {
            let mut d = Dimension::new(48_000.0);
            let p = am(0.4, mix);
            for n in 0..4000 {
                let l = (0.07 * n as f32).sin() * 0.6;
                let r = (0.05 * n as f32 + 1.0).sin() * 0.6;
                let (ol, or) = d.process_sample(l, r, &p);
                assert!(
                    ((ol + or) - (l + r)).abs() < 1e-4,
                    "mono sum changed at mix={mix} n={n}: {} vs {}",
                    ol + or,
                    l + r
                );
            }
        }
    }

    #[test]
    fn am_mode_widens_mono_input() {
        let mut d = Dimension::new(48_000.0);
        let p = am(0.5, 0.8);
        let mut max_diff = 0.0f32;
        for n in 0..4000 {
            let x = (0.09 * n as f32).sin() * 0.5;
            let (ol, or) = d.process_sample(x, x, &p);
            max_diff = max_diff.max((ol - or).abs());
        }
        assert!(max_diff > 1e-3, "AM should widen mono input, max_diff={max_diff}");
    }

    #[test]
    fn pitch_mode_is_finite_and_passes_at_zero_mix() {
        let mut d = Dimension::new(48_000.0);
        let zero = DimParams {
            size: 0.5,
            mode: DimMode::Pitch,
            hpf_hz: 120.0,
            mix: 0.0,
        };
        let (ol, or) = d.process_sample(0.3, 0.3, &zero);
        assert_eq!((ol, or), (0.3, 0.3));

        let p = DimParams {
            size: 0.5,
            mode: DimMode::Pitch,
            hpf_hz: 120.0,
            mix: 0.7,
        };
        for n in 0..4000 {
            let x = (0.11 * n as f32).sin() * 0.5;
            let (ol, or) = d.process_sample(x, x, &p);
            assert!(ol.is_finite() && or.is_finite(), "non-finite at {n}");
        }
    }
}
