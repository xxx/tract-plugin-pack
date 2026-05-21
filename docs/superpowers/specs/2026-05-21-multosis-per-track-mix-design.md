# Multosis Per-Track Mix — Design

**Date:** 2026-05-21
**Status:** Approved

## Summary

Add a per-track dry/wet **Mix** control to multosis. Each track's effect lane
gets its own wet/dry blend — the standard mix knob found on most effect
plugins — applied before the per-track amplitude MSEG and the wet-bus sum.
The existing global **Mix** knob is kept as a master dry/wet over the summed
result.

## Motivation

Today a track's effect lane is all-or-nothing: when the playhead activates a
track, its effect output goes onto the wet bus at full strength (scaled only
by the amplitude MSEG). There is no way to dial in a partially-wet effect on
one track while leaving another fully wet. The single global Mix blends dry
against the *whole* summed wet bus, so it cannot do this either. A per-track
Mix knob gives each track an independent dry/wet, the way a normal effect
plugin's mix knob works.

## Signal flow

Per-track mix `mix_r` blends track `r`'s lane between the dry input and that
track's effect output, *before* the amplitude MSEG and the wet-bus sum:

```
lane_r = (1 − mix_r)·dry + mix_r·effect_r(dry)
wet    = Σ_active  amp_r · lane_r
out    = dry + global_mix·(compress(wet) − dry)
```

- `effect_r(dry)` — track `r`'s effect output for the dry input sample.
- `amp_r` — track `r`'s amplitude-MSEG gain (unchanged).
- `compress` — the wet-bus soft-knee compressor (unchanged).
- `global_mix` — the existing master Mix parameter (unchanged).

Properties:

- **`mix_r = 1.0`** — `lane_r = effect_r(dry)`, byte-identical to today's
  behaviour. This is the default, so the change is sound-transparent until a
  Mix knob is moved.
- **`mix_r = 0.0`** — `lane_r = dry`; an *active* track contributes the dry
  signal to the wet bus (a "dry hit").
- **Gaps unchanged** — an inactive track contributes nothing to the sum, so
  with no track active `wet = 0` and `out = dry·(1 − global_mix)` — silence at
  `global_mix = 1`, exactly as today. The plugin keeps its rhythmic-gate
  character; per-track Mix only blends dry↔effect *within* each active hit.

Per-track Mix is a static knob, not an MSEG modulation target.

## Design

### 1. Data model (`effects.rs`)

`TrackEffect` gains a `mix: f32` field (range `0.0..=1.0`, linear):

```rust
pub struct TrackEffect {
    pub kind: EffectKind,
    pub params: [f32; MAX_EFFECT_PARAMS],
    /// Per-track dry/wet blend, 0.0 (dry) .. 1.0 (full effect).
    pub mix: f32,
}
```

- `TrackEffect::default_for_row` sets `mix: 1.0` — a freshly-assigned effect
  is fully wet.
- `TrackEffect` derives serde. The `mix` field is annotated
  `#[serde(default = "default_track_mix")]` where `default_track_mix() -> f32`
  returns `1.0`. A preset saved before this field loads with `mix = 1.0`,
  identical to its current sound.
- Per-track Mix is persisted GUI-edited config (it lives in the already-
  persisted `[TrackEffect; 16]` state), not a nih-plug automatable parameter —
  consistent with how effect parameters, the grid, and modulation are stored.

### 2. Engine (`engine.rs`)

`AudioEngine` already stores `track_effects: [TrackEffect; ROWS]` (bridged by
`set_effects`). `process_sample` reads `self.track_effects[r].mix` for each
active row and blends the lane:

```rust
fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16) -> (f32, f32) {
    let mut wet_l = 0.0;
    let mut wet_r = 0.0;
    for r in 0..ROWS {
        if active & (1 << r) == 0 {
            continue;
        }
        let (eff_l, eff_r) = self.effects[r].process_sample(dry_l, dry_r);
        let mix = self.track_effects[r].mix;
        // Per-track dry/wet blend, before the amplitude MSEG.
        let lane_l = dry_l + (eff_l - dry_l) * mix;
        let lane_r = dry_r + (eff_r - dry_r) * mix;
        let amp = self.modulation.amplitude(r);
        wet_l += amp * lane_l;
        wet_r += amp * lane_r;
    }
    (wet_l, wet_r)
}
```

One lerp per channel per active row — allocation-free, trivial cost. No change
to the segment loop, the compressor, or the global-mix application.

### 3. Editor UI (`editor/effect_editor.rs`, `editor.rs`)

A **Mix dial** in the effect editor's EFFECT section:

- **Placement** — a fixed slot at the right side of the EFFECT section,
  visually set apart from the effect-parameter dials (it is a track property,
  not an effect parameter, so it must not shift when the effect kind changes).
  A new `mix` rect is added to `EffectLayout`.
- **Widget** — the shared `param_dial` widget, value shown as a percentage
  (`0%`..`100%`).
- **Affordances** — the same ones the effect-parameter dials already have via
  the shared helpers: vertical drag to set; double-click to reset to `100%`;
  right-click text entry through the existing `TextEditState` path.
- **Hit-testing** — a new `EffectHit::Mix` variant; `effect_hit` returns it
  for the dial's rect; the press handler begins an `effect_dial_drag` (or a
  double-click reset, or a text edit) exactly as the param dials do.
- **Persistence** — editing the Mix dial writes `track_effects[row].mix` in
  the persisted `Mutex` and marks `config_dirty`; the audio thread re-bridges
  via `set_effects` on the next block, same as an effect-parameter edit.
- The Mix dial is drawn for every effect kind, including `EffectKind::None`
  (it is harmless there — a None track contributes nothing regardless).

### 4. Out of scope

- Per-track Mix is not an MSEG modulation target.
- No change to the global Mix, the compressor, the amplitude MSEG, the grid,
  or the sequencer.
- No new nih-plug parameters.

## Testing

- **Lane blend** — a unit test for the lerp: at `mix = 0` the lane equals dry;
  at `mix = 1` it equals the effect output; at `mix = 0.5` it is the midpoint.
- **Engine** — a test that an active track at `mix = 0.0` contributes the dry
  signal to the wet bus, and at `mix = 1.0` produces the same wet sum as
  before this change (a regression guard on sound-transparency at the
  default).
- **Default** — `TrackEffect::default_for_row` yields `mix == 1.0`.
- **Serde back-compat** — a `TrackEffect` JSON blob without a `mix` field
  deserializes with `mix == 1.0`; a round-trip with a non-default `mix`
  preserves it.
- **Editor** — `effect_hit` returns `EffectHit::Mix` for a point inside the
  Mix dial's rect; the layout rect is disjoint from the kind dropdown and the
  effect-parameter dials.

All work lands behind `cargo build`, `cargo clippy --workspace -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` clean.
