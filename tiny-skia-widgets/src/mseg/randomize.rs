//! MSEG randomizer — generates randomized envelopes in several styles.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

use crate::mseg::{HoldMode, MsegData, MsegNode, MAX_NODES};

/// Randomizer character. Each style biases node count, values, tension, and
/// stepping differently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RandomStyle {
    Smooth,
    Ramps,
    Stepped,
    Spiky,
    Chaos,
}

impl RandomStyle {
    /// All five variants in display order.
    pub const ALL: [RandomStyle; 5] = [
        RandomStyle::Smooth,
        RandomStyle::Ramps,
        RandomStyle::Stepped,
        RandomStyle::Spiky,
        RandomStyle::Chaos,
    ];

    /// Index of this variant in `ALL`.
    pub fn index(self) -> usize {
        match self {
            RandomStyle::Smooth => 0,
            RandomStyle::Ramps => 1,
            RandomStyle::Stepped => 2,
            RandomStyle::Spiky => 3,
            RandomStyle::Chaos => 4,
        }
    }

    /// Variant at position `i` in `ALL`. Clamps to the last element if out of
    /// range.
    pub fn from_index(i: usize) -> RandomStyle {
        RandomStyle::ALL[i.min(RandomStyle::ALL.len() - 1)]
    }
}

impl std::fmt::Display for RandomStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            RandomStyle::Smooth => "Smooth",
            RandomStyle::Ramps => "Ramps",
            RandomStyle::Stepped => "Stepped",
            RandomStyle::Spiky => "Spiky",
            RandomStyle::Chaos => "Chaos",
        };
        f.write_str(s)
    }
}

/// Deterministic xorshift32 PRNG — no dependency, seeded per `randomize` call.
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        // Avoid the all-zero state, which xorshift cannot leave.
        Rng(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Uniform f32 in 0..1.
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    /// Uniform f32 in `lo..hi`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }
    /// Uniform usize in `lo..=hi`.
    fn range_usize(&mut self, lo: usize, hi: usize) -> usize {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u32() as usize) % (hi - lo + 1)
    }
    fn bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }
}

/// Snap `v` to one of `steps` evenly-spaced levels in 0..1.
fn snap_value(v: f32, steps: u32) -> f32 {
    if steps == 0 {
        return v.clamp(0.0, 1.0);
    }
    let s = steps as f32;
    (v * s).round().clamp(0.0, s) / s
}

/// Regenerate `data.nodes` / `data.node_count` in the given `style`.
/// Deterministic given `seed`. Leaves `play_mode`, `sync_mode`, timing, and
/// grid settings untouched. Any `hold` left referencing an out-of-range node
/// is reset to `HoldMode::None`.
pub fn randomize(data: &mut MsegData, style: RandomStyle, seed: u32) {
    let mut rng = Rng::new(seed);

    // Node count. Stepped/Spiky fill the time grid (one node per cell, +1 for
    // the closing endpoint), capped at MAX_NODES. Smooth/Ramps are sparse.
    // Chaos picks freely.
    let grid_count = (data.time_divisions as usize + 1).clamp(2, MAX_NODES);
    let count = match style {
        RandomStyle::Stepped | RandomStyle::Spiky => grid_count,
        RandomStyle::Smooth | RandomStyle::Ramps => rng.range_usize(3, 6),
        RandomStyle::Chaos => rng.range_usize(3, 16.min(MAX_NODES)),
    };

    for i in 0..count {
        let time = if i == 0 {
            0.0
        } else if i == count - 1 {
            1.0
        } else {
            let even = i as f32 / (count - 1) as f32;
            if data.snap && data.time_divisions > 0 {
                let d = data.time_divisions as f32;
                ((even * d).round() / d).clamp(0.0, 1.0)
            } else {
                even
            }
        };

        let (mut value, tension, stepped) = match style {
            RandomStyle::Smooth => (rng.range(0.25, 0.85), rng.range(-0.6, 0.6), false),
            RandomStyle::Ramps => (rng.next_f32(), 0.0, false),
            RandomStyle::Stepped => (rng.next_f32(), 0.0, true),
            RandomStyle::Spiky => {
                let v = if i % 2 == 0 {
                    rng.range(0.0, 0.15)
                } else {
                    rng.range(0.85, 1.0)
                };
                (v, rng.range(-1.0, 1.0), rng.bool())
            }
            RandomStyle::Chaos => (rng.next_f32(), rng.range(-1.0, 1.0), rng.bool()),
        };

        if data.snap && matches!(style, RandomStyle::Stepped | RandomStyle::Spiky) {
            value = snap_value(value, data.value_steps);
        }

        data.nodes[i] = MsegNode {
            time,
            value,
            tension,
            stepped,
        };
    }
    data.node_count = count;

    // Snapping interior times can create collisions. Repair strict ascending
    // order by nudging any node that did not advance past its predecessor.
    // Work forward; then verify the closing endpoint is the strict maximum.
    for i in 1..count - 1 {
        let prev = data.nodes[i - 1].time;
        if data.nodes[i].time <= prev {
            data.nodes[i].time = (prev + 1e-3).min(1.0 - 1e-3);
        }
    }
    // The second-to-last interior node must be strictly less than the closing
    // endpoint (time == 1.0). Walk backward from count-2 and push down if needed.
    if count >= 3 {
        let second_last = count - 2;
        if data.nodes[second_last].time >= 1.0 {
            data.nodes[second_last].time = 1.0 - 1e-3;
        }
        // Propagate backward in case the nudge-forward pass piled nodes near 1.0.
        for i in (1..second_last).rev() {
            if data.nodes[i].time >= data.nodes[i + 1].time {
                data.nodes[i].time = (data.nodes[i + 1].time - 1e-3).max(0.0 + 1e-3);
            }
        }
    }

    // Invalidate a now-out-of-range hold.
    let hold_ok = match data.hold {
        HoldMode::None => true,
        HoldMode::Sustain(i) => i < count,
        HoldMode::Loop { start, end } => start < end && end < count,
    };
    if !hold_ok {
        data.hold = HoldMode::None;
    }

    data.debug_assert_valid();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::MsegData;

    #[test]
    fn randomize_is_deterministic() {
        let mut a = MsegData::default();
        let mut b = MsegData::default();
        randomize(&mut a, RandomStyle::Spiky, 12345);
        randomize(&mut b, RandomStyle::Spiky, 12345);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_different_seeds_differ() {
        let mut a = MsegData::default();
        let mut b = MsegData::default();
        randomize(&mut a, RandomStyle::Chaos, 1);
        randomize(&mut b, RandomStyle::Chaos, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_keeps_data_valid_for_every_style() {
        for style in [
            RandomStyle::Smooth,
            RandomStyle::Ramps,
            RandomStyle::Stepped,
            RandomStyle::Spiky,
            RandomStyle::Chaos,
        ] {
            for seed in 0..40u32 {
                let mut d = MsegData::default();
                randomize(&mut d, style, seed);
                assert!(d.is_valid(), "invalid for {style:?} seed {seed}");
                assert!(d.node_count >= 2 && d.node_count <= MAX_NODES);
            }
        }
    }

    #[test]
    fn stepped_style_makes_every_segment_stepped() {
        let mut d = MsegData::default();
        randomize(&mut d, RandomStyle::Stepped, 7);
        for i in 0..d.node_count - 1 {
            assert!(d.nodes[i].stepped, "segment {i} not stepped");
        }
    }

    #[test]
    fn smooth_style_has_no_stepped_segments() {
        let mut d = MsegData::default();
        randomize(&mut d, RandomStyle::Smooth, 7);
        for i in 0..d.node_count {
            assert!(!d.nodes[i].stepped);
        }
    }

    #[test]
    fn randomize_only_touches_shape() {
        let mut d = MsegData::default();
        d.time_seconds = 3.5;
        d.beats = 2.0;
        d.time_divisions = 12;
        d.value_steps = 5;
        d.play_mode = crate::mseg::PlayMode::Cyclic;
        randomize(&mut d, RandomStyle::Ramps, 9);
        assert_eq!(d.time_seconds, 3.5);
        assert_eq!(d.beats, 2.0);
        assert_eq!(d.time_divisions, 12);
        assert_eq!(d.value_steps, 5);
        assert_eq!(d.play_mode, crate::mseg::PlayMode::Cyclic);
    }

    #[test]
    fn randomize_clears_hold_when_count_changes() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5);
        d.hold = crate::mseg::HoldMode::Sustain(2);
        randomize(&mut d, RandomStyle::Chaos, 3);
        assert!(d.is_valid());
    }
}
