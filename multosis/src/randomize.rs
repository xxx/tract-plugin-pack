//! Deterministic randomization of cell activations and per-track effect
//! parameters.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §4.3.

use crate::effects::{norm_to_value, Effect, EffectInstance, EffectKind, ParamFormat, TrackEffect};
use crate::grid::Grid;
use crate::modulation::{switch_effect_kind, TrackModulation};

/// Deterministic xorshift32 PRNG — no dependency, seeded per call. Matches the
/// MSEG widget's `randomize` PRNG.
pub(crate) struct Rng(u32);

impl Rng {
    pub(crate) fn new(seed: u32) -> Self {
        // Mix the seed by multiplying by the golden-ratio prime so its bits
        // are spread across the whole state before xorshift32 consumes it.
        //
        // Without this, small consecutive integer seeds (1, 2, 3, ...) all
        // produce tiny first outputs -- the state simply hasn't had enough
        // iterations to diffuse a low-bit-weight value. That bug surfaced
        // in the effect randomizer: each click of "Randomize" used a fresh
        // counter-bumped seed, but the first parameter (Center / Threshold
        // on common effects) was always stuck at its minimum because its
        // norm came from that tiny first output, and the second parameter
        // (Feedback / Ratio) crept up by a constant per click because
        // consecutive seeds gave linearly-spaced second outputs.
        //
        // 0x9E37_79B9 is `floor(2^32 / phi)` -- the standard "mix bits"
        // constant. Any large odd constant works; the golden ratio is the
        // canonical choice because its bit pattern correlates poorly with
        // common seeds. xorshift cannot leave the all-zero state, so we
        // still map seed 0 to a fixed non-zero constant.
        let mixed = if seed == 0 {
            0x9E37_79B9
        } else {
            seed.wrapping_mul(0x9E37_79B9)
        };
        Rng(mixed)
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    pub(crate) fn bool(&mut self) -> bool {
        self.next_u32() & 1 == 1
    }

    /// Uniform `[0, 1)` f32. Uses the top 24 bits of `next_u32` so the result
    /// hits every representable mantissa in that range without bias.
    pub(crate) fn next_f32_01(&mut self) -> f32 {
        // f32 has a 24-bit mantissa; shifting right 8 keeps the top 24 bits
        // and divides by 2^24 below for an exact unit interval.
        let bits = self.next_u32() >> 8;
        bits as f32 * (1.0 / (1u32 << 24) as f32)
    }
}

/// Randomize a track's effect in place. The two cases from the spec:
///
/// 1. **Effect is None.** Pick a random non-None kind from
///    [`EffectKind::ALL`] that is **not** present in `in_use_elsewhere`
///    (the kinds currently assigned to OTHER tracks), assign it via
///    [`switch_effect_kind`] so params reset to defaults and modulation
///    targets get clamped to the new arity, then continue to step 2.
///    If every non-None kind is already in use elsewhere, leaves the
///    track on None and skips parameter randomization -- there's nothing
///    sensible to randomize.
///
/// 2. **Effect is already assigned.** Keep the kind exactly as-is and
///    randomize each parameter's value from a uniform `[0, 1)` sample
///    converted through the spec's `min`/`max`/`scaling` (so Log-scaled
///    params land log-uniformly across the range). Enum-format params
///    round to a valid integer index.
///
/// **`mix` is never touched** -- the spec calls that out explicitly so
/// dialed-in dry/wet balances survive a re-roll.
///
/// Deterministic in `seed`: same `(effect, modulation, in_use_elsewhere,
/// seed)` always produces the same result.
pub fn randomize_track_effect(
    effect: &mut TrackEffect,
    modulation: &mut TrackModulation,
    in_use_elsewhere: &[EffectKind],
    seed: u32,
) {
    let mut rng = Rng::new(seed);

    // Case 1: None on the current track -- choose a random unused kind.
    if effect.kind == EffectKind::None {
        let mut candidates: Vec<EffectKind> = EffectKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != EffectKind::None && !in_use_elsewhere.contains(k))
            .collect();
        if candidates.is_empty() {
            // Every non-None kind is taken on another track. The spec is
            // about picking an UNUSED one, so we don't shoehorn in a
            // duplicate -- leave the track on None.
            return;
        }
        let idx = (rng.next_u32() as usize) % candidates.len();
        let chosen = candidates.swap_remove(idx);
        switch_effect_kind(effect, modulation, chosen);
    }

    // Case 2 (and the tail of case 1): randomize each param value within
    // its spec range. `default_params_for_kind` already ran inside
    // `switch_effect_kind`, but we overwrite each used slot here.
    let instance = EffectInstance::new(effect.kind);
    let specs = instance.parameters();
    for (i, spec) in specs.iter().enumerate() {
        let norm = rng.next_f32_01();
        let value = match spec.format {
            ParamFormat::Enum { labels } => {
                // Discrete index: scale uniform [0, 1) by the label count
                // and floor to land on a valid index. The .min clamp
                // protects against the degenerate norm = 1.0 case (which
                // next_f32_01 can't actually produce, but cheap insurance).
                let n = labels.len().max(1) as f32;
                (norm * n).floor().min(n - 1.0)
            }
            _ => norm_to_value(norm, spec.min, spec.max, spec.scaling),
        };
        effect.params[i] = value;
    }
    // Slots past the kind's parameter count are not touched -- they're
    // unused for this kind and remain at their default (zero) state from
    // `switch_effect_kind`'s `default_params_for_kind` call.
}

/// Randomize `enabled` for every cell inside the loop region. Deterministic in
/// `seed`. Leaves cells outside the region untouched.
pub fn randomize_activations(grid: &mut Grid, seed: u32) {
    let mut rng = Rng::new(seed);
    let lr = grid.loop_region;
    for r in lr.row0..=lr.row1 {
        for c in lr.col0..=lr.col1 {
            grid.cell_mut(r, c).enabled = rng.bool();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{default_params_for_kind, MAX_EFFECT_PARAMS};
    use crate::grid::LoopRegion;

    fn fresh_track_effect(kind: EffectKind) -> TrackEffect {
        TrackEffect {
            kind,
            params: default_params_for_kind(kind),
            mix: 0.42, // Distinctive so tests can assert it survives.
            muted: false,
            soloed: false,
        }
    }

    fn fresh_modulation() -> TrackModulation {
        TrackModulation::default()
    }

    #[test]
    fn randomize_track_effect_assigns_a_new_kind_when_current_is_none() {
        let mut effect = fresh_track_effect(EffectKind::None);
        let mut modu = fresh_modulation();
        randomize_track_effect(&mut effect, &mut modu, &[], 1234);
        assert_ne!(effect.kind, EffectKind::None, "should pick a non-None kind");
    }

    #[test]
    fn randomize_track_effect_skips_kinds_in_use_elsewhere() {
        // Make every kind except WarpZone "in use". The randomizer must
        // pick WarpZone (the only available one).
        let mut in_use: Vec<EffectKind> = EffectKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != EffectKind::None && *k != EffectKind::WarpZone)
            .collect();
        in_use.push(EffectKind::None); // No-op in `contains` but ensures None coverage.
        let mut effect = fresh_track_effect(EffectKind::None);
        let mut modu = fresh_modulation();
        randomize_track_effect(&mut effect, &mut modu, &in_use, 1);
        assert_eq!(effect.kind, EffectKind::WarpZone);
    }

    #[test]
    fn randomize_track_effect_stays_none_when_all_kinds_in_use() {
        // Every non-None kind already in use elsewhere -> the only kind
        // the randomizer would be "allowed" to pick is None itself,
        // which is excluded -> keep current (None).
        let in_use: Vec<EffectKind> = EffectKind::ALL
            .iter()
            .copied()
            .filter(|k| *k != EffectKind::None)
            .collect();
        let mut effect = fresh_track_effect(EffectKind::None);
        let mut modu = fresh_modulation();
        randomize_track_effect(&mut effect, &mut modu, &in_use, 42);
        assert_eq!(effect.kind, EffectKind::None);
        // Params untouched too (all zero from default).
        assert_eq!(effect.params, [0.0; MAX_EFFECT_PARAMS]);
    }

    #[test]
    fn randomize_track_effect_preserves_kind_when_already_set() {
        // Pre-condition: already assigned to Comb. After randomizing,
        // the kind must still be Comb.
        let mut effect = fresh_track_effect(EffectKind::Comb);
        let mut modu = fresh_modulation();
        randomize_track_effect(&mut effect, &mut modu, &[EffectKind::Satch], 7);
        assert_eq!(effect.kind, EffectKind::Comb);
    }

    #[test]
    fn randomize_track_effect_preserves_mix_in_both_branches() {
        // None -> assigned: Mix must survive the kind switch.
        let mut e_none = fresh_track_effect(EffectKind::None);
        let mut m_none = fresh_modulation();
        randomize_track_effect(&mut e_none, &mut m_none, &[], 11);
        assert!(
            (e_none.mix - 0.42).abs() < 1e-6,
            "Mix must survive None->kind path; got {}",
            e_none.mix
        );
        // Already-assigned path: Mix must survive the param re-roll.
        let mut e_set = fresh_track_effect(EffectKind::Distortion);
        let mut m_set = fresh_modulation();
        randomize_track_effect(&mut e_set, &mut m_set, &[], 11);
        assert!(
            (e_set.mix - 0.42).abs() < 1e-6,
            "Mix must survive already-assigned path; got {}",
            e_set.mix
        );
    }

    #[test]
    fn randomize_track_effect_lands_params_inside_their_spec_ranges() {
        // For every non-None kind, randomize the params and verify each
        // landed inside its declared [min, max] range. Catches any
        // miscompute in `norm_to_value` or the Enum index path.
        for &kind in EffectKind::ALL.iter().filter(|&&k| k != EffectKind::None) {
            for seed in [1u32, 2, 7, 99, 12_345] {
                let mut effect = fresh_track_effect(kind);
                let mut modu = fresh_modulation();
                randomize_track_effect(&mut effect, &mut modu, &[], seed);
                let instance = EffectInstance::new(kind);
                let specs = instance.parameters();
                for (i, spec) in specs.iter().enumerate() {
                    let v = effect.params[i];
                    assert!(
                        v >= spec.min - 1e-5 && v <= spec.max + 1e-5,
                        "{:?} param {} (seed {}) out of range: {} not in [{}, {}]",
                        kind,
                        i,
                        seed,
                        v,
                        spec.min,
                        spec.max
                    );
                }
            }
        }
    }

    #[test]
    fn randomize_track_effect_is_deterministic_in_seed() {
        let make = || {
            let mut e = fresh_track_effect(EffectKind::None);
            let mut m = fresh_modulation();
            randomize_track_effect(&mut e, &mut m, &[], 9999);
            (e, m)
        };
        let a = make();
        let b = make();
        assert_eq!(a.0.kind, b.0.kind);
        assert_eq!(a.0.params, b.0.params);
    }

    #[test]
    fn randomize_track_effect_differs_by_seed() {
        // With at least 4 params, two different seeds should produce
        // different value arrays (very high probability). FmEffect has
        // 5 params -> plenty of bits.
        let mut e1 = fresh_track_effect(EffectKind::Fm);
        let mut m1 = fresh_modulation();
        randomize_track_effect(&mut e1, &mut m1, &[], 1);
        let mut e2 = fresh_track_effect(EffectKind::Fm);
        let mut m2 = fresh_modulation();
        randomize_track_effect(&mut e2, &mut m2, &[], 2);
        assert_ne!(e1.params, e2.params);
    }

    #[test]
    fn randomize_track_effect_param_zero_spreads_across_consecutive_seeds() {
        // Regression test for the "first param stuck at min" bug: with
        // a plain xorshift32 seeded by small consecutive integers
        // (1, 2, 3, ...), the first next_u32 output is always a tiny
        // value, so the first parameter (often a Log-scaled Hz or a
        // dB-range Linear) ends up clustered at its minimum across
        // clicks. The fix in `Rng::new` multiplies the seed by the
        // golden-ratio prime so the bits are mixed before xorshift
        // consumes them.
        //
        // Probe with Phaser, whose Center is a Log-scaled 50..8000 Hz
        // -- the WORST case for this bug because Log mapping crowds
        // near-zero norms toward the min.
        let mut centers = Vec::new();
        for seed in 1..=16u32 {
            let mut e = fresh_track_effect(EffectKind::Phaser);
            let mut m = fresh_modulation();
            randomize_track_effect(&mut e, &mut m, &[], seed);
            centers.push(e.params[0]);
        }
        let min_center = centers.iter().cloned().fold(f32::INFINITY, f32::min);
        let max_center = centers.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        // 16 seeds should span at least a 10x range across the
        // 50..8000 Hz Log space if the seed is properly mixed.
        // Pre-fix this ratio was ~1.001 (every value stuck at ~50 Hz).
        assert!(
            max_center / min_center > 10.0,
            "param[0] across 16 consecutive seeds spans only {} .. {} Hz \
             -- seed mixing regression?",
            min_center,
            max_center,
        );
    }

    #[test]
    fn randomize_track_effect_param_one_spreads_across_consecutive_seeds() {
        // Companion regression test for the "second param +1% per
        // click" bug from the same xorshift32 small-seed issue: the
        // 2nd next_u32 output increased linearly with seed, so
        // Linear-scaled second params crept up by a constant per click.
        // Probe Phaser's Feedback (0..95 % Linear).
        let mut fbs = Vec::new();
        for seed in 1..=16u32 {
            let mut e = fresh_track_effect(EffectKind::Phaser);
            let mut m = fresh_modulation();
            randomize_track_effect(&mut e, &mut m, &[], seed);
            fbs.push(e.params[1]);
        }
        // The standard deviation across 16 well-distributed samples
        // of a uniform [0, 95] should be ~27. Pre-fix the values were
        // monotonically increasing by ~1.5 each so stddev was ~7.
        // Demand at least 15 (well above the pre-fix value, well below
        // the ideal).
        let mean: f32 = fbs.iter().sum::<f32>() / fbs.len() as f32;
        let var: f32 = fbs.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / fbs.len() as f32;
        let stddev = var.sqrt();
        assert!(
            stddev > 15.0,
            "param[1] stddev across 16 consecutive seeds is only {} \
             -- expected ~27 for well-distributed samples in [0, 95]",
            stddev,
        );
    }

    #[test]
    fn randomize_track_effect_enum_params_are_integer_indices() {
        // Use SVF which has Enum-format Poles/Type params. After
        // randomize, those slots must be integers in [0, labels-1].
        let mut effect = fresh_track_effect(EffectKind::Svf);
        let mut modu = fresh_modulation();
        randomize_track_effect(&mut effect, &mut modu, &[], 314);
        let specs = EffectInstance::new(EffectKind::Svf).parameters();
        for (i, spec) in specs.iter().enumerate() {
            if let ParamFormat::Enum { labels } = spec.format {
                let v = effect.params[i];
                assert_eq!(
                    v.fract(),
                    0.0,
                    "Enum slot {} should be integer-valued, got {}",
                    i,
                    v
                );
                assert!(
                    (0.0..labels.len() as f32).contains(&v),
                    "Enum slot {} value {} out of [0, {}) range",
                    i,
                    v,
                    labels.len()
                );
            }
        }
    }

    #[test]
    fn randomize_activations_is_deterministic() {
        let mut a = Grid::default();
        let mut b = Grid::default();
        randomize_activations(&mut a, 4242);
        randomize_activations(&mut b, 4242);
        assert_eq!(a, b);
    }

    #[test]
    fn randomize_activations_differs_by_seed() {
        let mut a = Grid::default();
        let mut b = Grid::default();
        randomize_activations(&mut a, 1);
        randomize_activations(&mut b, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn randomize_activations_only_touches_cells_inside_the_region() {
        let mut g = Grid::default();
        g.loop_region = LoopRegion {
            row0: 4,
            row1: 6,
            col0: 10,
            col1: 14,
        };
        randomize_activations(&mut g, 99);
        for r in 0..crate::grid::ROWS {
            for c in 0..crate::grid::COLS {
                if !g.loop_region.contains(r, c) {
                    assert!(
                        g.cell(r, c).enabled,
                        "cell ({r},{c}) outside region changed"
                    );
                }
            }
        }
    }
}
