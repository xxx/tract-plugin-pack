//! Velvet sequence generation (GUI thread). Turns the three drawn curves +
//! Size/Density/Width/Seed into a sparse signed pulse train with per-pulse
//! coloration routing and per-pulse right-channel jitter.

use tiny_skia_widgets::mseg::{warp, MsegData, MsegNode, Polarity};

use crate::coloration::Q;
use crate::rng::Rng;

/// Inputs to a generation pass.
#[derive(Clone, Copy)]
pub struct GenParams {
    pub sample_rate: f32,
    /// Tail length, seconds.
    pub size_s: f32,
    /// Pulse density, pulses/second.
    pub density: f32,
    /// Max right-channel jitter at Width=1.0, in milliseconds.
    pub width_ms: f32,
    pub seed: u64,
}

/// Forward-only curve sampler (monotonic `phase`), mirrors miff's `curve_value`.
fn curve_value(data: &MsegData, phase: f32, seg: &mut usize) -> f32 {
    let a = data.active();
    let last = data.node_count - 1;
    if phase >= a[last].time {
        return a[last].value;
    }
    while *seg < last - 1 && a[*seg + 1].time <= phase {
        *seg += 1;
    }
    let n0 = a[*seg];
    let n1 = a[*seg + 1];
    if n0.stepped {
        return n0.value;
    }
    let span = n1.time - n0.time;
    let t = if span > 1e-9 { (phase - n0.time) / span } else { 0.0 };
    n0.value + (n1.value - n0.value) * warp(t, n0.tension)
}

/// Generate the velvet sequence into `out` (pre-allocated to `MAX_PULSES`).
/// Deterministic in `(params, curves)`. Energy-normalized so `Σ coeff² == 1`
/// (when any pulse has non-zero gain). Runs on the GUI thread only.
pub fn generate(
    out: &mut VelvetSequence,
    params: &GenParams,
    decay: &MsegData,
    width: &MsegData,
    tone: &MsegData,
) {
    let fs = params.sample_rate.max(1.0);
    let l_samples = ((params.size_s.max(0.01)) * fs) as usize;
    let td = (fs / params.density.max(1.0)).max(1.0);
    let m_count = ((l_samples as f32 / td) as usize).min(MAX_PULSES);
    let j_max = (params.width_ms * 0.001 * fs).max(0.0); // samples at Width=1

    let mut rng = Rng::new(params.seed);
    let (mut sd, mut sw, mut st) = (0usize, 0usize, 0usize);
    let denom = l_samples.max(1) as f32;

    let mut energy = 0.0f64;
    let mut max_loc = 0u32;

    for m in 0..m_count {
        // Location: one jittered pulse per grid cell.
        let r_loc = rng.next_f32();
        let k = (m as f32 * td + r_loc * (td - 1.0)).round().max(0.0) as u32;
        let phase = (k as f32 / denom).clamp(0.0, 1.0);

        // Sign.
        let sign = if rng.next_f32() < 0.5 { -1.0 } else { 1.0 };

        // Decay → gain.
        let g = curve_value(decay, phase, &mut sd).clamp(0.0, 1.0);
        let coeff = sign * g;

        // Tone → nearest dictionary filter (0 = darkest, Q-1 = brightest).
        let t = curve_value(tone, phase, &mut st).clamp(0.0, 1.0);
        let filter_idx = (t * (Q - 1) as f32).round() as u8;

        // Width → per-pulse max jitter; right channel offset in ±j samples.
        let w = curve_value(width, phase, &mut sw).clamp(0.0, 1.0);
        let j = (w * j_max).round();
        let r_jit = rng.next_f32(); // [0,1) → symmetric [-j, +j]
        let delta = ((r_jit * 2.0 - 1.0) * j).round() as i64;
        let k_r = (k as i64 + delta).max(0) as u32;

        out.location[m] = k;
        out.coeff[m] = coeff;
        out.filter_idx[m] = filter_idx.min((Q - 1) as u8);
        out.location_r[m] = k_r;

        energy += (coeff as f64) * (coeff as f64);
        max_loc = max_loc.max(k).max(k_r);
    }

    // Energy-normalize coefficients so output level is independent of M / shape.
    if energy > 1e-20 {
        let inv = (1.0 / energy.sqrt()) as f32;
        for c in out.coeff[..m_count].iter_mut() {
            *c *= inv;
        }
    }

    out.count = m_count;
    out.tail_len = if m_count == 0 { 0 } else { (max_loc as usize) + 1 };
}

/// Hard upper bound on pulse count: max Size (10 s) × max Density (4000/s) at
/// 48 kHz, rounded up. Buffers are pre-allocated to this so generation and
/// handoff never reallocate.
pub const MAX_PULSES: usize = 48_000;

/// A baked velvet sequence. Buffers are pre-allocated to `MAX_PULSES`; only
/// `[..count]` is meaningful. Cloning/copying is `[..count]` slices.
#[derive(Clone)]
pub struct VelvetSequence {
    pub count: usize,
    /// Left-channel pulse sample offsets (ascending), into the input ring.
    pub location: Vec<u32>,
    /// Signed decay gain per pulse: `s(m)·g(m)`, energy-normalized.
    pub coeff: Vec<f32>,
    /// Dictionary filter index per pulse (`0..Q`).
    pub filter_idx: Vec<u8>,
    /// Right-channel pulse sample offsets (jittered copy of `location`).
    pub location_r: Vec<u32>,
    /// Total tail length in samples (max of L/R locations + 1).
    pub tail_len: usize,
}

impl VelvetSequence {
    pub fn new() -> Self {
        Self {
            count: 0,
            location: vec![0; MAX_PULSES],
            coeff: vec![0.0; MAX_PULSES],
            filter_idx: vec![0; MAX_PULSES],
            location_r: vec![0; MAX_PULSES],
            tail_len: 0,
        }
    }

    /// Copy `other[..count]` into `self` without reallocating.
    pub fn copy_from(&mut self, other: &VelvetSequence) {
        self.count = other.count;
        self.tail_len = other.tail_len;
        self.location[..self.count].copy_from_slice(&other.location[..self.count]);
        self.coeff[..self.count].copy_from_slice(&other.coeff[..self.count]);
        self.filter_idx[..self.count].copy_from_slice(&other.filter_idx[..self.count]);
        self.location_r[..self.count].copy_from_slice(&other.location_r[..self.count]);
    }
}

impl Default for VelvetSequence {
    fn default() -> Self {
        Self::new()
    }
}

/// Default Decay curve: full at the start, decaying to silence — an
/// exponential-ish fall via positive tension. Unipolar.
pub fn default_decay_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 1.0, tension: 0.6, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

/// Default Width curve: a moderate, constant 0.5 width across the tail.
pub fn default_width_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 0.5, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.5, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

/// Default Tone curve: bright at the start, darkening over the tail (air
/// absorption). Value 1.0 = brightest dictionary filter, 0.0 = darkest.
pub fn default_tone_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 0.85, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.25, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia_widgets::mseg::value_at_phase;

    fn flat(value: f32) -> MsegData {
        let mut d = MsegData::default();
        d.nodes[0] = MsegNode { time: 0.0, value, tension: 0.0, stepped: false };
        d.nodes[1] = MsegNode { time: 1.0, value, tension: 0.0, stepped: false };
        d.debug_assert_valid();
        d
    }

    fn test_params() -> GenParams {
        GenParams { sample_rate: 48_000.0, size_s: 1.0, density: 1500.0, width_ms: 5.0, seed: 1 }
    }

    #[test]
    fn default_curves_are_valid() {
        assert!(default_decay_curve().is_valid());
        assert!(default_width_curve().is_valid());
        assert!(default_tone_curve().is_valid());
    }

    #[test]
    fn copy_from_preserves_active_region() {
        let mut a = VelvetSequence::new();
        a.count = 3;
        a.tail_len = 99;
        a.location[..3].copy_from_slice(&[1, 2, 3]);
        a.coeff[..3].copy_from_slice(&[0.1, 0.2, 0.3]);
        a.filter_idx[..3].copy_from_slice(&[0, 1, 2]);
        a.location_r[..3].copy_from_slice(&[1, 4, 9]);
        let mut b = VelvetSequence::new();
        b.copy_from(&a);
        assert_eq!(b.count, 3);
        assert_eq!(b.tail_len, 99);
        assert_eq!(&b.location[..3], &[1, 2, 3]);
        assert_eq!(&b.coeff[..3], &[0.1, 0.2, 0.3]);
        assert_eq!(&b.filter_idx[..3], &[0, 1, 2]);
        assert_eq!(&b.location_r[..3], &[1, 4, 9]);
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let p = test_params();
        let (d, w, t) = (default_decay_curve(), default_width_curve(), default_tone_curve());
        let mut a = VelvetSequence::new();
        let mut b = VelvetSequence::new();
        generate(&mut a, &p, &d, &w, &t);
        generate(&mut b, &p, &d, &w, &t);
        assert_eq!(a.count, b.count);
        assert_eq!(&a.location[..a.count], &b.location[..b.count]);
        assert_eq!(&a.coeff[..a.count], &b.coeff[..b.count]);
        assert_eq!(&a.location_r[..a.count], &b.location_r[..b.count]);
    }

    #[test]
    fn pulse_count_tracks_size_times_density() {
        let mut p = test_params();
        p.size_s = 2.0;
        p.density = 1000.0;
        let (d, w, t) = (flat(1.0), flat(0.0), flat(0.5));
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &d, &w, &t);
        // ~ size*density = 2000, within grid rounding.
        assert!((s.count as i32 - 2000).abs() < 5, "count {}", s.count);
    }

    #[test]
    fn coeffs_are_energy_normalized() {
        let p = test_params();
        let (d, w, t) = (default_decay_curve(), flat(0.0), flat(0.5));
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &d, &w, &t);
        let e: f64 = s.coeff[..s.count].iter().map(|&c| (c as f64).powi(2)).sum();
        assert!((e - 1.0).abs() < 1e-3, "energy {e}");
    }

    #[test]
    fn decay_curve_shapes_the_energy_envelope() {
        // A gated decay (full for first half, silent second half) must leave
        // the second-half pulses at zero gain.
        let p = test_params();
        let mut decay = MsegData::default();
        decay.insert_node(0.5, 0.0);
        // node[0] stepped=true holds value 1.0 up to node[1] at 0.5; then
        // node[1] has value 0.0, so from 0.5 onward the curve returns 0.0.
        decay.nodes[0] = MsegNode { time: 0.0, value: 1.0, tension: 0.0, stepped: true };
        let mid = decay.nodes[1];
        decay.nodes[1] = MsegNode { stepped: false, ..mid };
        decay.nodes[2] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
        decay.debug_assert_valid();

        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &decay, &flat(0.0), &flat(0.5));
        let l = (p.size_s * p.sample_rate) as f32;
        for m in 0..s.count {
            let phase = s.location[m] as f32 / l;
            if phase > 0.6 {
                assert!(s.coeff[m].abs() < 1e-6, "pulse past gate should be silent");
            }
        }
    }

    #[test]
    fn width_zero_makes_left_and_right_identical() {
        let p = test_params();
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &default_decay_curve(), &flat(0.0), &flat(0.5));
        assert_eq!(&s.location[..s.count], &s.location_r[..s.count], "width 0 = mono");
    }

    #[test]
    fn higher_width_decorrelates_more() {
        // Mean |k_R - k_L| must grow with the Width curve level.
        let p = test_params();
        let mean_offset = |wv: f32| {
            let mut s = VelvetSequence::new();
            generate(&mut s, &p, &default_decay_curve(), &flat(wv), &flat(0.5));
            let sum: i64 = (0..s.count)
                .map(|m| (s.location_r[m] as i64 - s.location[m] as i64).abs())
                .sum();
            sum as f64 / s.count.max(1) as f64
        };
        assert!(mean_offset(0.8) > mean_offset(0.2), "more width → more jitter");
    }

    #[test]
    fn tone_curve_selects_brighter_filters_when_higher() {
        let p = test_params();
        let mean_idx = |tv: f32| {
            let mut s = VelvetSequence::new();
            generate(&mut s, &p, &default_decay_curve(), &flat(0.0), &flat(tv));
            s.filter_idx[..s.count].iter().map(|&i| i as f64).sum::<f64>() / s.count.max(1) as f64
        };
        assert!(mean_idx(0.9) > mean_idx(0.1), "brighter tone → higher filter index");
    }

    #[test]
    fn curve_value_matches_value_at_phase() {
        let mut d = MsegData::default();
        d.insert_node(0.3, 0.8);
        d.insert_node(0.6, 0.2);
        let mut seg = 0;
        for i in 0..=100 {
            let phase = i as f32 / 100.0;
            let got = curve_value(&d, phase, &mut seg);
            let want = value_at_phase(&d, phase);
            assert!((got - want).abs() < 1e-5, "phase {phase}: {got} vs {want}");
        }
    }
}
