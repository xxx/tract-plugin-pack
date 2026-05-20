# Multosis Phase 3 — Envelope Retriggering & Trigger Sources — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Multosis a per-track `TriggerSource` (Free / CellLight / FreeHz) that resets the row's three MSEG phases on its event, plus the editor controls to pick it.

**Architecture:** `TrackModulation` gains a `trigger: TriggerSource` enum (serde). `Modulation` keeps a `prev_active` mask and a per-row Free-Hz oscillator phase; once per process block, before the existing per-MSEG advance, it decides which rows fire (cell-light edge-detect against `prev_active`, or Free-Hz wrap), zeroes those rows' three MSEG phases, then runs the unchanged 2b advance. The effect editor's MODULATION section gains a trigger dropdown and (for Free Hz) a log-scaled rate dial.

**Tech Stack:** Rust (nightly), nih-plug, `tiny-skia-widgets`, serde, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-19-multosis-phase-3-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message ends with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` lines below omit it — add it to each.

---

## Pre-existing state (186 multosis tests after Phase 2c, branch `multosis`)

- `multosis/src/modulation.rs`:
  - `pub struct TrackModulation { pub msegs: [MsegData; 3], pub targets: [Option<usize>; 2], pub depths: [f32; 2] }` — `Clone, PartialEq, Debug, Serialize, Deserialize`.
  - `impl TrackModulation { pub fn default_for_row(row: usize) -> Self; pub fn clamp_targets(&mut self, param_count: usize) }`.
  - `pub fn mseg_phase_delta(...)`, `pub fn assignable_value(...)` — pure helpers.
  - `pub struct Modulation { config: [TrackModulation; ROWS], phases: [[f32; 3]; ROWS], amplitudes: [f32; ROWS] }`.
  - `impl Modulation { pub fn new(); pub fn set_config(&mut self, &[TrackModulation; ROWS]); pub fn reset(&mut self); pub fn amplitude(&self, row) -> f32; pub fn update_block(&mut self, block_len: usize, bpm: f64, sample_rate: f64, effects: &mut [EffectInstance; ROWS], track_effects: &[TrackEffect; ROWS]); #[cfg(test)] pub fn phases_all_zero(&self) -> bool }`.
  - `const ROWS: usize = 16`.
  - Imports: `use crate::effects::{Effect, EffectInstance, ParamSpec, TrackEffect}; use tiny_skia_widgets::{advance, value_at_phase, MsegData, PlayMode, SyncMode}`.
  - Existing tests call `update_block(64, 120.0, 48_000.0, &mut effects, &track_effects)` (5 args). Phase 3 changes the signature to 6 args (a new `active_mask: u16` after `sample_rate`); every call site updates.
- `multosis/src/engine.rs`:
  - `AudioEngine { propagator, clock, effects: [EffectInstance; ROWS], sample_rate, track_effects: [TrackEffect; ROWS], modulation: Modulation, last_active: u16 }`.
  - At the top of `process`, after computing `n = left.len().min(right.len())`, the engine calls `self.modulation.update_block(n, bpm, self.sample_rate as f64, &mut self.effects, &self.track_effects)`. Phase 3 adds `self.last_active` to this call.
  - `self.last_active = active;` is set at the end of `process` (after the segment loop) — so at the *top* of the next block, `self.last_active` is the previous block's final mask, which is what cell-light edge detection needs.
- `multosis/src/editor/effect_editor.rs`:
  - `pub struct EffectLayout { pub back, kind, dials: [...; DIAL_SLOTS], mseg_selector, target, depth, mseg_pane }` — all `(f32,f32,f32,f32)` physical-pixel rects.
  - `pub const DIAL_SLOTS: usize = 4` (= `MAX_EFFECT_PARAMS`).
  - `pub fn effect_layout(scale: f32) -> EffectLayout`. The current numbers (in logical px, relative to `ox = MARGIN + TRACK_PANEL_W`, `oy = STATUS_H + GUTTER`):
    - `back = (ox, oy+4, 90, 26)`
    - `kind = (ox, oy+50, 150, 28)`
    - `dials[i] = (ox + 180 + i*96, oy+44, 80, 80)`
    - `mseg_selector = (ox, oy+168, 240, 26)`
    - `target = (ox+470, oy+167, 170, 28)`
    - `depth = (ox+660, oy+162, 70, 70)`
    - `mseg_pane = (ox, oy+208, mw, 422)`
  - `pub enum EffectHit { Back, Kind, Dial(usize), MsegSelector(usize), Target, Depth, MsegPane }`.
  - `pub fn effect_hit(px, py, scale, param_count: usize, selected_mseg: usize) -> Option<EffectHit>`.
  - `pub fn kind_items() -> Vec<&'static str>`; `pub fn target_items(kind) -> Vec<&'static str>`; `pub fn target_from_item(item) -> Option<usize>`; `pub fn target_to_item(target) -> usize`.
  - `pub fn draw_effect_section(pixmap, tr, track, track_index, kind_dropdown_open, scale)`; `pub fn draw_modulation_controls(pixmap, tr, selected_mseg, kind, target, depth, target_dropdown_open, scale)`.
- `multosis/src/editor.rs`:
  - `MultosisWindow` holds `selected_track: usize`, `selected_mseg: usize`, `mseg_edit: MsegEditState`, `kind_dropdown: DropdownState<EffectAction>` (shared with the target dropdown), `effect_dial_drag: DragState<EffectHit>`, `mseg_last_click_time/_pos`, `config_dirty: Arc<AtomicBool>`, `active_rows: Arc<AtomicU16>`.
  - `enum EffectAction { Kind, Target }`.
  - `MultosisWindow::mark_config_dirty()` sets the flag.
  - `on_effect_press(px, py, shift)` dispatches `EffectHit` and is called from the Left `ButtonPressed` arm when `view == View::Effect`.
  - `apply_effect_dial(i, norm)`, `apply_depth_drag(norm)`, `apply_kind_switch(kind)`, `apply_target_selection(idx)`, `selected_track_effect()`, `selected_track_modulation()`, `selected_track_param_count()`, `param_spec(i)`.
  - `draw_effect_view(&mut self)` draws the EFFECT section, the active MSEG (`draw_mseg`), the inactive MSEGs as ghosts (`draw_mseg_ghost`), and the modulation controls (`draw_modulation_controls`). The dropdown popup is drawn last in `draw()`.

All geometry below is in **logical** units unless a function name or comment says "physical"; physical = logical × `scale`.

---

### Task 1: `TriggerSource` enum + `TrackModulation` field

**Files:**
- Modify: `multosis/src/modulation.rs`

- [ ] **Step 1: Write the failing tests**

Append to `modulation.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn trigger_source_default_is_free() {
        assert_eq!(TriggerSource::default(), TriggerSource::Free);
    }

    #[test]
    fn trigger_source_variants_serde_round_trip() {
        for src in [
            TriggerSource::Free,
            TriggerSource::CellLight,
            TriggerSource::FreeHz { hz: 2.5 },
        ] {
            let json = serde_json::to_string(&src).unwrap();
            let back: TriggerSource = serde_json::from_str(&json).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn track_modulation_with_trigger_serde_round_trips() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.trigger = TriggerSource::FreeHz { hz: 4.0 };
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.trigger, TriggerSource::FreeHz { hz: 4.0 });
    }

    #[test]
    fn track_modulation_loads_missing_trigger_as_free() {
        // A JSON shaped like a pre-Phase-3 TrackModulation (no "trigger" key)
        // deserialises with trigger = Free, per serde's additive default.
        let tm = TrackModulation::default_for_row(0);
        let json = serde_json::to_string(&tm).unwrap();
        // Strip the trigger field from the JSON to simulate the old shape.
        let stripped = strip_trigger_field(&json);
        let back: TrackModulation = serde_json::from_str(&stripped).unwrap();
        assert_eq!(back.trigger, TriggerSource::Free);
    }

    fn strip_trigger_field(json: &str) -> String {
        // Naively remove the `"trigger":<value>,` substring. Works for the
        // serde_json default representation of small enums.
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let mut obj = v.as_object().unwrap().clone();
        obj.remove("trigger");
        serde_json::to_string(&serde_json::Value::Object(obj)).unwrap()
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(trigger_source) + test(track_modulation_with_trigger) + test(track_modulation_loads_missing_trigger)'`
Expected: build failure — `cannot find type TriggerSource` and no field `trigger` on `TrackModulation`.

- [ ] **Step 3: Write minimal implementation**

In `modulation.rs`, add the enum (above `TrackModulation`):

```rust
/// The event that causes a track's three MSEG phases to reset to 0.
/// Per Phase 3 design — Free is the Phase 2b free-running default; CellLight
/// fires on the row's inactive→active edge under the wavefront; FreeHz fires
/// every `1.0/hz` seconds independently of any sync.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    Free,
    CellLight,
    FreeHz { hz: f32 },
}

impl Default for TriggerSource {
    fn default() -> Self {
        TriggerSource::Free
    }
}
```

Extend `TrackModulation`:

```rust
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackModulation {
    pub msegs: [MsegData; 3],
    pub targets: [Option<usize>; 2],
    pub depths: [f32; 2],
    /// The event that resets all three of this row's MSEG phases.
    #[serde(default)]
    pub trigger: TriggerSource,
}
```

The `#[serde(default)]` attribute on `trigger` is what makes the missing-key test pass (the field defaults to `TriggerSource::Free` when absent).

In `TrackModulation::default_for_row`, add `trigger: TriggerSource::Free` to the literal at the end (after `depths: [0.4, 0.0]`):

```rust
        TrackModulation {
            msegs: [amplitude, sweep, spare],
            targets: [Some(0), None],
            depths: [0.4, 0.0],
            trigger: TriggerSource::Free,
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(trigger_source) + test(track_modulation_with_trigger) + test(track_modulation_loads_missing_trigger)'`
Expected: PASS — 4 tests.
Run: `cargo nextest run -p multosis` — PASS, 190 tests (186 + 4 new).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/modulation.rs
git commit -m "feat(multosis): add TriggerSource and the per-track trigger field"
```

---

### Task 2: The `Modulation` runtime — edge detect, Free-Hz oscillator, phase reset

**Files:**
- Modify: `multosis/src/modulation.rs`
- Modify: `multosis/src/engine.rs`

The biggest task. `update_block` gains an `active_mask: u16` argument, the runtime gains `prev_active`/`hz_phases`, the fire-decision step runs before the existing 2b per-MSEG advance, and the engine passes `self.last_active` at the call site.

- [ ] **Step 1: Write the failing tests**

Append to `modulation.rs`'s test module. First, a test-only phase accessor will be needed:

```rust
    #[test]
    fn fires_last_block_default_is_zero() {
        let m = Modulation::new();
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn cell_light_fires_on_inactive_to_active_edge() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[3].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Block 1: row 3 was inactive last block (prev=0) and is active now -> fires.
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
        // Block 2: row 3 still active -> does NOT re-fire (no edge).
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        // Block 3: row 3 went inactive -> no fire (only active edges fire).
        m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
        // Block 4: row 3 re-armed (inactive -> active) -> fires again.
        m.update_block(64, 120.0, 48_000.0, 1 << 3, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 3);
    }

    #[test]
    fn free_hz_fires_at_roughly_the_expected_rate() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 10 Hz, 48 kHz, 480-sample blocks -> exactly 1 fire per block, on average.
        // Run 100 blocks; expect ~100 fires (allow ±1 for the boundary).
        let mut fires = 0usize;
        for _ in 0..100 {
            m.update_block(480, 120.0, 48_000.0, 0, &mut effects, &track_effects);
            if m.fires_last_block() & (1 << 5) != 0 {
                fires += 1;
            }
        }
        assert!(
            (99..=101).contains(&fires),
            "10 Hz over 100 blocks of 1 cycle each: got {fires} fires"
        );
    }

    #[test]
    fn free_hz_nonpositive_never_fires() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 0.0 };
        cfg[1].trigger = TriggerSource::FreeHz { hz: -2.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..50 {
            m.update_block(480, 120.0, 48_000.0, 0, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block() & 0b11, 0);
        }
    }

    #[test]
    fn fire_zeros_the_rows_three_phases() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        // Set row 2's MSEGs to short Beat lengths so the phases advance fast,
        // then verify that a fire on row 2 resets them.
        cfg[2].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Advance many blocks with no fires -> phases drift away from 0.
        for _ in 0..50 {
            m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);
        }
        // At least one of row 2's three phases should be non-zero now.
        let any_nonzero =
            (0..3).any(|k| m.phase_for_test(2, k) > 1e-6);
        assert!(any_nonzero, "phases should have drifted with no fires");
        // Now fire row 2 (inactive->active edge).
        m.update_block(64, 120.0, 48_000.0, 1 << 2, &mut effects, &track_effects);
        // After a fire, the row's three phases are reset to 0 (the per-MSEG
        // advance then re-runs from 0; the post-advance phase equals the
        // block's first dt, not 0). So the right test is: less than they
        // would have been without a fire — verified by comparing to the
        // amplitude (flat-1.0 default) seen one block later.
        for k in 0..3 {
            // Read each phase: it should be a *small* value (one block's dt),
            // not the multi-cycle accumulation it was before.
            let phi = m.phase_for_test(2, k);
            assert!(
                phi.abs() < 0.1,
                "after a fire, MSEG[{k}] phase should be near 0, got {phi}"
            );
        }
    }

    #[test]
    fn free_source_does_not_fire() {
        let mut m = Modulation::new();
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        // default_for_row's trigger is Free.
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..20 {
            m.update_block(64, 120.0, 48_000.0, 0xFFFF, &mut effects, &track_effects);
            assert_eq!(m.fires_last_block(), 0);
        }
    }

    #[test]
    fn reset_zeroes_hz_phases_and_prev_active() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Run a block so hz_phases advances and prev_active becomes nonzero.
        m.update_block(64, 120.0, 48_000.0, 1, &mut effects, &track_effects);
        m.reset();
        assert!(m.hz_phases_all_zero());
        assert_eq!(m.prev_active_for_test(), 0);
    }
```

Also update **every existing** test in `modulation.rs` that calls `update_block`. Search the file (`rg "update_block\(" multosis/src/modulation.rs`) and update each call site to insert `0` as the new fifth argument (after `sample_rate`, before `&mut effects`). For example:

Before: `m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);`
After:  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);`

(Passing `0` means "no row is active" — the previous default behaviour is unchanged for Free-source rows.)

Likewise, update **every** `update_block` call in `multosis/src/engine.rs`'s tests (`rg "update_block\(" multosis/src/engine.rs`) — search and update.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo build -p multosis 2>&1 | head -30`
Expected: many `wrong number of arguments` errors (the signature mismatch) and `cannot find method fires_last_block` / `phase_for_test` / `hz_phases_all_zero` / `prev_active_for_test`.

- [ ] **Step 3: Write minimal implementation**

In `modulation.rs`, extend the `Modulation` struct (after `amplitudes`):

```rust
pub struct Modulation {
    config: [TrackModulation; ROWS],
    /// Free-running phase per `[row][mseg]`.
    phases: [[f32; 3]; ROWS],
    /// Latest per-row amplitude gain, set by `update_block`.
    amplitudes: [f32; ROWS],
    /// Last block's active-row mask, for cell-light edge detection.
    prev_active: u16,
    /// Free-Hz oscillator phase per row, advances 0..1 and wraps modulo 1.
    hz_phases: [f32; ROWS],
    /// The rows that fired this block (bit `r` set). Set by `update_block`.
    fires: u16,
}
```

Update `Modulation::new`:

```rust
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            phases: [[0.0; 3]; ROWS],
            amplitudes: [1.0; ROWS],
            prev_active: 0,
            hz_phases: [0.0; ROWS],
            fires: 0,
        }
    }
```

Update `Modulation::reset` to also zero the new fields:

```rust
    pub fn reset(&mut self) {
        self.phases = [[0.0; 3]; ROWS];
        self.prev_active = 0;
        self.hz_phases = [0.0; ROWS];
        self.fires = 0;
    }
```

Change the `update_block` signature — add `active_mask: u16` after `sample_rate`:

```rust
    pub fn update_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        active_mask: u16,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
```

At the **top** of `update_block`'s body (before the existing per-MSEG advance loop), insert the fire-decision + phase-reset step:

```rust
        // Phase 3: decide which rows fire this block, then reset their phases.
        let mut fires: u16 = 0;
        for row in 0..ROWS {
            let cur_lit = (active_mask & (1 << row)) != 0;
            let prev_lit = (self.prev_active & (1 << row)) != 0;
            let fire = match self.config[row].trigger {
                TriggerSource::Free => false,
                TriggerSource::CellLight => cur_lit && !prev_lit,
                TriggerSource::FreeHz { hz } => {
                    if hz <= 0.0 {
                        false
                    } else {
                        self.hz_phases[row] +=
                            (block_len as f32 * hz) / sample_rate as f32;
                        if self.hz_phases[row] >= 1.0 {
                            // Retain fractional remainder; multiple wraps in
                            // one block still count as one fire (spec §7).
                            self.hz_phases[row] -= self.hz_phases[row].floor();
                            true
                        } else {
                            false
                        }
                    }
                }
            };
            if fire {
                fires |= 1 << row;
            }
        }
        // Reset phases for firing rows — all three MSEGs in lockstep.
        for row in 0..ROWS {
            if fires & (1 << row) != 0 {
                self.phases[row] = [0.0; 3];
            }
        }
        self.fires = fires;
        self.prev_active = active_mask;
```

The existing 2b per-MSEG advance loop (`for row in 0..ROWS { for k in 0..3 { ... } }`) runs unchanged after this block.

Add the test-only accessors (place them next to the existing `phases_all_zero`):

```rust
    /// Test helper: the row that fired this block (set by `update_block`).
    #[cfg(test)]
    pub fn fires_last_block(&self) -> u16 {
        self.fires
    }

    /// Test helper: the current phase for `[row][k]`.
    #[cfg(test)]
    pub fn phase_for_test(&self, row: usize, k: usize) -> f32 {
        self.phases[row][k]
    }

    /// Test helper: true when every Free-Hz oscillator phase is 0.
    #[cfg(test)]
    pub fn hz_phases_all_zero(&self) -> bool {
        self.hz_phases.iter().all(|&p| p == 0.0)
    }

    /// Test helper: the `prev_active` mask.
    #[cfg(test)]
    pub fn prev_active_for_test(&self) -> u16 {
        self.prev_active
    }
```

In `multosis/src/engine.rs`, find the `self.modulation.update_block(...)` call at the top of `process` and add `self.last_active` as the new fifth argument:

```rust
        self.modulation.update_block(
            n,
            bpm,
            self.sample_rate as f64,
            self.last_active,
            &mut self.effects,
            &self.track_effects,
        );
```

(Tests in `engine.rs` that call `engine.process(...)` keep their existing signatures — they don't call `update_block` directly.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(cell_light) + test(free_hz) + test(fire_zeros) + test(free_source_does_not_fire) + test(fires_last_block_default) + test(reset_zeroes_hz)'`
Expected: PASS — 7 tests.
Run: `cargo nextest run -p multosis` — PASS, 197 tests (190 + 7).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/modulation.rs multosis/src/engine.rs
git commit -m "feat(multosis): cell-light & Free-Hz trigger sources in the modulation runtime"
```

---

### Task 3: Effect-editor layout, hit-test, draw helpers for the trigger controls

**Files:**
- Modify: `multosis/src/editor/effect_editor.rs`

Pure helpers and layout — wiring into `MultosisWindow` is Task 4. The trigger controls go on the same row as the MSEG selector, to the left; the MSEG selector / target / depth shift right to make room.

- [ ] **Step 1: Write the failing tests**

Append to `effect_editor.rs`'s test module:

```rust
    #[test]
    fn trigger_items_lists_three_sources() {
        let items = trigger_items();
        assert_eq!(items, ["Free run", "Cell light", "Free Hz"]);
    }

    #[test]
    fn trigger_from_and_to_item_round_trip() {
        // 0 -> Free, 1 -> CellLight, 2 -> FreeHz{<carried hz>}.
        assert_eq!(trigger_from_item(0, 1.0), TriggerSource::Free);
        assert_eq!(trigger_from_item(1, 1.0), TriggerSource::CellLight);
        assert_eq!(trigger_from_item(2, 3.5), TriggerSource::FreeHz { hz: 3.5 });
        assert_eq!(trigger_to_item(TriggerSource::Free), 0);
        assert_eq!(trigger_to_item(TriggerSource::CellLight), 1);
        assert_eq!(trigger_to_item(TriggerSource::FreeHz { hz: 99.0 }), 2);
    }

    #[test]
    fn hz_norm_round_trips_within_range() {
        for &hz in &[0.05_f32, 0.1, 1.0, 5.0, 20.0] {
            let norm = hz_to_norm(hz);
            assert!((0.0..=1.0).contains(&norm), "norm for hz {hz}: {norm}");
            let back = norm_to_hz(norm);
            // Log mapping: relative error < 1e-4.
            assert!(
                ((back - hz) / hz).abs() < 1e-4,
                "round-trip {hz} -> {norm} -> {back}"
            );
        }
        // hz below range clamps to min; above clamps to max.
        assert_eq!(hz_to_norm(0.001), 0.0);
        assert_eq!(hz_to_norm(100.0), 1.0);
    }

    #[test]
    fn layout_includes_trigger_rects_disjoint_from_other_controls() {
        let lay = effect_layout(1.0);
        assert!(!rects_overlap(lay.trigger, lay.mseg_selector));
        assert!(!rects_overlap(lay.trigger, lay.trigger_rate));
        assert!(!rects_overlap(lay.trigger_rate, lay.mseg_selector));
        // Trigger sits to the LEFT of the MSEG selector on the same row.
        assert!(lay.trigger.0 < lay.mseg_selector.0);
        // Both fit within the main area.
        assert!(lay.trigger.0 >= 0.0);
    }

    #[test]
    fn effect_hit_returns_trigger_on_the_dropdown_rect() {
        let lay = effect_layout(1.0);
        let (tx, ty, tw, th) = lay.trigger;
        // Trigger hit fires regardless of selected_mseg.
        assert_eq!(
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 0, false),
            Some(EffectHit::Trigger)
        );
        assert_eq!(
            effect_hit(tx + tw / 2.0, ty + th / 2.0, 1.0, 2, 1, false),
            Some(EffectHit::Trigger)
        );
    }

    #[test]
    fn effect_hit_returns_trigger_rate_only_when_free_hz() {
        let lay = effect_layout(1.0);
        let (rx, ry, rw, rh) = lay.trigger_rate;
        // FreeHz: rate dial is hot.
        assert_eq!(
            effect_hit(rx + rw / 2.0, ry + rh / 2.0, 1.0, 2, 0, true),
            Some(EffectHit::TriggerRate)
        );
        // Not FreeHz: rate dial is not returned (falls through).
        let other = effect_hit(rx + rw / 2.0, ry + rh / 2.0, 1.0, 2, 0, false);
        // The fall-through may resolve to MsegPane or None depending on
        // layout; the important check is that it is NOT TriggerRate.
        assert_ne!(other, Some(EffectHit::TriggerRate));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(trigger_items_lists) + test(trigger_from_and_to_item) + test(hz_norm_round_trips) + test(layout_includes_trigger) + test(effect_hit_returns_trigger)'`
Expected: build failure — `cannot find function trigger_items` / `trigger_from_item` / `hz_to_norm` / no `trigger` field on `EffectLayout` / `effect_hit` signature mismatch.

- [ ] **Step 3: Write minimal implementation**

In `effect_editor.rs`:

Add the import (top of file, with the other use lines):

```rust
use crate::modulation::TriggerSource;
```

Add to `EffectLayout` (after `mseg_pane`):

```rust
    /// Trigger-source dropdown trigger.
    pub trigger: (f32, f32, f32, f32),
    /// Trigger-rate dial — only hot when the source is `FreeHz`.
    pub trigger_rate: (f32, f32, f32, f32),
```

Update `effect_layout` — shift the existing MODULATION-row controls right to make room for `trigger` + `trigger_rate`, and add the two new rects. The new MODULATION-row layout (logical):

```rust
    // MODULATION section — trigger + rate on the left, then MSEG selector +
    // target + depth. The trigger and rate are PER-TRACK (govern all 3 MSEGs).
    let trigger = l(ox, oy + 168.0, 130.0, 26.0);
    let trigger_rate = l(ox + 146.0, oy + 162.0, 60.0, 38.0);
    let mseg_selector = l(ox + 222.0, oy + 168.0, 240.0, 26.0);
    let target = l(ox + 478.0, oy + 167.0, 170.0, 28.0);
    let depth = l(ox + 664.0, oy + 162.0, 70.0, 70.0);
    let mseg_pane = l(ox, oy + 208.0, mw, 422.0);
```

(The MSEG pane y origin stays at `oy + 208.0` so it does not overlap the depth dial bottom — verify by inspection that 208 ≥ 162 + 70 + a small gap; the existing layout already has this geometry intact for the depth dial.)

Update the `EffectLayout` constructor at the end of `effect_layout` to include the new fields:

```rust
    EffectLayout {
        back,
        kind,
        dials,
        mseg_selector,
        target,
        depth,
        mseg_pane,
        trigger,
        trigger_rate,
    }
```

Update `EffectHit` — add two variants:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectHit {
    Back,
    Kind,
    Dial(usize),
    MsegSelector(usize),
    Target,
    Depth,
    MsegPane,
    /// The per-track trigger-source dropdown.
    Trigger,
    /// The per-track trigger-rate dial (only hot when the source is FreeHz).
    TriggerRate,
}
```

Update `effect_hit` — add the `is_free_hz: bool` argument and the new arms. The full new signature and body:

```rust
pub fn effect_hit(
    px: f32,
    py: f32,
    scale: f32,
    param_count: usize,
    selected_mseg: usize,
    is_free_hz: bool,
) -> Option<EffectHit> {
    let lay = effect_layout(scale);
    if in_rect(lay.back, px, py) {
        return Some(EffectHit::Back);
    }
    if in_rect(lay.kind, px, py) {
        return Some(EffectHit::Kind);
    }
    for i in 0..param_count.min(DIAL_SLOTS) {
        if in_rect(lay.dials[i], px, py) {
            return Some(EffectHit::Dial(i));
        }
    }
    // Per-track trigger controls — checked before the per-MSEG selector.
    if in_rect(lay.trigger, px, py) {
        return Some(EffectHit::Trigger);
    }
    if is_free_hz && in_rect(lay.trigger_rate, px, py) {
        return Some(EffectHit::TriggerRate);
    }
    // MSEG selector — three equal segments.
    let (sx, sy, sw, sh) = lay.mseg_selector;
    if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
        let seg = (((px - sx) / (sw / 3.0)) as usize).min(2);
        return Some(EffectHit::MsegSelector(seg));
    }
    if selected_mseg != 0 {
        if in_rect(lay.target, px, py) {
            return Some(EffectHit::Target);
        }
        if in_rect(lay.depth, px, py) {
            return Some(EffectHit::Depth);
        }
    }
    if in_rect(lay.mseg_pane, px, py) {
        return Some(EffectHit::MsegPane);
    }
    None
}
```

(All call sites of `effect_hit` must add a `bool` argument — Task 4 wires it.)

Add the helper functions and the draw extension (place after `target_to_item`, before the test module):

```rust
/// The trigger-source dropdown items, in `TriggerSource` discriminant order.
pub fn trigger_items() -> [&'static str; 3] {
    ["Free run", "Cell light", "Free Hz"]
}

/// Build a `TriggerSource` from a dropdown item index. `carried_hz` is the
/// `hz` to seed `FreeHz` with (the dial's current value, or a default).
pub fn trigger_from_item(item: usize, carried_hz: f32) -> TriggerSource {
    match item {
        0 => TriggerSource::Free,
        1 => TriggerSource::CellLight,
        _ => TriggerSource::FreeHz { hz: carried_hz },
    }
}

/// The dropdown item index for a `TriggerSource`.
pub fn trigger_to_item(src: TriggerSource) -> usize {
    match src {
        TriggerSource::Free => 0,
        TriggerSource::CellLight => 1,
        TriggerSource::FreeHz { .. } => 2,
    }
}

/// The trigger-rate dial range (Hz).
pub const TRIGGER_RATE_MIN_HZ: f32 = 0.05;
pub const TRIGGER_RATE_MAX_HZ: f32 = 20.0;

/// Map a 0..1 dial position to a Hz value, log-skewed across the rate range.
pub fn norm_to_hz(norm: f32) -> f32 {
    let n = norm.clamp(0.0, 1.0);
    TRIGGER_RATE_MIN_HZ * (TRIGGER_RATE_MAX_HZ / TRIGGER_RATE_MIN_HZ).powf(n)
}

/// Map a Hz value to a 0..1 dial position. Clamps to the rate range.
pub fn hz_to_norm(hz: f32) -> f32 {
    if hz <= TRIGGER_RATE_MIN_HZ {
        return 0.0;
    }
    if hz >= TRIGGER_RATE_MAX_HZ {
        return 1.0;
    }
    (hz / TRIGGER_RATE_MIN_HZ).log(TRIGGER_RATE_MAX_HZ / TRIGGER_RATE_MIN_HZ)
}

/// Draw the per-track trigger dropdown trigger and (when the source is
/// `FreeHz`) the rate dial. Called as part of the MODULATION section draw.
pub fn draw_trigger_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    trigger: TriggerSource,
    trigger_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    let label = match trigger {
        TriggerSource::Free => "Free run",
        TriggerSource::CellLight => "Cell light",
        TriggerSource::FreeHz { .. } => "Free Hz",
    };
    widgets::dropdown::draw_dropdown_trigger(
        pixmap,
        tr,
        lay.trigger,
        label,
        trigger_dropdown_open,
    );
    if let TriggerSource::FreeHz { hz } = trigger {
        let (rx, ry, rw, rh) = lay.trigger_rate;
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            rx + rw / 2.0,
            ry + rh / 2.0,
            (rw.min(rh) / 2.0) - 6.0 * scale,
            "Rate",
            &format!("{hz:.2} Hz"),
            hz_to_norm(hz),
        );
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(trigger_items_lists) + test(trigger_from_and_to_item) + test(hz_norm_round_trips) + test(layout_includes_trigger) + test(effect_hit_returns_trigger)'`
Expected: PASS — 6 tests.
Run: `cargo nextest run -p multosis` — PASS (203 tests; but **the existing `effect_hit` call site in `editor.rs` no longer compiles** — see Step 5).

Note: this task changes the `effect_hit` signature (adds a `bool` arg). Until Task 4 wires the new arg, `editor.rs`'s call to `effect_hit(...)` is broken. To keep this task green on its own, add a temporary `false` at the `editor.rs` call site as part of Step 3 — search for `effect_hit(` in `editor.rs`, and on the call passing the existing 5 args, append `, false` as a sixth arg. Task 4 replaces this with the real `is_free_hz` value.

- [ ] **Step 5: Update the `editor.rs` call site to keep the build green**

`rg "effect_hit\(" multosis/src/editor.rs` — find the single call (in `on_effect_press`). It currently passes `px, py, self.scale_factor, param_count, self.selected_mseg`. Append `, false` (temporary placeholder).

Then re-run:
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 203 tests.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 6: Commit**

```bash
git add multosis/src/editor/effect_editor.rs multosis/src/editor.rs
git commit -m "feat(multosis): trigger-control layout + hit-test + helpers (effect editor)"
```

---

### Task 4: Wire the trigger controls into `MultosisWindow`

**Files:**
- Modify: `multosis/src/editor.rs`

The `EffectAction` enum gains a `Trigger` variant; the shared `DropdownState<EffectAction>` handles all three roles. The MODULATION draw extends to call `draw_trigger_controls`. `effect_hit` is called with the real `is_free_hz` flag. The new hits are dispatched.

- [ ] **Step 1: Update `EffectAction` enum**

Find `enum EffectAction { Kind, Target }` in `editor.rs` and add `Trigger`:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EffectAction {
    Kind,
    Target,
    Trigger,
}
```

- [ ] **Step 2: Extend `draw_effect_view`**

In `MultosisWindow::draw_effect_view`, after the call to `effect_editor::draw_modulation_controls(...)`, add:

```rust
        // Per-track trigger control + (conditionally) rate dial.
        let trigger = self.selected_track_modulation().trigger;
        effect_editor::draw_trigger_controls(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            trigger,
            self.kind_dropdown.is_open_for(EffectAction::Trigger),
            self.scale_factor,
        );
```

Add a `use crate::modulation::TriggerSource;` at the top of `editor.rs` if not already imported.

- [ ] **Step 3: Update the `effect_hit` call**

Replace the temporary `, false` placeholder at the `effect_hit(...)` call in `on_effect_press` with the real flag — `is_free_hz` is true when the active trigger source is `FreeHz`:

```rust
        let trigger = self.selected_track_modulation().trigger;
        let is_free_hz = matches!(trigger, TriggerSource::FreeHz { .. });
        let hit = effect_editor::effect_hit(
            px,
            py,
            self.scale_factor,
            param_count,
            self.selected_mseg,
            is_free_hz,
        );
```

- [ ] **Step 4: Dispatch the new hits in `on_effect_press`**

In the `match hit` block, add arms for the two new variants. The `Trigger` arm opens the shared dropdown for `EffectAction::Trigger`; the `TriggerRate` arm begins a rate-dial drag. The `effect_dial_drag` is currently typed `DragState<EffectHit>` and already handles `Dial(i)` and `Depth` — extending it to `TriggerRate` is one new arm in the drag-update path.

```rust
            Some(EffectHit::Trigger) => {
                let trigger = self.selected_track_modulation().trigger;
                let current = effect_editor::trigger_to_item(trigger);
                let items = effect_editor::trigger_items();
                let window_size = (self.physical_width as f32, self.physical_height as f32);
                self.kind_dropdown.open(
                    EffectAction::Trigger,
                    effect_editor::effect_layout(self.scale_factor).trigger,
                    &items,
                    current,
                    false,
                    window_size,
                );
                return true;
            }
            Some(EffectHit::TriggerRate) => {
                let trigger = self.selected_track_modulation().trigger;
                if let TriggerSource::FreeHz { hz } = trigger {
                    let current_norm = effect_editor::hz_to_norm(hz);
                    self.effect_dial_drag.begin_drag(
                        EffectHit::TriggerRate,
                        current_norm,
                        false,
                    );
                    return true;
                }
                return false;
            }
```

(Keep the `Some(EffectHit::MsegSelector)` / `Target` / `Depth` / `MsegPane` arms as they were — Task 9 of 2c left them populated.)

- [ ] **Step 5: Apply the rate-drag**

Find the `CursorMoved` handling for `effect_dial_drag` in `on_event` (where it dispatches `Dial(i)` / `Depth`). Add a `TriggerRate` arm:

```rust
                    EffectHit::TriggerRate => {
                        let current = effect_editor::hz_to_norm(
                            match self.selected_track_modulation().trigger {
                                TriggerSource::FreeHz { hz } => hz,
                                _ => 1.0,
                            },
                        );
                        if let Some(norm) =
                            self.effect_dial_drag.update_drag(shift, current)
                        {
                            self.apply_trigger_rate_drag(norm);
                        }
                    }
```

Add the helper `apply_trigger_rate_drag` to `impl MultosisWindow` (next to `apply_depth_drag`):

```rust
    /// Update the trigger rate from the rate-dial drag's normalised value.
    fn apply_trigger_rate_drag(&mut self, norm: f32) {
        let new_hz = effect_editor::norm_to_hz(norm);
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            if let TriggerSource::FreeHz { hz } = &mut cfg[self.selected_track].trigger
            {
                *hz = new_hz;
                self.mark_config_dirty();
            }
        }
    }
```

- [ ] **Step 6: Apply the trigger-dropdown selection**

Find the `DropdownEvent::Selected(action, idx)` handler. There are currently arms for `EffectAction::Kind` → `apply_kind_switch` and `EffectAction::Target` → `apply_target_selection`. Add a `Trigger` arm:

```rust
                EffectAction::Trigger => self.apply_trigger_selection(idx),
```

Add the helper to `impl MultosisWindow`:

```rust
    /// Apply a trigger-dropdown selection: convert the item index to a
    /// `TriggerSource` (carrying the current Hz if any), write it, mark dirty.
    fn apply_trigger_selection(&mut self, idx: usize) {
        let carried_hz = match self.selected_track_modulation().trigger {
            TriggerSource::FreeHz { hz } => hz,
            _ => 1.0,
        };
        let new_trigger = effect_editor::trigger_from_item(idx, carried_hz);
        if let Ok(mut cfg) = self.params.track_modulation.lock() {
            cfg[self.selected_track].trigger = new_trigger;
            self.mark_config_dirty();
        }
    }
```

- [ ] **Step 7: Run the build and the full suite**

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS, 203 tests (unchanged — Task 4 adds no unit tests; the wiring is verified by Task 5's smoke test).
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 8: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): wire the trigger dropdown and rate dial into the editor"
```

---

### Task 5: Verification

**Files:** none — checks and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS, all green (203 tests).
Run: `cargo nextest run --workspace` — PASS, all green.
Run: `cargo clippy --workspace -- -D warnings` — no warnings.
Run: `cargo fmt --check` — clean.

- [ ] **Step 2: Release build and bundle**

Run: `cargo xtask native build --bin multosis --release` — standalone binary builds.
Run: `cargo xtask native nih-plug bundle multosis --release` — VST3 + CLAP + standalone bundle, no errors.

- [ ] **Step 3: Manual smoke test**

Run `cargo run --bin multosis` (or load the bundle in a host). Confirm:
- Open a track's effect editor. The MODULATION section shows a new **trigger dropdown** (`Free run`) to the left of the Amp/M1/M2 selector. The rate dial is hidden.
- Picking **Cell light** in the dropdown makes that track's filter/crusher modulation re-aim itself every time the track sounds (every time a lit cell on that row enters the wavefront).
- Picking **Free Hz** reveals the rate dial. Turning the dial changes the retrigger rate audibly (0.05..20 Hz, log-scaled — slow at the low end, fast at the high end).
- Picking **Free run** restores the Phase 2b free-running behaviour (the modulation drifts at the MSEG's own length).
- The trigger setting persists across plugin reload; pre-Phase-3 saved projects load with `Free run` (no migration).
- The grid editor, the effect editor (kind dropdown, dials, MSEG selector, target, depth, MSEG pane), `< Grid`, and the toolbar all still work.

Report the smoke-test observations.

- [ ] **Step 4: Commit (only if Step 1 required formatting edits)**

```bash
git add -A
git commit -m "style(multosis): apply rustfmt for the Phase 3 trigger work"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- Multosis has a per-track `TriggerSource` (`Free` / `CellLight` / `FreeHz { hz }`) that resets all three of the row's MSEG phases on its event; the engine's cell-light edge detect and Free-Hz oscillator both run allocation-free and lock-free in the audio path.
- The effect editor's MODULATION section has a trigger dropdown and a conditional rate dial; edits are bridged live via the existing `config_dirty` handoff (Phase 2c §5).
- `cargo nextest run -p multosis` is green (203 tests); `cargo clippy --workspace -- -D warnings` is clean; the plugin bundles and the smoke test confirms each source.

## Spec coverage check (self-review)

- §1 `TriggerSource` enum — added with `Free` / `CellLight` / `FreeHz { hz }`, plain serde, `Default = Free` (Task 1).
- §2 Per-track state — `TrackModulation.trigger: TriggerSource` with `#[serde(default)]`; `default_for_row` sets it `Free`; pre-Phase-3 JSON loads as `Free` (Task 1).
- §3 Engine — `Modulation { prev_active, hz_phases, fires }`, `update_block(active_mask, …)` runs the fire-decision before the 2b advance, zeros firing rows' phases (Task 2). Engine passes `self.last_active` (Task 2).
- §3.1 Plumbing — `last_active` is the previous block's mask at the call site; the first block reads `0` (no fires); correct (Task 2).
- §4 Editor UI — trigger dropdown + (conditional) rate dial in the MODULATION header, log Hz mapping, shared `DropdownState<EffectAction>` extended with `Trigger`, `effect_hit` gains `is_free_hz` (Tasks 3, 4).
- §5 Defaults & persistence — covered above; `#[serde(default)]` provides the additive behaviour.
- §6 Out of scope — no MIDI / transient / per-MSEG / probability work in this milestone.
- §7 Testing — edge-detection / Free-Hz rate / non-positive Hz / phase reset / Free backward-compat / persistence / round-trips (Tasks 1, 2, 3); smoke test (Task 5).

## Note on task sequencing

Tasks 1–2 are pure DSP / data and ship green on their own. Task 3 changes `effect_hit`'s signature, so it includes the one-line `, false` placeholder at `editor.rs`'s call site to keep the build green; Task 4 replaces that placeholder with the real flag and wires the new hits. Task 5 is verification + smoke test.
