//! Velvet sequence generation (GUI thread). Turns the three drawn curves +
//! Size/Density/Width/Seed into a sparse signed pulse train with per-pulse
//! coloration routing and per-pulse right-channel jitter.

use tiny_skia_widgets::mseg::{MsegData, MsegNode, Polarity};

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
}
