//! Fixed-capacity rhythmic patterns and editor operations.

use serde::{Deserialize, Serialize};

pub const MAX_STEPS: usize = 64;
pub const STORAGE_STEPS: usize = 128;
pub const SLOT_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Pattern {
    pub length: u16,
    pub active: [u64; 2],
    pub ties: [u64; 2],
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            length: 16,
            active: [0xffff, 0],
            ties: [0, 0],
        }
    }
}

fn bit(words: &[u64; 2], index: usize) -> bool {
    index < STORAGE_STEPS && words[index / 64] & (1 << (index % 64)) != 0
}

fn set_bit(words: &mut [u64; 2], index: usize, enabled: bool) {
    if index >= STORAGE_STEPS {
        return;
    }
    let mask = 1 << (index % 64);
    if enabled {
        words[index / 64] |= mask;
    } else {
        words[index / 64] &= !mask;
    }
}

impl Pattern {
    pub fn len(&self) -> usize {
        usize::from(self.length).clamp(1, MAX_STEPS)
    }

    pub fn is_empty(&self) -> bool {
        (0..self.len()).all(|i| !self.is_on(i))
    }

    pub fn is_on(&self, index: usize) -> bool {
        index < self.len() && bit(&self.active, index)
    }

    pub fn tied(&self, index: usize) -> bool {
        self.is_on(index) && self.is_on((index + 1) % self.len()) && bit(&self.ties, index)
    }

    pub fn set_length(&mut self, length: usize) {
        self.length = length.clamp(1, MAX_STEPS) as u16;
    }

    pub fn set_on(&mut self, index: usize, enabled: bool) {
        if index >= self.len() {
            return;
        }
        set_bit(&mut self.active, index, enabled);
        if !enabled {
            let previous = (index + self.len() - 1) % self.len();
            set_bit(&mut self.ties, index, false);
            set_bit(&mut self.ties, previous, false);
        }
    }

    pub fn set_tie(&mut self, index: usize, enabled: bool) {
        if index < self.len() {
            let valid = enabled && self.is_on(index) && self.is_on((index + 1) % self.len());
            set_bit(&mut self.ties, index, valid);
        }
    }

    pub fn rotate(&mut self, right: bool) {
        let before = *self;
        let len = self.len();
        for i in 0..len {
            let source = if right {
                (i + len - 1) % len
            } else {
                (i + 1) % len
            };
            set_bit(&mut self.active, i, before.is_on(source));
            set_bit(&mut self.ties, i, before.tied(source));
        }
    }

    pub fn randomize(&mut self, rng: &mut Random, density: f32, tie_probability: f32) {
        for i in 0..self.len() {
            set_bit(&mut self.active, i, rng.unit() < density.clamp(0.0, 1.0));
        }
        for i in 0..self.len() {
            self.set_tie(i, rng.unit() < tie_probability.clamp(0.0, 1.0));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PatternBank {
    pub patterns: [Pattern; SLOT_COUNT],
}

impl Default for PatternBank {
    fn default() -> Self {
        Self {
            patterns: [Pattern::default(); SLOT_COUNT],
        }
    }
}

/// Seedable SplitMix64 generator used exclusively by the editor.
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        (((z ^ (z >> 31)) >> 40) as f32) / 16_777_216.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_is_bounded_and_hidden_steps_survive() {
        let mut p = Pattern::default();
        p.set_length(64);
        p.set_on(63, true);
        p.set_tie(63, true);
        p.set_length(3);
        assert!(!p.is_on(63));
        p.set_length(128);
        assert_eq!(p.len(), 64);
        assert!(p.is_on(63));
        assert!(p.tied(63));
        p.set_length(0);
        assert_eq!(p.len(), 1);
    }

    #[test]
    fn turning_off_a_step_clears_incoming_and_outgoing_ties() {
        let mut p = Pattern::default();
        p.set_tie(15, true);
        p.set_tie(0, true);
        p.set_on(0, false);
        p.set_on(0, true);
        assert!(!p.tied(15));
        assert!(!p.tied(0));
    }

    #[test]
    fn rotation_preserves_wrap_ties_and_hidden_data() {
        let mut p = Pattern::default();
        p.set_on(2, false);
        p.set_tie(15, true);
        p.active[1] = 0x1234;
        let before = p;
        p.rotate(true);
        assert!(p.tied(0));
        assert!(!p.is_on(3));
        p.rotate(false);
        assert_eq!(p, before);
    }

    #[test]
    fn randomization_endpoints_and_seed_are_reproducible() {
        let mut a = Pattern::default();
        a.randomize(&mut Random::new(42), 0.0, 1.0);
        assert!(a.is_empty());
        assert_eq!(a.ties[0], 0);
        a.randomize(&mut Random::new(42), 1.0, 1.0);
        assert_eq!(a.active[0], 0xffff);
        assert_eq!(a.ties[0], 0xffff);
        let mut b = a;
        a.randomize(&mut Random::new(123), 0.5, 0.25);
        b.randomize(&mut Random::new(123), 0.5, 0.25);
        assert_eq!(a, b);
    }

    #[test]
    fn persistence_keeps_future_capacity_and_handles_invalid_lengths() {
        let mut bank = PatternBank::default();
        bank.patterns[7].active[1] = u64::MAX;
        bank.patterns[0].length = 0;
        let json = serde_json::to_string(&bank).unwrap();
        let restored: PatternBank = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, bank);
        assert_eq!(restored.patterns[0].len(), 1);
        assert_eq!(
            serde_json::from_str::<PatternBank>("{}").unwrap(),
            PatternBank::default()
        );
    }
}
