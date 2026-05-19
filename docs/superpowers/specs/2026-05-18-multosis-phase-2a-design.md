# Multosis Phase 2, Milestone 2a — Effect Abstraction — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Phase 1 made Multosis an audible routing sequencer using two hardwired "throwaway" effects (`Lowpass`, `Bitcrush`) selected globally by an `effect_bank` parameter. Phase 2 — per the Phase 1 spec §1.2 — replaces that with a real effect system: a standardized effect trait, a registry, per-track effect instances, a 3-MSEG modulation engine, and an effect-editor UI.

Phase 2 is built as three milestones:

- **2a — Effect abstraction (this spec).** The `Effect` trait, the effect registry, per-track effect instances in the data model, and conversion of the audio engine to trait-based effects. The Phase 1 Lowpass and Bitcrush are re-expressed as the first two trait effects.
- **2b — Modulation engine.** The 3-MSEG-per-effect system (one amplitude MSEG + two assignable), on its own clock, driving the parameter seam 2a establishes.
- **2c — Effect-editor UI + tabbed shell.** The tabbed shell, the per-track effect-assignment UI, and the effect-editor tab.

2a builds **only the abstraction and the engine conversion** — the modulation engine and the UI are deferred. As with the Phase 1 §1.3 Game-of-Life seam, 2a preserves a clean seam for 2b (the parameter model) without writing any 2b code.

## Reference

- Phase 1 design: `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — especially §1.1 (foundational model: routing routes triggers; a lit cell with no effect is silent but still routes), §3.1 (params vs. state), §6 (the audio engine and the two throwaway effects).
- Current code: `multosis/src/effects.rs` (the throwaway effects), `multosis/src/engine.rs` (`AudioEngine` — per-row effect processing), `multosis/src/lib.rs` (`MultosisParams`, the `effect_bank` `EnumParam`).

## §1 The `Effect` trait

Every effect is a plain struct implementing a standardized trait. Conceptually:

```
trait Effect {
    /// Process a stereo block in place. The audio engine hands each active
    /// row a copy of the dry input; the effect transforms it to wet.
    fn process(&mut self, left: &mut [f32], right: &mut [f32], sample_rate: f32);

    /// Clear all DSP state (filter memory, etc.) so a row's effect does not
    /// click when the row reactivates after being dark.
    fn reset(&mut self);

    /// The effect's modulatable parameters — name, range, default. Static per
    /// effect kind. Used by the 2b modulation engine and the 2c editor.
    fn parameters(&self) -> &[ParamSpec];

    /// Set parameter `index` (into `parameters()`) to `value`. In 2a the
    /// values are set once from the persisted config; 2b's MSEGs will write
    /// them every block. `index` out of range is ignored.
    fn set_param(&mut self, index: usize, value: f32);
}
```

`ParamSpec` is a small `Copy` descriptor: `{ name: &'static str, min: f32, max: f32, default: f32 }` (a parameter's units/skew can be added later if needed — YAGNI for 2a).

The trait is the contract 2b and 2c are written against. **No `dyn`** — see §2.

Constraints (per CLAUDE.md): `process`/`reset`/`set_param` run on the audio thread and must be allocation-free; no `unsafe`.

## §2 Registry and dispatch

- **`EffectKind`** — a `Copy` enum, the registry: `Lowpass`, `Bitcrush`. Helpers: `EffectKind::ALL` (the slice of all kinds) and `name(self) -> &'static str`. Adding an effect in a later phase = one variant + its struct.
- **`EffectInstance`** — a `Copy`-or-cheaply-movable enum holding the live effect: `Lowpass(LowpassEffect)`, `Bitcrush(BitcrushEffect)`. It delegates the `Effect` trait methods to the contained struct via `match`. `EffectInstance::new(kind: EffectKind) -> EffectInstance` constructs a fresh instance.

Enum dispatch (not `Box<dyn Effect>`) keeps effect state inline — no heap, no allocation when a track's effect changes, no `unsafe` — consistent with the workspace's enum-dispatch DSP (e.g. six-pack's saturation algorithms). The audio engine holds a fixed `[EffectInstance; 16]`, one per track row.

## §3 The two ported effects

Re-expressed from the Phase 1 throwaway effects as `Effect`-implementing structs, each holding its DSP state and current parameter values:

- **`LowpassEffect`** — a resonant lowpass (the Phase 1 SVF / one-pole+resonance). Parameters: `cutoff`, `resonance`.
- **`BitcrushEffect`** — sample-rate / bit-depth reduction. Parameters: `bit_depth`, `rate_reduction`.

Each effect's `parameters()` returns its `ParamSpec` list; `process` reads the current parameter values from its own fields. The Phase 1 row-index→character mapping is gone — character now comes from each track instance's own parameter values (§4).

## §4 Per-track effect config — persisted state

Each of the 16 track rows carries its own effect. The persisted state:

- **`TrackEffect`** — `{ kind: EffectKind, params: [f32; MAX_EFFECT_PARAMS] }`. `MAX_EFFECT_PARAMS` is a small fixed cap (4 — headroom past the current max of 2) so the array is serde-stable as effects are added. `params[i]` holds the value for the effect's `parameters()[i]`; entries past the kind's parameter count are unused.
- The plugin persists a `[TrackEffect; 16]` (16 ≤ 32, so the standard `#[persist]` JSON serde works — no hand-rolled serde, unlike `Grid`'s 512-cell array). It lives alongside the `Grid` in plugin state.
- **Default config** — `TrackEffect::default_for_row(row)` spreads kind and parameters by row index, so 2a plays with audible per-track variety before the 2c assignment UI exists: e.g. alternating Lowpass/Bitcrush across rows with parameters spread (row 0 darkest/most-crushed → row 15 open/clean), echoing the Phase 1 row-index character mapping. The exact spread is a detail for the plan; the requirement is that the default is musically varied across rows and demonstrably exercises both effects.

The Phase 1 **`effect_bank` parameter is removed** (and its `EffectBank` enum — `EffectKind` supersedes it). Removing a parameter changes the plugin's parameter set; hosts may need the plugin re-added (the known Bitwig param-rescan behaviour).

In 2a the config is **static** — loaded from persisted state (or the default) at initialization; there is no live editing yet. The GUI→audio handoff for live config edits is a 2c concern (it will mirror the `Grid` `Mutex` + `try_lock` handoff). 2a's audio engine simply builds its effect instances from the config once.

## §5 Audio-engine conversion

`AudioEngine` is converted from the hardwired throwaway path to trait-based effects:

- It owns `[EffectInstance; 16]`, built from the `[TrackEffect; 16]` config — each instance constructed via `EffectInstance::new(kind)` with `set_param` applied for each of the kind's parameters from the config's `params`.
- The Phase 1 §6 process flow is otherwise unchanged: determine active rows (a row is active if any of its cells is lit; dedupe per tick); for each active **enabled** row, copy the dry input into a scratch buffer, run that row's `EffectInstance` over it, scale by the row's **amplitude** (a per-row gain — constant `1.0` in 2a; the seam for 2b's amplitude MSEG), and sum into the wet accumulator; `out = lerp(dry, wet_sum, mix)`; apply `output_gain`; publish the wavefront.
- Per-row effect DSP state persists across steps (`reset` is only called on explicit reset / sample-rate change), so a row's effect does not click when the row reactivates — the Phase 1 §6 guarantee, preserved.
- A lit row whose effect is `enabled = false` (per-cell mute) contributes nothing — unchanged. (The "empty track" case from Phase 1 §1.1 does not arise in 2a: every track always has an `EffectKind`; the empty-track case stays a future concern.)
- No allocations on the audio thread; `try_lock()` only.

`engine.process` no longer takes an `EffectBank` argument; it uses the per-track instances it owns.

## §6 The 2b / 2c seam

2a deliberately builds the seam and nothing past it:

- **For 2b (modulation):** every effect declares its parameters (`parameters()`) and accepts values (`set_param`); the engine applies a per-row `amplitude` gain. 2b adds 3 MSEGs per track effect — one driving `amplitude`, two assignable to declared parameters — written each block onto this seam. No MSEG type, clock, or assignment exists in 2a.
- **For 2c (UI):** `EffectKind::ALL` / `name()` and `ParamSpec` give the editor everything it needs to render an effect's controls; `TrackEffect` is the per-track config the assignment UI edits. 2a writes no UI and no config handoff.

## §7 Testing

Per CLAUDE.md — TDD, inline `#[cfg(test)]` modules, `cargo nextest`.

- **Effect DSP** — `LowpassEffect`: a swept-cutoff test (low cutoff attenuates highs, high cutoff passes them); `BitcrushEffect`: quantization is observable (output takes a reduced set of levels / a held sample-rate). Each effect's `reset` clears state.
- **The trait surface** — `parameters()` returns the expected specs; `set_param` round-trips into the effect's behaviour (a parameter change changes the output); an out-of-range `set_param` index is a no-op.
- **Dispatch** — `EffectInstance::new(kind)` builds the right variant; the delegated trait methods reach the contained effect.
- **Config** — `TrackEffect` serde round-trip; `[TrackEffect; 16]` round-trips through the plugin's `#[persist]` path; `default_for_row` produces a varied config exercising both effect kinds.
- **Engine** — the converted `AudioEngine` runs per-track effects: a grid where two rows are lit and carry different effect kinds produces the sum of both effects' outputs; a disabled lit row is silent; effect state persists across steps (no reset between consecutive active steps).
- The standalone bin plays the sequencer with the default per-track config (verified by ear).

## §8 Out of scope (2b / 2c / later)

- The 3-MSEG modulation engine, the MSEG clock, parameter-to-MSEG assignment — 2b.
- The tabbed shell, per-track effect-assignment UI, the effect-editor tab, the live config handoff — 2c.
- Any new effects beyond the two ported — Phase 3.
- Automatic wet-level compensation; presets; the empty-track (effect-less row) case.
