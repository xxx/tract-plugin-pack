# Multosis Phase 2, Milestone 2c — Effect Editor & Tabbed Shell — Design

**Date:** 2026-05-19
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

Milestone 2a gave Multosis a per-track effect abstraction; 2b added the 3-MSEG-per-track modulation engine. Both left their config (`track_effects`, `track_modulation`) editable but bridged into the audio engine only once, at `initialize()` — there is no UI to view or change a track's effect, its parameters, or its MSEGs. Milestone 2c builds that UI: a **persistent left-edge track listing** plus a **per-track effect editor**, and the **live config handoff** that makes edits audible immediately.

2c is the last milestone of Phase 2. After it, every Phase 2 deliverable named in the Phase 1 design (§Phase 2: "the effect-editor UI tab, and the tabbed shell") is complete.

## Reference

- Phase 1 design `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — §7 (the grid editor UI; "the tabbed shell is Phase 2"), §Phase 2 bullet.
- Phase 2a design `docs/superpowers/specs/2026-05-18-multosis-phase-2a-design.md` — the effect abstraction and the `initialize()` config bridge.
- Phase 2b design `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md` — the modulation engine and `TrackModulation`.
- Current code:
  - `multosis/src/editor.rs` — `MultosisWindow` (the baseview handler: `draw`, `on_event`), `MultosisEditor`, `create()`. Holds `params`, `wavefront_display`, `seq_status`, `grid_handoff`, `reset_request`, drag state, the `grid_cache`.
  - `multosis/src/editor/grid_view.rs` — grid layout constants, `cell_zone`/`row_at`/`column_at` hit-testing, `GridCache`, `draw_*`.
  - `multosis/src/editor/toolbar.rs` — the top toolbar (drawn full-width).
  - `multosis/src/effects.rs` — `Effect`, `ParamSpec`, `EffectKind` (`ALL`, `name()`), `EffectInstance` (`new`, `kind`), `TrackEffect { kind, params: [f32; MAX_EFFECT_PARAMS] }`.
  - `multosis/src/modulation.rs` — `TrackModulation { msegs: [MsegData; 3], targets: [Option<usize>; 2], depths: [f32; 2] }`, `Modulation`.
  - `multosis/src/engine.rs` — `AudioEngine`: `set_effects`, `set_modulation`, `process` (computes a per-segment `active: u16` row mask), `wavefront()`.
  - `multosis/src/lib.rs` — `MultosisParams` (`#[persist]` `track_effects`, `track_modulation`), `process()` (the `initialize()` bridge, the `reset_request` flag), `create()`.
- Existing widgets in `tiny-skia-widgets` (`use tiny_skia_widgets as widgets;`): `dropdown` (`DropdownState`, `draw_dropdown_popup`), `param_dial` (a rotary dial with right-click text entry), `controls` (button / slider / **stepped selector**), `mseg` (`MsegData`, `MsegEditState` — `new()` full editor / `new_curve_only()`; `on_mouse_down/move/up`, `MsegEdit::Changed`; `draw_mseg(pixmap, text_renderer, rect, data, state, scale)`; `mseg_layout`, `phase_to_x`, `value_to_y`). `miff` is the live reference consumer of the MSEG editor widget.

## §1 The view shell

`MultosisWindow` gains GUI-only navigation state — **not persisted, not automatable**:

```
enum View { Grid, Effect }
view: View                 // starts at View::Grid
selected_track: usize      // 0..16, the track the Effect view edits
```

The top **toolbar stays full-width and shared** across both views (unchanged). Below it the window splits into a left **track listing** (§2, width `TRACK_PANEL_W`, always drawn) and a **main area** to its right. The main area shows the grid in `View::Grid` and the effect editor (§3) in `View::Effect`.

- Clicking a track-listing entry sets `selected_track` and switches to `View::Effect`.
- The effect editor's `← Back to Grid` control switches to `View::Grid`. `selected_track` is retained.
- The window opens in `View::Grid`.

**Window width.** `WINDOW_WIDTH` grows by `TRACK_PANEL_W` (the panel is new screen real estate, not taken from the grid). `WINDOW_HEIGHT` is unchanged. The grid's drawing origin and all of `grid_view`'s pixel↔cell hit-testing shift right by `TRACK_PANEL_W`; the grid layout is otherwise untouched. The editor stays freely resizable — `scale = physical_width / WINDOW_WIDTH` as today; the `window_size_matches_the_grid` test is updated for the new width.

**Event/draw routing.** `MultosisWindow::draw` always draws the toolbar and the track listing, then dispatches the main area by `view`. `on_event` routes mouse events: toolbar hits first (both views); then, by `view`, either the existing grid/region/paint handling or the effect editor's hit-testing. The track-listing strip is hit-tested in both views.

## §2 The track listing

A vertical list of 16 entries down the left edge, below the toolbar, drawn in **both** views. Each entry shows, left to right:

- the **track number** (1–16);
- the **effect kind name** (`EffectKind::name()` of `track_effects[row].kind`);
- a **"currently sounding" dot** — lit when the track is producing audio this moment.

In `View::Grid` the 16 entries align vertically with the 16 grid rows, so the listing doubles as grid row labels. In `View::Effect` the entry for `selected_track` is highlighted.

**Interaction.** A left click on entry `r` selects track `r` and switches to `View::Effect` (§1). The listing is not a drag surface.

**The "sounding" signal.** `AudioEngine::process` already computes, per segment, an `active: u16` bitmask (bit `r` = row `r` has a lit, enabled cell under the wavefront). The engine retains the last block's mask in a field, exposed via `AudioEngine::active_mask() -> u16`. `lib.rs`'s `process()` publishes it — after the engine runs — into a new `Arc<AtomicU16>` (`active_rows`), mirroring how the wavefront is published to `WavefrontDisplay`. The track listing reads this atomic each frame; bit `r` set → entry `r`'s dot is lit. `active_rows` is created in `MultosisPlugin`, shared into the editor via `create()`.

## §3 The effect editor

Shown in the main area in `View::Effect`, editing `selected_track`. Two stacked sections under an editor bar.

**Editor bar.** A `← Back to Grid` control (left) and an `Editing Track N` label.

**EFFECT section.**
- An **effect-kind dropdown** (`dropdown` widget) listing `EffectKind::ALL` by `name()`. Selecting a kind changes `track_effects[selected_track].kind` (see §7 for the param/target consequences).
- One **rotary dial** (`param_dial`) per parameter the current effect declares (`EffectInstance::new(kind).parameters()` — name, min, max, default). The dial spans the `ParamSpec` range linearly; it carries the parameter's name. Right-click text entry is inherited from `param_dial`. Editing a dial writes `track_effects[selected_track].params[i]`.

**MODULATION section.**
- A **3-way stepped selector** (`controls` stepped selector) — `Amp` / `MSEG 1` / `MSEG 2` — chooses the **active MSEG** (`selected_mseg: 0|1|2`, GUI-only window state). Selecting a different MSEG just re-points the one MSEG pane; it does not reset anything.
- When an **assignable** MSEG (`MSEG 1`/`MSEG 2`, i.e. `selected_mseg ∈ {1,2}`) is active, two controls appear:
  - a **target dropdown** — items are `(none)` followed by the current effect's parameter names; choosing `(none)` sets `targets[selected_mseg-1] = None`, choosing parameter `i` sets `Some(i)`;
  - a **depth dial** (`param_dial`, bipolar −1..1) bound to `depths[selected_mseg-1]`.
  For the **amplitude** MSEG (`selected_mseg == 0`) these two controls are hidden — the amplitude MSEG has no target or depth (2b §3).
- The **MSEG pane** (§4) — one MSEG editor showing the active MSEG.

All edits mutate the persisted mutexes in place and raise the dirty flag (§5).

## §4 The MSEG pane & ghost rendering

The MSEG pane is the existing `mseg` editor widget in **full-editor** mode (`MsegEditState::new()` — the sync-mode / length / play-mode / randomize / style strip is wanted; `miff`'s curve-only mode is not). `MultosisWindow` holds **one** `MsegEditState`; switching `selected_mseg` re-points it at a different `MsegData` (`msegs[selected_mseg]` of `track_modulation[selected_track]`). Mouse events in the pane drive `on_mouse_down/move/up`; an `MsegEdit::Changed` result raises the dirty flag.

**Ghost curves.** Behind the active MSEG, the two inactive MSEGs of the same track render as faint, non-interactive context curves. This needs a new function in the `tiny-skia-widgets` `mseg` module — a ghost-curve renderer that draws only a `MsegData`'s curve polyline within the editor's canvas sub-rect, in a faint colour, with no nodes/markers/strip. The effect editor draws the two ghosts first, then `draw_mseg` for the active MSEG on top. The ghost renderer reuses the public `mseg_layout` / `phase_to_x` / `value_to_y` geometry so ghosts align exactly with the active curve.

## §5 Editing the config & the live handoff

The editor edits the **already-persisted** state directly: `params.track_effects: Arc<Mutex<[TrackEffect; 16]>>` and `params.track_modulation: Arc<Mutex<[TrackModulation; 16]>>`. Each edit locks the relevant mutex, mutates in place, and unlocks (GUI thread — locking is fine here).

**The dirty flag.** A new `Arc<AtomicBool>` (`config_dirty`), created in `MultosisPlugin` and shared into the editor via `create()` (the same pattern as `reset_request`). Any editor edit — kind change, parameter dial, MSEG curve edit, MSEG strip change, target, depth — stores `true`.

**The re-bridge.** In `lib.rs`'s `process()`, before the engine runs: if `config_dirty` is set, attempt to `try_lock()` **both** configs; on success, re-bridge them into the engine (`engine.set_effects(&cfg)` / `engine.set_modulation(&mcfg)` — the same calls `initialize()` makes) and only **then** clear `config_dirty`. If either `try_lock` fails (the editor holds a lock this instant), leave `config_dirty` set and retry next block. Clearing the flag only after a successful re-bridge means no edit is ever lost. Allocation-free, `try_lock` only — audio-thread-safe.

**Incremental `set_effects`.** Today `AudioEngine::set_effects` rebuilds every `EffectInstance` from scratch, which resets DSP state (a click on every edit). 2c makes it incremental: for each row, if `config[r].kind` equals the live `self.effects[r].kind()`, keep the existing instance and only re-apply each `set_param` (no DSP-state reset → parameter edits are glitch-free); only a **kind change** rebuilds that row's instance (`EffectInstance::new`, `set_sample_rate`, `set_param` — an audible swap is expected and acceptable when you change the effect). `self.track_effects` is updated unconditionally so the modulation engine reads fresh base values. `EffectInstance::new` and the `[TrackEffect; 16]` copy are stack-only; `set_modulation`/`set_config` copy a `Copy` array — all allocation-free.

`set_modulation` already just copies the config; MSEG runtime phases are untouched, so MSEG edits apply smoothly under the free-running clock.

## §6 Module structure

- New `multosis/src/editor/track_list.rs` — `TRACK_PANEL_W`, the per-entry layout, `track_at(px, py, scale) -> Option<usize>` hit-testing, and the panel draw (number, effect name, sounding dot). GUI-only.
- New `multosis/src/editor/effect_editor.rs` — the effect-editor layout, hit-testing (back control, kind dropdown, param dials, MSEG selector, target dropdown, depth dial, MSEG pane rect), and draw. Holds no persistent state itself; reads `track_effects`/`track_modulation` and the window's `selected_track`/`selected_mseg`/`MsegEditState`.
- `multosis/src/editor.rs` — `MultosisWindow` gains `view`, `selected_track`, `selected_mseg`, a `MsegEditState`, the dropdown states, the `active_rows`/`config_dirty` `Arc`s, and routes `draw`/`on_event` by `view`.
- `multosis/src/editor/grid_view.rs` — grid coordinates shift right by `TRACK_PANEL_W` (origin + hit-testing); layout otherwise unchanged.
- `multosis/src/engine.rs` — retains the last `active` mask; adds `active_mask()`.
- `multosis/src/lib.rs` — `MultosisPlugin` owns `active_rows: Arc<AtomicU16>` and `config_dirty: Arc<AtomicBool>`; `process()` does the re-bridge and publishes `active_rows`; `create()` passes both to the editor.
- `tiny-skia-widgets` `mseg` module — gains the ghost-curve renderer (§4).

## §7 Defaults & the kind switch

- The window opens in `View::Grid`; `selected_track = 0`, `selected_mseg = 0`.
- **Changing a track's effect kind** (the EFFECT dropdown): set `track_effects[t].kind` to the new kind and **reset `params` to that kind's defaults** (`EffectInstance::new(kind).parameters()[i].default` for each `i`, zero past the kind's count). Then clamp the track's two assignable-MSEG `targets`: any `Some(i)` with `i >=` the new kind's parameter count becomes `None` (both current effects have two parameters, so in practice targets survive; the clamp keeps the model sound as effects are added). Raise the dirty flag.
- No new persisted state — 2c adds only GUI-runtime state and two `Arc` signals. Projects saved before 2c are unaffected (`track_effects`/`track_modulation` already persist from 2a/2b).

## §8 Out of scope (Phase 3)

- New effect kinds, more than two effect parameters, log-scaled parameter dials, an effect bypass/enable per track.
- Envelope retriggering and alternative MSEG trigger sources (Phase 3, per 2b §7).
- Presets, the seed, copy/paste of a track's effect or modulation between tracks.
- Persisting the current view / selected track across plugin reloads.
- Automating effect parameters from the host (the effect params remain plugin state edited only in the GUI, as in 2a/2b).

## §9 Testing

Per CLAUDE.md — TDD, inline `#[cfg(test)]` modules, `cargo nextest`. UI rendering is verified by the manual smoke test; the testable logic:

- **Track-list hit-testing** — `track_at` maps a pixel in entry `r`'s band to `Some(r)`, and a pixel outside the panel to `None`, across scale factors.
- **Effect-editor hit-testing** — pixels resolve to the right control (back / kind dropdown / each param dial / MSEG selector segment / target dropdown / depth dial / MSEG pane), and disjointly.
- **The kind switch** — changing kind resets `params` to the new kind's defaults and clears any now-out-of-range MSEG target; an in-range target survives.
- **Incremental `set_effects`** — a re-bridge with an unchanged kind preserves the effect's DSP state (output continuity — no reset transient); a re-bridge with a changed kind installs the new effect.
- **The handoff** — `config_dirty` stays set when the re-bridge can't lock and clears only after a successful re-bridge; an edit made through the editor's mutate path becomes audible (the engine's output changes after the next `process`).
- **`active_mask`** — the engine reports the last block's active-row mask; a row with a lit, enabled cell under the wavefront sets its bit.
- **Window size** — `WINDOW_WIDTH` equals the toolbar/grid width plus `TRACK_PANEL_W` (the layout-consistency test is updated).
- The standalone bin: the track listing shows each track's effect and lights dots as tracks sound; clicking a track opens its editor; changing kind, dials, MSEG curves, target and depth are all audible immediately; `← Back to Grid` returns; the grid still edits correctly in its shifted position.
