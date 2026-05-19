# Multosis Phase 2, Milestone 2b — Modulation Engine — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a 3-MSEG-per-track modulation engine — one amplitude MSEG and two assignable MSEGs per track, free-running on their own clocks, driving the 2a effect-parameter and amplitude seams.

**Architecture:** A new `multosis/src/modulation.rs` holds a persisted `TrackModulation` config (3 reused `MsegData` + targets + depths), pure modulation/clock helpers, and a `Modulation` runtime that advances per-MSEG phases and applies modulation each block. The audio engine owns a `Modulation`, evaluates it once per block, and scales each row's wet output by its amplitude MSEG. The MSEG type itself is `tiny-skia-widgets`'s `MsegData`, reused unchanged.

**Tech Stack:** Rust (nightly), nih-plug, `tiny-skia-widgets` (`MsegData`), `cargo nextest`, serde.

**Reference:** `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message ends with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` lines below omit it — add it to each.

**Pre-existing state (156 multosis tests, 933 workspace tests green):**
- `tiny-skia-widgets` (a dependency of `multosis`, used as `use tiny_skia_widgets as widgets;`) exports, from its crate root, `MsegData`, `MsegNode`, `PlayMode`, `SyncMode`, `HoldMode`, `value_at_phase`, `advance`:
  - `MsegData` — `#[derive(Clone, Copy, PartialEq, Debug)]` + custom serde; all fields `pub`: `nodes: [MsegNode; 128]`, `node_count: usize`, `play_mode: PlayMode`, `hold: HoldMode`, `sync_mode: SyncMode`, `time_seconds: f32`, `beats: f32`, `time_divisions: u32`, `value_steps: u32`, `snap: bool`. `MsegData::default()` = a 2-node 0→1 ramp, `Triggered`, `Time` sync, `time_seconds = 1.0`, `beats = 1.0`. `insert_node(&mut self, time, value) -> Option<usize>`, `move_node(&mut self, idx, time, value)`.
  - `PlayMode { Triggered, Cyclic }`; `SyncMode { Time, Beat }`.
  - `value_at_phase(&MsegData, phase: f32) -> f32` — pure, returns the curve value (0..1) at phase (0..1, clamped).
  - `advance(&MsegData, phase: f32, dt: f32, released: bool) -> (f32, bool)` — pure; `dt` is a **phase delta in 0..1 space** (the caller converts time→phase); returns `(next_phase, finished)`. In `Cyclic` mode it wraps the phase.
  - `[MsegData; N]` and structs containing `MsegData` `#[derive(Serialize, Deserialize)]` directly (MsegData has custom serde).
- `multosis/src/effects.rs` — `Effect` trait (`process_sample`, `set_sample_rate`, `reset`, `parameters(&self) -> &'static [ParamSpec]`, `set_param(&mut self, usize, f32)`); `ParamSpec { name, min, max, default }` (`Copy`); `EffectInstance` (enum, implements `Effect`, `EffectInstance::new(kind)`); `TrackEffect { kind: EffectKind, params: [f32; MAX_EFFECT_PARAMS] }` (`MAX_EFFECT_PARAMS = 4`), `TrackEffect::default_for_row(row)`.
- `multosis/src/engine.rs` — `AudioEngine { propagator, clock, effects: [EffectInstance; 16], sample_rate: f32 }`; `new()`; `set_effects(&mut self, config: &[TrackEffect; 16])` (rebuilds `effects` from the config); `set_sample_rate(sr)`; `reset()`; `process_sample(&mut self, dry_l, dry_r, active: u16) -> (f32,f32)` (sums each active row's `effects[r].process_sample(...)`); `process(&mut self, left: &mut [f32], right: &mut [f32], playing: bool, samples_per_step: f64, mix: f32, auto_restart: bool, grid: &Grid)` (`#[allow(clippy::too_many_arguments)]`; walks the block in segments, per-sample calls `process_sample`). `ROWS = 16` is in scope. Engine tests call `engine.process(&mut left, &mut right, true, 10.0, mix, auto_restart, &grid)` (a `bpm` argument is added in Task 4).
- `multosis/src/lib.rs` — `MultosisParams` has `#[persist = "grid"]` and `#[persist = "track-effects"] Arc<Mutex<[crate::effects::TrackEffect; 16]>>`; `impl Default` initialises every field. The `Multosis` plugin's `process()` reads `let bpm = transport.tempo.unwrap_or(120.0);` and `let sps = crate::clock::samples_per_step(...)`, then calls `self.engine.process(&mut *left, &mut *right, playing, sps, mix, auto_restart, &self.grid)`. There is init code bridging the persisted config into the engine: the grid bridge and `if let Ok(cfg) = self.params.track_effects.lock() { self.engine.set_effects(&cfg); }` (after `set_sample_rate`). `pub mod` lines list the modules; there is no `modulation` module yet.

---

### Task 1: `TrackModulation` — the per-track modulation config

**Files:**
- Create: `multosis/src/modulation.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/modulation.rs` with this content:

```rust
//! The modulation engine — Phase 2 Milestone 2b. Three MSEGs per track row
//! (one amplitude + two assignable), free-running on their own clocks,
//! driving the 2a effect-parameter and amplitude seams.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2b-design.md`.

use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};

/// The number of track rows. Matches `crate::grid::ROWS`.
const ROWS: usize = 16;

/// One track row's modulation: three MSEGs and the two assignable MSEGs'
/// targets and depths. `msegs[0]` is the amplitude MSEG; `msegs[1]` and
/// `msegs[2]` are the assignable MSEGs — `targets[k]` / `depths[k]` belong to
/// `msegs[k + 1]`.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackModulation {
    pub msegs: [MsegData; 3],
    /// Target effect-parameter index for each assignable MSEG, or `None`.
    pub targets: [Option<usize>; 2],
    /// Bipolar modulation depth (−1..1) for each assignable MSEG.
    pub depths: [f32; 2],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_modulation_serde_round_trips() {
        let tm = TrackModulation::default_for_row(3);
        let json = serde_json::to_string(&tm).unwrap();
        let back: TrackModulation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tm);
    }

    #[test]
    fn track_modulation_array_serde_round_trips() {
        let cfg: [TrackModulation; ROWS] = std::array::from_fn(TrackModulation::default_for_row);
        let json = serde_json::to_string(&cfg).unwrap();
        let back: [TrackModulation; ROWS] = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }

    #[test]
    fn default_for_row_assigns_one_assignable_and_varies_by_row() {
        let a = TrackModulation::default_for_row(0);
        let b = TrackModulation::default_for_row(7);
        // msegs[1] is assigned to param 0; msegs[2] is unassigned.
        assert_eq!(a.targets[0], Some(0));
        assert_eq!(a.targets[1], None);
        assert!(a.depths[0] != 0.0);
        assert_eq!(a.depths[1], 0.0);
        // The assignable MSEG's loop length differs by row.
        assert!(a.msegs[1].beats != b.msegs[1].beats);
        // The amplitude MSEG is flat at 1.0.
        assert!(a.msegs[0].nodes[..a.msegs[0].node_count]
            .iter()
            .all(|n| (n.value - 1.0).abs() < 1e-6));
    }
}
```

Add `pub mod modulation;` to `multosis/src/lib.rs` (with the other `pub mod` lines, alphabetically — after `mod grid;`/`handoff` as fits).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(track_modulation) + test(default_for_row_assigns)'`
Expected: build failure — `no function default_for_row` / `TrackModulation` has no such method. (If `serde_json` is unavailable, it is already a `multosis` dev-dependency — confirm; the `effects.rs` `TrackEffect` tests use it.)

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/modulation.rs`, after the `TrackModulation` struct (before the `#[cfg(test)]` module):

```rust
impl TrackModulation {
    /// The default modulation for track row `row`. The amplitude MSEG is flat
    /// at 1.0 (no level change); `msegs[1]` is a cyclic triangle assigned to
    /// effect parameter 0, its loop length spread by row so each track drifts
    /// at its own rate; `msegs[2]` is an unused cyclic default.
    pub fn default_for_row(row: usize) -> Self {
        // msegs[0] — amplitude: flat at 1.0.
        let mut amplitude = MsegData::default();
        amplitude.nodes[0].value = 1.0;
        amplitude.nodes[1].value = 1.0;
        amplitude.play_mode = PlayMode::Cyclic;

        // msegs[1] — assignable: a cyclic triangle, Beat-synced, length by row.
        let mut sweep = MsegData::default(); // nodes (0,0) and (1,1)
        let _ = sweep.insert_node(0.5, 1.0); // -> (0,0) (0.5,1.0) (1,1.0)
        sweep.move_node(2, 1.0, 0.0); // last node value -> 0: triangle
        sweep.play_mode = PlayMode::Cyclic;
        sweep.sync_mode = SyncMode::Beat;
        sweep.beats = 4.0 + row as f32 * 2.0; // 4..34 beats across the rows

        // msegs[2] — assignable: unused default.
        let mut spare = MsegData::default();
        spare.play_mode = PlayMode::Cyclic;

        TrackModulation {
            msegs: [amplitude, sweep, spare],
            targets: [Some(0), None],
            depths: [0.4, 0.0],
        }
    }
}

impl Default for TrackModulation {
    fn default() -> Self {
        Self::default_for_row(0)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(track_modulation) + test(default_for_row_assigns)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles, no warnings. (`modulation.rs` imports only `MsegData`/`PlayMode`/`SyncMode` so far; Tasks 2–3 add the `crate::effects` and `advance`/`value_at_phase` imports as they need them.)

- [ ] **Step 5: Commit**

```bash
git add multosis/src/modulation.rs multosis/src/lib.rs
git commit -m "feat(multosis): add the TrackModulation per-track config"
```

---

### Task 2: Modulation math — the clock and the assignable mapping

**Files:**
- Modify: `multosis/src/modulation.rs`

- [ ] **Step 1: Write the failing test**

Add to `modulation.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn mseg_phase_delta_time_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Time;
        m.time_seconds = 2.0; // a 2-second envelope
        // One 48-sample block at 48 kHz advances 48 / (2 * 48000) of the cycle.
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn mseg_phase_delta_beat_sync() {
        let mut m = MsegData::default();
        m.sync_mode = SyncMode::Beat;
        m.beats = 4.0; // 4 beats long
        // At 120 BPM a beat is 0.5 s -> 4 beats = 2 s = 96000 samples @ 48k.
        let dt = mseg_phase_delta(&m, 48, 120.0, 48_000.0);
        assert!((dt - 48.0 / 96_000.0).abs() < 1e-9, "got {dt}");
    }

    #[test]
    fn assignable_value_midline_is_the_base() {
        let spec = ParamSpec { name: "p", min: 0.0, max: 100.0, default: 50.0 };
        // MSEG value 0.5 (midline) -> no deviation from the base.
        assert!((assignable_value(0.5, 40.0, 1.0, spec) - 40.0).abs() < 1e-6);
    }

    #[test]
    fn assignable_value_depth_and_sign() {
        let spec = ParamSpec { name: "p", min: 0.0, max: 100.0, default: 50.0 };
        // Full positive depth, MSEG at 1.0 -> base + 1*range, clamped to max.
        assert_eq!(assignable_value(1.0, 40.0, 1.0, spec), 100.0);
        // Negative depth inverts: MSEG at 1.0 -> base - range, clamped to min.
        assert_eq!(assignable_value(1.0, 40.0, -1.0, spec), 0.0);
        // Half depth, MSEG at 1.0 -> base + 0.5*range.
        assert!((assignable_value(1.0, 20.0, 0.5, spec) - 70.0).abs() < 1e-4);
    }

    #[test]
    fn assignable_value_always_within_range() {
        let spec = ParamSpec { name: "p", min: 5.0, max: 9.0, default: 7.0 };
        for &v in &[0.0_f32, 0.25, 0.5, 0.75, 1.0] {
            for &d in &[-1.0_f32, -0.3, 0.0, 0.6, 1.0] {
                let out = assignable_value(v, 8.0, d, spec);
                assert!((5.0..=9.0).contains(&out), "v {v} d {d} -> {out}");
            }
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(mseg_phase_delta) + test(assignable_value)'`
Expected: build failure — `cannot find function mseg_phase_delta` / `assignable_value`.

- [ ] **Step 3: Write minimal implementation**

Add the import `use crate::effects::ParamSpec;` to `modulation.rs`'s `use` section. Then add, after the `TrackModulation` `impl` blocks (before the test module):

```rust
/// The phase delta (0..1 space) one `block_len`-sample process block advances
/// `mseg`, given the host `bpm` and `sample_rate`. Honours the MSEG's
/// `sync_mode`: `Time` uses `time_seconds`, `Beat` converts `beats` via the
/// tempo. Returns 0.0 for a degenerate (zero/negative) length.
pub fn mseg_phase_delta(mseg: &MsegData, block_len: usize, bpm: f64, sample_rate: f64) -> f32 {
    let length_samples = match mseg.sync_mode {
        SyncMode::Time => mseg.time_seconds as f64 * sample_rate,
        SyncMode::Beat => mseg.beats as f64 * (60.0 / bpm) * sample_rate,
    };
    if length_samples > 0.0 {
        (block_len as f64 / length_samples) as f32
    } else {
        0.0
    }
}

/// The effective effect-parameter value for an assignable MSEG modulating
/// parameter `spec` around `base`. `mseg_value` is the MSEG's 0..1 output;
/// `depth` is the bipolar (−1..1) modulation depth. The MSEG midline (0.5)
/// leaves the parameter at `base`; the result is clamped to the parameter's
/// range.
pub fn assignable_value(mseg_value: f32, base: f32, depth: f32, spec: ParamSpec) -> f32 {
    let bipolar = mseg_value * 2.0 - 1.0;
    let deviation = bipolar * depth * (spec.max - spec.min);
    (base + deviation).clamp(spec.min, spec.max)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(mseg_phase_delta) + test(assignable_value)'`
Expected: PASS — 5 tests. Then `cargo build -p multosis` — compiles.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/modulation.rs
git commit -m "feat(multosis): add modulation clock and assignable-value math"
```

---

### Task 3: The `Modulation` runtime

**Files:**
- Modify: `multosis/src/modulation.rs`

- [ ] **Step 1: Write the failing test**

Add to `modulation.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn modulation_amplitude_reflects_the_amplitude_mseg() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] =
            std::array::from_fn(TrackEffect::default_for_row);
        m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        // The default amplitude MSEG is flat at 1.0.
        for r in 0..ROWS {
            assert!((m.amplitude(r) - 1.0).abs() < 1e-6, "row {r}: {}", m.amplitude(r));
        }
    }

    #[test]
    fn modulation_applies_an_assignable_mseg_to_its_effect() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        for e in &mut effects {
            e.set_sample_rate(48_000.0);
        }
        let track_effects: [TrackEffect; ROWS] =
            std::array::from_fn(TrackEffect::default_for_row);
        // Run a block, then drive the effects with a signal and capture output.
        m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_first: Vec<f32> = (0..200).map(|_| effects[0].process_sample(1.0, -1.0).0).collect();
        // Advance many blocks so the cyclic MSEG has moved, re-apply.
        for _ in 0..400 {
            m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        let after_later: Vec<f32> = (0..200).map(|_| effects[0].process_sample(1.0, -1.0).0).collect();
        // The modulated cutoff changed -> the filtered output differs.
        assert!(
            after_first != after_later,
            "an assigned MSEG should modulate the effect over time"
        );
    }

    #[test]
    fn modulation_reset_zeroes_phases() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(crate::effects::EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] =
            std::array::from_fn(TrackEffect::default_for_row);
        for _ in 0..100 {
            m.update_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        m.reset();
        // After reset, every phase is back at 0.
        assert!(m.phases_all_zero());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(modulation_amplitude) + test(modulation_applies) + test(modulation_reset)'`
Expected: build failure — `cannot find type Modulation`.

- [ ] **Step 3: Write minimal implementation**

Add the imports `use crate::effects::{Effect, EffectInstance, TrackEffect};` and extend the `tiny_skia_widgets` `use` to also bring in `advance` and `value_at_phase` (so it reads `use tiny_skia_widgets::{advance, value_at_phase, MsegData, PlayMode, SyncMode};`). Then add, after `assignable_value` (before the test module):

```rust
/// The modulation runtime owned by the audio engine — the per-track config,
/// each MSEG's free-running phase, and the latest per-row amplitude gain.
pub struct Modulation {
    config: [TrackModulation; ROWS],
    /// Free-running phase per `[row][mseg]`.
    phases: [[f32; 3]; ROWS],
    /// Latest per-row amplitude gain, set by `update_block`.
    amplitudes: [f32; ROWS],
}

impl Modulation {
    /// A runtime with the default per-row modulation and zeroed phases.
    pub fn new() -> Self {
        Self {
            config: std::array::from_fn(TrackModulation::default_for_row),
            phases: [[0.0; 3]; ROWS],
            amplitudes: [1.0; ROWS],
        }
    }

    /// Replace the per-track modulation config (bridged from persisted state
    /// at init — off the audio thread).
    pub fn set_config(&mut self, config: &[TrackModulation; ROWS]) {
        self.config = config.clone();
    }

    /// Reset every MSEG phase to 0.
    pub fn reset(&mut self) {
        self.phases = [[0.0; 3]; ROWS];
    }

    /// The latest amplitude gain for `row` (set by the previous `update_block`).
    pub fn amplitude(&self, row: usize) -> f32 {
        self.amplitudes[row]
    }

    /// Advance every MSEG one process block, evaluate it, and apply: the
    /// amplitude MSEG sets `amplitudes[row]`; each assigned assignable MSEG
    /// writes its target effect parameter via `set_param`. Allocation-free.
    pub fn update_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        for row in 0..ROWS {
            let tm = &self.config[row];
            for k in 0..3 {
                let mseg = &tm.msegs[k];
                let dt = mseg_phase_delta(mseg, block_len, bpm, sample_rate);
                let (next, _finished) = advance(mseg, self.phases[row][k], dt, false);
                self.phases[row][k] = next;
                let value = value_at_phase(mseg, next);
                if k == 0 {
                    // Amplitude MSEG.
                    self.amplitudes[row] = value;
                } else if let Some(target) = tm.targets[k - 1] {
                    // Assignable MSEG -> a target effect parameter.
                    if let Some(&spec) = effects[row].parameters().get(target) {
                        let base = track_effects[row].params[target];
                        let depth = tm.depths[k - 1];
                        effects[row].set_param(target, assignable_value(value, base, depth, spec));
                    }
                }
            }
        }
    }

    /// Test helper: true when every MSEG phase is 0.
    #[cfg(test)]
    pub fn phases_all_zero(&self) -> bool {
        self.phases.iter().flatten().all(|&p| p == 0.0)
    }
}

impl Default for Modulation {
    fn default() -> Self {
        Self::new()
    }
}
```

NOTE: `effects[row].parameters()` returns `&'static [ParamSpec]`; `.get(target)` guards an out-of-range target. `&spec` copies the `ParamSpec` (it is `Copy`). The three borrows in `update_block` (`&self.config` via `tm`, `&mut self.phases`/`&mut self.amplitudes`, the `effects`/`track_effects` arguments) — `tm` borrows `self.config` immutably while `self.phases`/`self.amplitudes` are written; if the borrow checker objects to holding `tm` across the `self.phases` writes, index `self.config[row]` inline instead of binding `tm`, or copy the needed `MsegData`/target/depth out first. Keep the logic identical.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(modulation_amplitude) + test(modulation_applies) + test(modulation_reset)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles with NO warnings (all of `modulation.rs`'s imports are now used).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/modulation.rs
git commit -m "feat(multosis): add the Modulation runtime"
```

---

### Task 4: Wire modulation into the audio engine

**Files:**
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/lib.rs`

The interlocked integration: the engine owns a `Modulation`, `process` gains a `bpm` argument, and the plugin persists + bridges the modulation config.

- [ ] **Step 1: Write / update the failing tests** (`multosis/src/engine.rs`)

- Every existing test that calls `engine.process(...)` (the six Phase-1 tests plus `engine_runs_per_track_effects`) takes a new `bpm: f64` argument — insert `120.0` immediately after the `samples_per_step` argument (the new signature is in Step 3). So `engine.process(&mut left, &mut right, true, 10.0, mix, auto_restart, &grid)` becomes `engine.process(&mut left, &mut right, true, 10.0, 120.0, mix, auto_restart, &grid)`.
- Add this new test:

```rust
    #[test]
    fn engine_applies_modulation() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let effect_cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        engine.set_effects(&effect_cfg);
        let mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        engine.set_modulation(&mod_cfg);
        let grid = Grid::default_routing();
        // Run two well-separated stretches of audio; the default per-track
        // assignable MSEG should make the wet output drift over time.
        let mut a = [0.4_f32; 64];
        let mut b = [0.4_f32; 64];
        engine.process(&mut a, &mut a.clone(), true, 10.0, 120.0, 1.0, true, &grid);
        for _ in 0..300 {
            let mut l = [0.4_f32; 64];
            let mut r = [0.4_f32; 64];
            engine.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, true, &grid);
        }
        engine.process(&mut b, &mut b.clone(), true, 10.0, 120.0, 1.0, true, &grid);
        assert!(a.iter().all(|s| s.is_finite()) && b.iter().all(|s| s.is_finite()));
        assert!(a != b, "modulation should make the output drift over time");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(engine_applies_modulation)'`
Expected: build failure — `no method named set_modulation`.

- [ ] **Step 3: Convert `AudioEngine`** (`multosis/src/engine.rs`)

- Imports: add `use crate::effects::TrackEffect;` (if not already imported) and `use crate::modulation::{Modulation, TrackModulation};`.
- `AudioEngine` gains two fields: `track_effects: [TrackEffect; 16]` (kept so modulation can read base parameter values) and `modulation: Modulation`.
- `AudioEngine::new()` — initialise `track_effects: std::array::from_fn(TrackEffect::default_for_row)` and `modulation: Modulation::new()`.
- `set_effects(&mut self, config: &[TrackEffect; 16])` — in addition to rebuilding `self.effects`, store `self.track_effects = *config` (or `config.clone()` — `TrackEffect` is `Copy`, so `*config` works).
- Add `pub fn set_modulation(&mut self, config: &[TrackModulation; 16])` — `self.modulation.set_config(config)`.
- `reset()` — additionally call `self.modulation.reset()`.
- `process` — add a `bpm: f64` parameter, placed immediately after `samples_per_step`: the new signature is
  `process(&mut self, left: &mut [f32], right: &mut [f32], playing: bool, samples_per_step: f64, bpm: f64, mix: f32, auto_restart: bool, grid: &Grid)`.
  At the **top of `process`**, before the segment loop, call modulation once for the block:
  ```rust
  let n = left.len().min(right.len());
  self.modulation.update_block(
      n,
      bpm,
      self.sample_rate as f64,
      &mut self.effects,
      &self.track_effects,
  );
  ```
  (Place this where `n` is computed; if `n` is already a local, reuse it.) The `#[allow(clippy::too_many_arguments)]` stays.
- `process_sample` — scale each active row's wet contribution by its amplitude: where it currently does `wet_l += l; wet_r += r_out;`, change to
  ```rust
  let amp = self.modulation.amplitude(r);
  wet_l += amp * l;
  wet_r += amp * r_out;
  ```
  (`r` is the row index in the active-row loop.)

- [ ] **Step 4: Update `lib.rs`**

In `multosis/src/lib.rs`:
- Add a persisted modulation field to `MultosisParams` (near `track_effects`):
  ```rust
      /// Per-track modulation configuration — persisted plugin state.
      #[persist = "track-modulation"]
      pub track_modulation: Arc<Mutex<[crate::modulation::TrackModulation; 16]>>,
  ```
  Initialise it in `impl Default for MultosisParams`:
  ```rust
              track_modulation: Arc::new(Mutex::new(std::array::from_fn(
                  crate::modulation::TrackModulation::default_for_row,
              ))),
  ```
- In the init code that bridges `track_effects` into the engine (`if let Ok(cfg) = self.params.track_effects.lock() { self.engine.set_effects(&cfg); }`), add the modulation bridge right after it:
  ```rust
          if let Ok(m) = self.params.track_modulation.lock() {
              self.engine.set_modulation(&m);
          }
  ```
- In `process()`, add `bpm` to the `engine.process` call (the local `bpm` already exists from `transport.tempo`): the new call is
  `self.engine.process(&mut *left, &mut *right, playing, sps, bpm, mix, auto_restart, &self.grid)`.

- [ ] **Step 5: Verify**

Run: `cargo nextest run -p multosis --lib -E 'test(engine_applies_modulation)'` — PASS.
Run: `cargo build -p multosis` — compiles, NO warnings.
Run: `cargo nextest run -p multosis` — PASS, 169 tests (156 + 3 + 5 + 4 from Tasks 1–3, + 1 new engine test).
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean (run `cargo fmt -p multosis` if not, before committing).

- [ ] **Step 6: Commit**

```bash
git add multosis/src/engine.rs multosis/src/lib.rs
git commit -m "feat(multosis): drive effects with the 3-MSEG modulation engine"
```

---

### Task 5: Verification

**Files:** none — checks and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS, 169 tests.
Run: `cargo nextest run --workspace` — PASS, all green (946 tests).
Run: `cargo clippy -p multosis -- -D warnings` — no warnings.
Run: `cargo fmt -p multosis -- --check` — clean (if a diff, `cargo fmt -p multosis` and commit it in Step 4).

- [ ] **Step 2: Release build and bundle**

Run: `cargo build --bin multosis --release` — the standalone binary builds.
Run: `cargo nih-plug bundle multosis --release` — VST3 + CLAP bundle, no errors.

- [ ] **Step 3: Manual smoke test**

Run `cargo run --bin multosis` in a host (or standalone). Confirm:
- The sequencer plays; with the default per-track modulation, each lit row's effect parameter audibly drifts over time (the assignable MSEG sweeping cutoff / bit depth), at a different rate per row.
- Levels stay stable (the amplitude MSEG is flat by default).
- The sequencer, grid editor, toolbar, reset — all still work.

Report the smoke-test observations.

- [ ] **Step 4: Commit (only if Step 1 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for the modulation engine"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- Multosis has a 3-MSEG-per-track modulation engine: a flat-by-default amplitude MSEG scaling each row's output, and two assignable MSEGs modulating effect parameters around their base values; all free-running on Time/Beat clocks. `[TrackModulation; 16]` is persisted.
- `cargo nextest run -p multosis` is green (169 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles and plays with audible per-track modulation.

## Spec coverage check (self-review)

- §1 MSEG reused — `tiny-skia-widgets`'s `MsegData`, no new type (all tasks `use` it).
- §2 Per-track config — `TrackModulation` (Task 1); `[TrackModulation; 16]` persisted via `#[persist = "track-modulation"]` (Task 4 Step 4); a missing key defaults (nih-plug `#[persist]` behaviour); plain serde derive.
- §3 Modulation math — `assignable_value` (around-base, bipolar depth, clamped) and the amplitude MSEG value used directly as the row gain (Task 2 + the `update_block` `k == 0` branch in Task 3).
- §4 The clock — `mseg_phase_delta` (Time/Beat → phase delta) + `advance` free-running cyclic per block (Tasks 2–3).
- §5 Engine integration — `Modulation` owned by `AudioEngine`, `update_block` once per block, `process` gains `bpm`, `process_sample` scales by amplitude (Task 4); allocation-free, config bridged at init.
- §6 Defaults — `TrackModulation::default_for_row` (flat amplitude, `msegs[1]` a row-varied cyclic triangle assigned to param 0, `msegs[2]` unassigned) — Task 1.
- §7 Out of scope — no MSEG editor / tabbed shell / assignment UI (2c); no retriggering (Phase 3): none added.
- §8 Testing — modulation math, the clock, the runtime, engine application, `TrackModulation` serde + default (Tasks 1–4); smoke test (Task 5).

## Note on task sequencing

Tasks 1–3 build `modulation.rs` additively — a new module, the crate stays green throughout. Task 4 is the interlocked engine + plugin integration: it changes `AudioEngine::process`'s signature, so `lib.rs`'s call and the engine tests change together in one commit. Each task ends with a green build, green tests, and clean clippy.
