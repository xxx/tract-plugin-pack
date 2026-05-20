# Multosis Phase 3 — Envelope Retriggering & Alternative Trigger Sources — Design

**Date:** 2026-05-19
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Phase 2b built the 3-MSEG modulation engine with every MSEG **free-running** on its own clock (Phase 1 §1.1: "envelopes sweep underneath"). Phase 2b §7 explicitly deferred "envelope retriggering, alternative trigger sources (MIDI, transient, free Hz), retrigger variety" to Phase 3. This milestone delivers the first cut of that work: a per-track **trigger source** that resets the row's MSEG phases on a chosen event, with two new sources beyond Free — **cell light** (the Multosis-native "this row just lit up under the wavefront" edge) and **Free Hz** (a rate-based retrigger at a user-set frequency). The enum is shaped so further sources (MIDI, transient, etc.) can land later without churning the data model.

## Reference

- Phase 1 design `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — §1.1 (envelopes run on their own Time/Beat clock, the grid only gates), §1.3 / Phase 3 (envelope-retrigger variety is deferred).
- Phase 2b design `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md` — §3 (modulation math), §4 (the free-running clock), §5 (engine integration), §7 (out of scope).
- Phase 2c design `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` — §3 (effect editor MODULATION section), §5 (the `config_dirty` live handoff).
- Current code:
  - `multosis/src/modulation.rs` — `TrackModulation { msegs: [MsegData; 3], targets: [Option<usize>; 2], depths: [f32; 2] }`; `Modulation { config, phases: [[f32; 3]; ROWS], amplitudes: [f32; ROWS] }`; `update_block(block_len, bpm, sample_rate, &mut effects, &track_effects)`.
  - `multosis/src/engine.rs` — `AudioEngine::process` already computes a per-segment `active: u16` row mask and retains the block's last value in `last_active` (`active_mask()`).
  - `multosis/src/editor/effect_editor.rs` — `EffectLayout` + `effect_hit` + `draw_modulation_controls`. The MODULATION section currently holds the 3-way MSEG selector (`mseg_selector` rect) and, for assignable MSEGs, the target dropdown + depth dial.
  - `multosis/src/editor.rs` — `MultosisWindow` holds the shared `DropdownState<EffectAction>` used by Kind and Target, the `mseg_edit: MsegEditState`, the `selected_track` / `selected_mseg`, and `config_dirty: Arc<AtomicBool>` with a `mark_config_dirty()` helper.

## §1 The `TriggerSource` enum

A new public type in `multosis/src/modulation.rs`:

```
enum TriggerSource {
    /// Free-running — phases advance every block, no resets (the Phase 2b behaviour).
    Free,
    /// Edge-triggered on a row's inactive→active transition in the engine's
    /// active-row mask.
    CellLight,
    /// Rate-triggered — fires every `1.0 / hz` seconds, independent of any
    /// `MsegData::sync_mode`. `hz` is positive; the engine treats hz ≤ 0 as
    /// "never fires" defensively.
    FreeHz { hz: f32 },
}
```

`#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]` so it persists alongside the rest of `TrackModulation`'s fields without custom serde. The variants stay open: future sources (MIDI, transient, …) add their own variant and an oscillator/edge-detect branch in the runtime; nothing else changes.

## §2 Per-track trigger state

`TrackModulation` gains a single field:

```
pub trigger: TriggerSource,
```

The trigger is **per track**, not per MSEG: when it fires, all three of that row's MSEG phases reset together (the design conversation chose this for UI simplicity — one dropdown per track rather than three). `default_for_row` initialises `trigger: TriggerSource::Free`, so 2b's behaviour is preserved exactly for existing default tracks. A pre-Phase-3 saved project loads with the new field at its `Default` (`Free`) — nih-plug's `#[persist]` already handles a missing field this way; no migration code.

`TrackModulation`'s existing `#[derive(serde::Serialize, serde::Deserialize)]` covers the new field automatically because `TriggerSource` is plain serde.

## §3 The engine — edge detection, rate oscillator, phase reset

`Modulation` (the runtime in `modulation.rs`) gains two parallel arrays:

```
prev_active: u16          // last block's active-row mask, for cell-light edge detection
hz_phases: [f32; ROWS]    // Free-Hz oscillator phase per row, advances 0..1 and wraps
```

`Modulation::new` initialises both to zero. `Modulation::reset` zeros both too (transport stopped→playing edge consistently re-zeroes all modulation state).

**The fire-decision step.** `update_block` gains an `active_mask: u16` argument (the engine's `last_active`, passed by `AudioEngine::process` — see §3.1) and runs this loop **before** the existing per-MSEG advance:

```
fires: u16 = 0
for row in 0..ROWS:
    let cur_lit  = (active_mask & (1 << row)) != 0
    let prev_lit = (prev_active & (1 << row)) != 0
    let fire = match config[row].trigger:
        Free       => false
        CellLight  => cur_lit && !prev_lit
        FreeHz{hz} =>
            if hz <= 0.0 { false }
            else {
                hz_phases[row] += (block_len as f32 * hz) / sample_rate as f32
                if hz_phases[row] >= 1.0 {
                    hz_phases[row] -= hz_phases[row].floor()   // wrap, retain fractional
                    true
                } else {
                    false
                }
            }
    if fire: fires |= 1 << row
prev_active = active_mask
```

**Reset.** Then, for each `row` with `fires & (1<<row)` set, zero `phases[row][0..3]` (all three of the track's MSEGs in lockstep).

**Existing advance.** The current per-MSEG advance loop (the 2b `for row { for k { mseg_phase_delta → advance → store phase → value_at_phase → apply }}`) runs unchanged after the reset. Rows that fired this block start from phase 0; rows that didn't continue from where they were.

All of this is allocation-free, lock-free, no `unsafe`. The arrays are stack-sized. The free-Hz step does one float multiply and a wrap per block per row — sub-microsecond.

### §3.1 The plumbing into `AudioEngine::process`

`AudioEngine::process` already has `active` (the per-segment row mask) and stores `last_active` at the end. `Modulation::update_block` runs **before** the engine's segment loop, with the engine's `last_active` field passed as `active_mask`. Concretely: at the top of `AudioEngine::process`, the engine calls
`self.modulation.update_block(n, bpm, sr, active_mask = self.last_active, &mut self.effects, &self.track_effects)`.
This is the only `process()`-side change; `last_active` is read in the same place 2b reads it.

**Cell-light latency.** Because the engine passes the **previous** block's final mask, a row whose first lit segment occurred during block N fires at the **start of block N+1** — up to one block of latency (≈1.3 ms at 64-sample blocks, ≈21 ms at 1024-sample blocks). This is intentional: the fire-decision lives at the natural call site (top of `process`) and the modulation engine is already block-resolution. A future polish can move edge detection into the segment loop for sample-accurate response if the latency turns out to be musically problematic.

For the first block ever (`last_active = 0`), no row will fire on cell-light — correct: nothing was active before.

## §4 The editor — the trigger dropdown + rate dial

In the per-track effect editor's MODULATION section header, **left of** the existing 3-way Amp/M1/M2 selector:

- A **trigger dropdown** (the existing `DropdownState<EffectAction>` gains a new `Trigger` action variant) — items are `Free run`, `Cell light`, `Free Hz`.
- When the source is `Free Hz`, a small **rate dial** appears immediately to the dropdown's right. Logarithmic 0.05..20 Hz, default 1.0 Hz; bound to `track_modulation[t].trigger`'s inner `hz`. Hidden for the other sources (`effect_hit` does not return its arm in those cases).

The setting is per-track, drawn once at the top of MODULATION (not per active MSEG). Switching `selected_mseg` changes which MSEG the pane shows but never the trigger control — the same setting governs all three.

`effect_editor.rs` gains:
- `EffectLayout` fields `trigger: (f32,f32,f32,f32)` (the dropdown trigger rect) and `trigger_rate: (f32,f32,f32,f32)` (the rate dial). Both physical-pixel rects scaled from the logical layout.
- `EffectHit::Trigger` and `EffectHit::TriggerRate` variants.
- `effect_hit` returns `Trigger` when the cursor is on the dropdown rect; `TriggerRate` only when the current source is `FreeHz`.
- `trigger_items()` returning the dropdown labels in `TriggerSource`-order (`Free run`, `Cell light`, `Free Hz`).
- `draw_modulation_controls` extended to draw the trigger dropdown trigger and (conditionally) the rate dial. The dial uses `param_dial::draw_dial` with a log-skewed normalisation between the dial position (0..1) and the `hz` value, similar to the lowpass-cutoff dial's spirit — concrete formula: `hz = 0.05 * (20.0 / 0.05).powf(norm)`, `norm = (hz / 0.05).log(20.0 / 0.05)` (clamped).

`MultosisWindow` wires the new hits:
- `EffectHit::Trigger` → open the shared dropdown for `EffectAction::Trigger` with `trigger_items()`.
- On `DropdownEvent::Selected(EffectAction::Trigger, idx)` → write `track_modulation[t].trigger = …` (mapping `idx` to the variant — `idx == 2` (Free Hz) seeds the variant with the dial's current `hz`, or `1.0` if no previous value), `mark_config_dirty()`.
- `EffectHit::TriggerRate` → begin a depth-style drag for the rate dial; on update, write the new `hz` back into the `FreeHz` variant, `mark_config_dirty()`.

The kind-switch path (Phase 2c §7) does NOT touch the trigger field — switching effect kind has no bearing on the modulation trigger.

## §5 Defaults & persistence

- `TrackModulation::default_for_row` sets `trigger: TriggerSource::Free`. Every existing default row keeps Phase 2b behaviour.
- Persistence: `TrackModulation` already derives `Serialize/Deserialize`; adding the `trigger` field extends the serialised shape additively. A project saved before Phase 3 loads with the new field at its `Default` (`Free`), per nih-plug's `#[persist]` semantics — no migration code.

## §6 Out of scope (Phase 4+)

- Additional trigger sources: MIDI note-on, transient detection on the dry input, transport-step / loop-boundary edges. The `TriggerSource` enum is open; new variants add their own edge-detect / oscillator branch in §3 without disturbing existing data.
- Per-MSEG triggers (currently per-track — the chosen scope).
- Retrigger probability / chance, retrigger delay, attack/decay-rate modulation by velocity-like inputs.
- A visual indicator in the MSEG pane showing the moment a trigger fired (a flash, etc.).
- Changing the Phase 2b free-running default — Phase 3 is additive.

## §7 Testing

Per CLAUDE.md — TDD, inline `#[cfg(test)]`, `cargo nextest`.

- **Edge detection.** A `CellLight`-triggered row fires exactly once on the inactive→active edge; doesn't re-fire while the row stays active; fires again on a fresh edge.
- **Free Hz oscillator.** With a known Hz / SR / block_len, the fire count over N blocks matches the analytic expectation within ±1; the phase wraps modulo 1.0 (fractional remainder retained); `hz ≤ 0` never fires; very high `hz` (per-block-or-more) fires at least once per block.
- **Phase reset.** After a row fires, the three MSEG phases observably reset to 0 next block (each starts the next advance from 0); rows that did not fire keep their previous phases.
- **`Free`-source backward-compat.** A `Free`-source row's `update_block` output is identical, sample-for-sample, to today's `Modulation::update_block` — no observable behaviour change for projects that haven't touched the new control.
- **Persistence.** A `TrackModulation` with each `TriggerSource` variant round-trips through serde JSON. A JSON missing the `trigger` key deserialises to `TriggerSource::Free` (the additive-default semantics).
- **Editor.** `effect_hit` returns `Trigger` on the dropdown rect and `TriggerRate` only when the source is `FreeHz`; `trigger_items` lists exactly the three sources in source-enum order; the rate-dial log mapping round-trips (`hz → norm → hz` within tolerance).
- **Smoke test.** Standalone bin: setting a row to `Cell light` makes its filter sweep re-aim itself every time that row sounds; `Free Hz` produces an audibly steady rate; `Free run` is indistinguishable from 2b. The grid editor, effect editor, and `< Grid` still work.
