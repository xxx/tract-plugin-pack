# Multosis Phase 2, Milestone 2b — Modulation Engine — Design

**Date:** 2026-05-19
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Milestone 2a gave Multosis a per-track effect abstraction with a parameter seam: every effect declares its modulatable `parameters()` and accepts values via `set_param`, and the audio engine applies a per-row amplitude gain (currently a literal `1.0`). Milestone 2b builds the modulation engine that drives that seam — **three MSEGs per track effect**: one amplitude MSEG plus two assignable MSEGs, each running on its own clock, decoupled from the step grid (the Phase 1 §1.1 "envelopes sweep underneath" model).

2b builds the modulation **engine** only. The MSEG-editing UI and the effect/assignment UI are Milestone 2c; envelope retriggering is Phase 3. As 2a left a seam for 2b, 2b leaves the MSEG data fully editable for 2c without writing any UI.

## Reference

- Phase 1 design `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — §1.1 (envelopes run on their own Time/Beat clock, decoupled from the grid; the grid only gates), §1.2 (Phase 2 phasing), §1.3 / Phase 3 (envelope-retrigger variety is deferred).
- Phase 2a design `docs/superpowers/specs/2026-05-18-multosis-phase-2a-design.md` §6 — the seam: `parameters()`/`set_param`, the per-row amplitude gain.
- Existing MSEG: `tiny-skia-widgets/src/mseg/` — `MsegData` (`Copy`, 128-node, `sync_mode: SyncMode` Time/Beat, `time_seconds`/`beats`, `play_mode`, `hold`, custom serde); `mseg::value_at_phase(&MsegData, phase) -> f32` (pure, 0..1); `mseg::advance(&MsegData, phase, dt, released) -> (phase, finished)` (pure phase advance honouring the document's playback rules). Used live by `miff`.
- Current code: `multosis/src/effects.rs` (`Effect`, `ParamSpec`, `EffectKind`, `EffectInstance`, `TrackEffect { kind, params: [f32; MAX_EFFECT_PARAMS] }`); `multosis/src/engine.rs` (`AudioEngine`, the per-segment / per-sample `process` loop, `process_sample`, the amplitude seam comment); `multosis/src/lib.rs` (`MultosisParams` `#[persist]` fields, `process()` — host BPM via `context.transport().tempo`).

## §1 The MSEG type — reused

2b reuses `tiny-skia-widgets`'s `MsegData` unchanged. It is already `Copy`, heap-free, serde-serializable, audio-thread-safe, and has the pure evaluation/advance functions 2b needs. **2b writes no MSEG type.**

## §2 Per-track modulation config — new persisted state

A new struct, in a new `multosis/src/modulation.rs` module:

```
struct TrackModulation {
    /// msegs[0] = amplitude MSEG; msegs[1], msegs[2] = the two assignable MSEGs.
    msegs: [MsegData; 3],
    /// For each assignable MSEG, the target effect-parameter index, or None.
    targets: [Option<usize>; 2],
    /// For each assignable MSEG, a bipolar modulation depth (−1..1).
    depths: [f32; 2],
}
```

`targets[k]` / `depths[k]` belong to `msegs[k + 1]` (the amplitude MSEG `msegs[0]` has no target or depth — see §3).

The plugin gains a persisted field on `MultosisParams`: `#[persist = "track-modulation"] Arc<Mutex<[TrackModulation; 16]>>` — one `TrackModulation` per track row, separate from 2a's `track_effects`. A project saved before 2b lacks the `track-modulation` key, so nih-plug's `#[persist]` leaves the field at its `Default` — no migration code is needed. `MsegData` has custom serde; `[MsegData; 3]`, `[Option<usize>; 2]`, `[f32; 2]`, and `[TrackModulation; 16]` are all small arrays (≤ 32) that derive serde directly.

## §3 The modulation math

**Assignable MSEGs (`msegs[1]`, `msegs[2]`).** When `targets[k]` is `Some(i)`, MSEG `k+1` modulates effect parameter `i` *around its base value*:

```
mseg_value = value_at_phase(&msegs[k+1], phase)   // 0..1
bipolar    = mseg_value * 2.0 - 1.0               // −1..1, midline 0.5 → 0
spec       = effect.parameters()[i]               // ParamSpec, gives min/max
effective  = base + bipolar * depths[k] * (spec.max - spec.min)
effective  = effective.clamp(spec.min, spec.max)
effect.set_param(i, effective)
```

`base` is `track_effects[row].params[i]` — the 2a per-track base value (the knob the 2c editor exposes). The MSEG midline (0.5) leaves the parameter at its base; `depths[k]` scales the swing; a negative depth inverts the curve. When `targets[k]` is `None`, MSEG `k+1` is not evaluated and parameter `i` keeps its static base.

**Amplitude MSEG (`msegs[0]`).** Its value *is* the row's per-block amplitude gain — `amplitude[row] = value_at_phase(&msegs[0], phase)` (0..1). A flat-1.0 amplitude MSEG means no level change. There is no separate base or depth for amplitude (the gain's natural base is 1.0 and the MSEG simply is the gain curve). The engine multiplies each row's wet contribution by `amplitude[row]` (replacing the 2a literal `1.0`).

## §4 The clock — free-running

Each of the 48 MSEGs (16 rows × 3) has its own runtime phase. There is no shared modulation clock and no coupling to the step grid.

- **Length in samples** comes from the MSEG's own `sync_mode`: `SyncMode::Time` → `time_seconds · sample_rate`; `SyncMode::Beat` → `beats · (60 / bpm) · sample_rate` (host BPM is read in `process()`).
- Once per process block, each MSEG's phase advances by `block_len ÷ length_in_samples`, wrapped via `mseg::advance` (cyclic looping, honouring any loop region in the `MsegData`).
- All 48 MSEGs advance every block, whether or not their row is currently lit — they "sweep underneath", so a cell that lights picks the modulation up at its free-running position.
- Retriggering (resetting an MSEG's phase when a cell lights, alternative trigger sources) is **Phase 3** — 2b runs every MSEG free.

## §5 Engine integration

The modulation runtime (per-MSEG phases + the `[TrackModulation; 16]` data) lives in `modulation.rs` and is owned by `AudioEngine`, bridged in at init from the persisted `track-modulation` field (mirroring how 2a's `set_effects` bridges `track_effects`).

Once per `AudioEngine::process` block, **before** the per-segment/per-sample loop:

1. Advance all 48 MSEG phases by the block length (§4), using the block's host BPM and the engine's sample rate.
2. For each of the 16 rows: evaluate `msegs[0]` → store `amplitude[row]`; for each assigned assignable MSEG, evaluate it and apply the §3 result through `effect.set_param`.

The per-sample loop in `process_sample` then scales each active row's wet `(l, r)` by `amplitude[row]` before summing.

Modulation is applied **per block** (envelope/LFO rate) — cheap, standard, and avoids recomputing effect coefficients every sample; per-sample smoothing of fast MSEGs is a possible later refinement. All of it is allocation-free; no locks on the audio thread (the config is bridged at init, like `track_effects`).

`AudioEngine::process` gains a `bpm: f64` argument (the modulation clock needs it); `lib.rs`'s `process()` already has `bpm` and passes it.

## §6 Defaults

Before the 2c editor exists, each track's `TrackModulation` defaults so 2b plays with audible per-track movement (echoing 2a's varied default config):

- `msegs[0]` (amplitude) — flat at 1.0 (no level pumping; safe).
- `msegs[1]` — assigned (`targets[0] = Some(0)`) to the effect's first parameter, a gentle curve at a moderate depth, with the MSEG's Beat length spread by row so each row's filter/crusher drifts at a different rate.
- `msegs[2]` — unassigned (`targets[1] = None`, `depths[1] = 0`).

The exact default MSEG node shapes and the per-row length spread are a detail for the plan; the requirement is a modest, musical, per-row-varied default that audibly exercises the assignable-MSEG path.

## §7 Out of scope (2c / Phase 3)

- The MSEG-editor UI, the tabbed shell, per-track effect/modulation assignment UI, a live config handoff — Milestone 2c.
- Envelope retriggering, alternative trigger sources (MIDI, transient, free Hz), retrigger variety — Phase 3.
- Per-sample modulation smoothing; modulating the `mix` / `output_gain` / global parameters; modulation of the amplitude with a depth control — not in 2b.

## §8 Testing

Per CLAUDE.md — TDD, inline `#[cfg(test)]` modules, `cargo nextest`.

- **Modulation math** — the assignable mapping: MSEG midline leaves the parameter at base; full positive depth swings up to (clamped at) `max`; negative depth inverts; the result is always within `[min, max]`. The amplitude mapping: a flat-1.0 MSEG yields gain 1.0; a flat-0 MSEG yields gain 0.
- **The clock** — `Time` length and `Beat` length both convert to the expected sample count (a `Beat` MSEG at a known BPM/SR); the phase wraps cyclically; all phases advance independently.
- **Engine** — `AudioEngine::process` applies modulation per block: a row with an assignable MSEG assigned to a parameter produces output that changes over successive blocks; the amplitude MSEG scales a row's contribution; an unassigned MSEG leaves the base parameter untouched; modulation is allocation-free.
- **Persistence** — `TrackModulation` and `[TrackModulation; 16]` serde round-trip; a plugin state with no `track-modulation` key loads with the default modulation.
- The standalone bin plays the sequencer with the default per-track modulation (verified by ear).
