# Multosis Phase 1 — Milestone 1b-i Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the headless `multosis` library (Milestone 1a) into a working nih-plug audio plugin — an audible routing sequencer with automatable parameters and two throwaway effects — with no editor yet.

**Architecture:** The `multosis` crate gains the plugin layer. `handoff.rs` carries the `Grid` from a future GUI to the audio thread (the miff `KernelHandoff` pattern). `effects.rs` holds two hardwired throwaway effects (a per-row lowpass and a per-row bitcrush). `engine.rs` is the `AudioEngine` — it drives the `Propagator` + `StepClock` from Milestone 1a, applies the lit rows' effects to the dry input, and mixes. `lib.rs` gains the `Plugin` impl, parameters, and `process()`. The grid is fixed at `default_routing()` until Milestone 1b-ii adds the editor.

**Tech Stack:** Rust (nightly), nih-plug (fork `finish-vst3-pr`), `cargo nextest`. No GUI crates — that is Milestone 1b-ii.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` — §3.1 (params/state), §6 (audio engine + throwaway effects).

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** Every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state:** The `multosis` crate already has, fully tested (59 tests green): `grid` (`Direction`, `Cell`, `LoopRegion`, `Grid`, `next_cell`, `default_routing`/`reset_routing`/`reinit_activations`/`sanitize`, serde), `region`, `randomize`, `propagation` (`Wavefront`, `step_manual`, `Propagator`, `SequenceState`), and `clock` (`Speed`, `samples_per_step`, `StepClock`). `Grid` is `Copy` and implements `serde::Serialize`/`Deserialize`. `Grid::default()` == `Grid::default_routing()`.

---

### Task 1: Plugin crate manifest and standalone bin

**Files:**
- Modify: `multosis/Cargo.toml`
- Create: `multosis/src/main.rs`

- [ ] **Step 1: Rewrite the crate manifest**

Replace the entire contents of `multosis/Cargo.toml` with:

```toml
[package]
name = "multosis"
version = "0.1.0"
edition = "2021"
description = "A multi-FX routing sequencer"
license = "GPL-3.0-or-later"

[lib]
crate-type = ["cdylib", "lib"]

[[bin]]
name = "multosis"
path = "src/main.rs"

[dependencies]
nih_plug = { git = "https://github.com/xxx/nih-plug.git", branch = "finish-vst3-pr", features = ["standalone", "assert_process_allocs"] }
serde = { version = "1.0", features = ["derive"] }

[dev-dependencies]
serde_json = "1.0"

[package.metadata.bundler]
name = "Multosis"
company = "mpd"
description = "A multi-FX routing sequencer"
license = "GPL-3.0-or-later"
version = "0.1.0"
```

- [ ] **Step 2: Create the standalone bin**

Create `multosis/src/main.rs`:

```rust
use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<multosis::Multosis>();
}
```

- [ ] **Step 3: Verify the library still builds**

Run: `cargo build -p multosis --lib`
Expected: the library compiles cleanly. (The bin will NOT build yet — `multosis::Multosis` does not exist until Task 9. That is expected at this step.)

- [ ] **Step 4: Verify the existing tests still pass**

Run: `cargo nextest run -p multosis`
Expected: PASS — 59 tests (adding nih-plug as a dependency must not disturb the 1a modules).

- [ ] **Step 5: Commit**

```bash
git add multosis/Cargo.toml multosis/src/main.rs Cargo.lock
git commit -m "feat(multosis): add plugin crate manifest and standalone bin"
```

---

### Task 2: `GridHandoff` — GUI→audio handoff

**Files:**
- Create: `multosis/src/handoff.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/handoff.rs`:

```rust
//! Lock-free-ish GUI→audio handoff of the routing `Grid`.
//!
//! Mirrors miff's `KernelHandoff`: the GUI thread publishes with a blocking
//! lock; the audio thread reads with a non-blocking `try_lock` and keeps its
//! previous copy on contention. `Grid` is `Copy` (~1.5 KB), so a read is an
//! allocation-free stack copy.

use crate::grid::Grid;
use std::sync::Mutex;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_read_returns_the_initial_grid() {
        let handoff = GridHandoff::new(Grid::default());
        let g = handoff.try_read().expect("uncontended read");
        assert_eq!(g, Grid::default());
    }

    #[test]
    fn publish_then_read_sees_the_new_grid() {
        let handoff = GridHandoff::new(Grid::default());
        let mut edited = Grid::default();
        edited.cell_mut(4, 4).enabled = false;
        handoff.publish(edited);
        let g = handoff.try_read().expect("uncontended read");
        assert!(!g.cell(4, 4).enabled);
    }
}
```

Add `pub mod handoff;` to `multosis/src/lib.rs` (with the other `pub mod` lines).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis handoff`
Expected: build failure — `cannot find type GridHandoff`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/handoff.rs`, after the `use` lines (before the `#[cfg(test)]` module):

```rust
/// Carries the latest `Grid` from the GUI thread to the audio thread.
pub struct GridHandoff {
    shared: Mutex<Grid>,
}

impl GridHandoff {
    /// A handoff seeded with `grid`.
    pub fn new(grid: Grid) -> Self {
        Self {
            shared: Mutex::new(grid),
        }
    }

    /// GUI thread: publish a new grid. Blocks briefly on the lock.
    pub fn publish(&self, grid: Grid) {
        if let Ok(mut slot) = self.shared.lock() {
            *slot = grid;
        }
    }

    /// Audio thread: read the latest grid without blocking. Returns `None` on
    /// lock contention — the caller keeps its previous copy.
    pub fn try_read(&self) -> Option<Grid> {
        self.shared.try_lock().ok().map(|slot| *slot)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis handoff`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/handoff.rs multosis/src/lib.rs
git commit -m "feat(multosis): add GridHandoff GUI-to-audio handoff"
```

---

### Task 3: `Speed` as a host parameter, and the `EffectBank` enum

**Files:**
- Modify: `multosis/src/clock.rs`
- Create: `multosis/src/effects.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/effects.rs`:

```rust
//! The two throwaway effects for Milestone 1b — a per-row lowpass and a
//! per-row bitcrush. Hardwired, with no shared abstraction; the standardised
//! effect trait is Phase 2. Each effect's character is mapped from the row
//! index so the wavefront's vertical motion is immediately audible.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.1.

use nih_plug::prelude::Enum;

/// Which throwaway effect every row uses. A host parameter.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum EffectBank {
    #[id = "lowpass"]
    #[name = "Lowpass"]
    Lowpass,
    #[id = "bitcrush"]
    #[name = "Bitcrush"]
    Bitcrush,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_bank_variants_distinct() {
        assert_ne!(EffectBank::Lowpass, EffectBank::Bitcrush);
    }
}
```

Add `pub mod effects;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis effect_bank`
Expected: build failure — `effects` module / `EffectBank` not found until the module is wired in; once wired, it should compile and pass. If it already compiles and passes after Step 1, that is acceptable — `EffectBank` is a plain enum. Proceed.

- [ ] **Step 3: Make `Speed` a host parameter**

In `multosis/src/clock.rs`, change the `Speed` enum's derive and add per-variant attributes so it can back an `EnumParam`. Replace the existing `Speed` enum definition (the `#[derive(...)] pub enum Speed { ... }` block) with:

```rust
/// How fast the wavefront advances — a musical note division. Backs the
/// plugin's `speed` parameter, so it derives nih-plug's `Enum`.
#[derive(nih_plug::prelude::Enum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Speed {
    /// 1/32 note.
    #[id = "div32"]
    #[name = "1/32"]
    Div32,
    /// 1/16 note.
    #[id = "div16"]
    #[name = "1/16"]
    Div16,
    /// 1/8 note.
    #[id = "div8"]
    #[name = "1/8"]
    Div8,
    /// 1/4 note.
    #[id = "div4"]
    #[name = "1/4"]
    Div4,
    /// 1/2 note.
    #[id = "div2"]
    #[name = "1/2"]
    Div2,
    /// Whole note.
    #[id = "div1"]
    #[name = "1/1"]
    Div1,
}
```

Leave the `impl Speed` block (`ALL`, `quarter_notes`) and `samples_per_step` and `StepClock` exactly as they are.

- [ ] **Step 4: Run tests to verify everything passes**

Run: `cargo nextest run -p multosis`
Expected: PASS — 60 tests (the 59 from before, plus `effect_bank_variants_distinct`). The existing `clock` tests (`speed_*`, `samples_per_step_*`, `step_clock_*`) must still pass — the `Enum` derive and the attributes do not change `Speed`'s values.

Run: `cargo build -p multosis --lib`
Expected: clean, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/clock.rs multosis/src/effects.rs multosis/src/lib.rs
git commit -m "feat(multosis): add EffectBank enum and make Speed a host param"
```

---

### Task 4: `LowpassBank` — the per-row lowpass effect

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/effects.rs`:

```rust
    #[test]
    fn lowpass_open_row_passes_a_constant() {
        // The brightest row (ROWS-1) has a very high cutoff; a constant input
        // settles to ~itself within a few samples.
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        let row = crate::grid::ROWS - 1;
        let mut y = 0.0;
        for _ in 0..256 {
            y = lp.process(row, 0, 1.0);
        }
        assert!(y > 0.9, "open row should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_dark_row_attenuates_alternating_signal() {
        // The darkest row (0) has a low cutoff; a fast ±1 alternation is
        // heavily attenuated relative to the open row.
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        let mut dark_peak = 0.0_f32;
        let mut open_peak = 0.0_f32;
        for i in 0..512 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            dark_peak = dark_peak.max(lp.process(0, 0, x).abs());
            open_peak = open_peak.max(lp.process(crate::grid::ROWS - 1, 0, x).abs());
        }
        assert!(
            dark_peak < open_peak,
            "dark row ({dark_peak}) should attenuate more than open ({open_peak})"
        );
    }

    #[test]
    fn lowpass_reset_clears_state() {
        let mut lp = LowpassBank::new();
        lp.set_sample_rate(48_000.0);
        for _ in 0..100 {
            lp.process(0, 0, 1.0);
        }
        lp.reset();
        // After reset the first output is just the first filtered step from 0.
        let y = lp.process(0, 0, 1.0);
        assert!(y < 0.5, "state should be cleared, got {y}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis lowpass`
Expected: build failure — `cannot find type LowpassBank`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs`, after the `EffectBank` enum (before the `#[cfg(test)]` module):

```rust
use crate::grid::ROWS;

/// A one-pole lowpass per row. Throwaway effect: the cutoff is mapped from the
/// row index — row 0 is darkest (~200 Hz), row `ROWS - 1` is open (~18 kHz) —
/// so the wavefront's vertical motion is audible. State is one running value
/// per (row, channel); it persists across steps so the filter does not click.
pub struct LowpassBank {
    /// Running output value per `[row][channel]` (2 channels).
    state: [[f32; 2]; ROWS],
    /// One-pole coefficient per row, set by `set_sample_rate`.
    coeff: [f32; ROWS],
}

impl LowpassBank {
    /// A bank with cleared state and zeroed coefficients. Call
    /// `set_sample_rate` before processing.
    pub fn new() -> Self {
        Self {
            state: [[0.0; 2]; ROWS],
            coeff: [0.0; ROWS],
        }
    }

    /// Recompute the per-row coefficients for `sample_rate` (Hz). Row 0 maps
    /// to ~200 Hz, row `ROWS - 1` to ~18 kHz, log-spaced in between.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(1.0);
        for r in 0..ROWS {
            let t = r as f32 / (ROWS - 1) as f32;
            let cutoff = 200.0 * (18_000.0_f32 / 200.0).powf(t);
            let alpha = 1.0 - (-2.0 * std::f32::consts::PI * cutoff / sr).exp();
            self.coeff[r] = alpha.clamp(0.0, 1.0);
        }
    }

    /// Clear all filter state.
    pub fn reset(&mut self) {
        self.state = [[0.0; 2]; ROWS];
    }

    /// Process one sample of `channel` (0 or 1) for `row`.
    pub fn process(&mut self, row: usize, channel: usize, x: f32) -> f32 {
        let a = self.coeff[row];
        let prev = self.state[row][channel];
        let y = prev + a * (x - prev);
        self.state[row][channel] = y;
        y
    }
}

impl Default for LowpassBank {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis lowpass`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): add LowpassBank throwaway effect"
```

---

### Task 5: `BitcrushBank` — the per-row bitcrush effect

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `multosis/src/effects.rs`:

```rust
    #[test]
    fn bitcrush_dark_row_quantizes_coarsely() {
        // Row 0 (~2 bits) snaps a small value hard; row ROWS-1 (~16 bits)
        // barely moves it.
        let bc = BitcrushBank::new();
        let x = 0.1_f32;
        let crushed = bc.process(0, x);
        let clean = bc.process(crate::grid::ROWS - 1, x);
        assert!(
            (crushed - x).abs() > (clean - x).abs(),
            "dark row should distort more (crushed={crushed}, clean={clean})"
        );
    }

    #[test]
    fn bitcrush_is_bounded() {
        // Quantisation never blows the signal far past its input range.
        let bc = BitcrushBank::new();
        for r in 0..crate::grid::ROWS {
            for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
                let y = bc.process(r, x);
                assert!(y.abs() <= 1.5, "row {r}, x {x} -> {y} out of range");
            }
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis bitcrush`
Expected: build failure — `cannot find type BitcrushBank`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs`, after the `LowpassBank` code (before the `#[cfg(test)]` module):

```rust
/// Per-row bit-depth reduction. Throwaway effect: the bit depth is mapped from
/// the row index — row 0 is heavily crushed (~2 bits), row `ROWS - 1` is
/// nearly clean (~16 bits). Stateless — bit quantisation has no memory.
pub struct BitcrushBank {
    /// Quantisation step per row.
    step: [f32; ROWS],
}

impl BitcrushBank {
    /// A bank with per-row quantisation steps computed.
    pub fn new() -> Self {
        let mut step = [0.0; ROWS];
        for (r, s) in step.iter_mut().enumerate() {
            let t = r as f32 / (ROWS - 1) as f32;
            let bits = 2.0 + t * 14.0; // 2..16 bits
            *s = 2.0_f32.powf(1.0 - bits);
        }
        Self { step }
    }

    /// Process one sample for `row`. Stateless; both channels use this.
    pub fn process(&self, row: usize, x: f32) -> f32 {
        let s = self.step[row];
        (x / s).round() * s
    }
}

impl Default for BitcrushBank {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis bitcrush`
Expected: PASS — 2 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): add BitcrushBank throwaway effect"
```

---

### Task 6: `AudioEngine` scaffold and `active_rows`

**Files:**
- Create: `multosis/src/engine.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/engine.rs`:

```rust
//! The audio engine: drives the Milestone 1a `Propagator` + `StepClock`,
//! applies the lit rows' throwaway effects to the dry input, and mixes.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.

use crate::clock::StepClock;
use crate::effects::{BitcrushBank, EffectBank, LowpassBank};
use crate::grid::{Grid, COLS, ROWS};
use crate::propagation::{Propagator, Wavefront};

/// Upper bound on step boundaries handled within one process block. At any
/// realistic tempo/speed/sample-rate, `samples_per_step` is at least a few
/// hundred samples, so a process block crosses at most a handful of
/// boundaries; 64 is a generous ceiling. Extra boundaries in a pathologically
/// large block are simply dropped (graceful, never panics).
const MAX_BOUNDARIES: usize = 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_rows_marks_lit_enabled_cells() {
        let grid = Grid::default_routing();
        let mut wf = Wavefront::empty();
        wf.set(3, 0, true);
        wf.set(9, 5, true);
        let mask = AudioEngine::active_rows(&grid, &wf);
        assert_eq!(mask, (1 << 3) | (1 << 9));
    }

    #[test]
    fn active_rows_ignores_disabled_cells() {
        let mut grid = Grid::default_routing();
        grid.cell_mut(7, 2).enabled = false;
        let mut wf = Wavefront::empty();
        wf.set(7, 2, true); // lit but disabled -> not active
        assert_eq!(AudioEngine::active_rows(&grid, &wf), 0);
    }

    #[test]
    fn active_rows_dedupes_a_row_with_two_lit_cells() {
        let grid = Grid::default_routing();
        let mut wf = Wavefront::empty();
        wf.set(4, 1, true);
        wf.set(4, 30, true); // same row, twice -> one bit
        assert_eq!(AudioEngine::active_rows(&grid, &wf), 1 << 4);
    }

    #[test]
    fn new_engine_has_an_empty_wavefront() {
        let engine = AudioEngine::new();
        assert!(engine.wavefront().is_empty());
    }
}
```

Add `pub mod engine;` to `multosis/src/lib.rs`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis active_rows`
Expected: build failure — `cannot find type AudioEngine`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/engine.rs`, after the `MAX_BOUNDARIES` const (before the `#[cfg(test)]` module):

```rust
/// Ties the propagation engine, step clock, and throwaway effects into a
/// stereo block processor.
pub struct AudioEngine {
    propagator: Propagator,
    clock: StepClock,
    lowpass: LowpassBank,
    bitcrush: BitcrushBank,
}

impl AudioEngine {
    /// A fresh engine: `Initial` propagator, zeroed clock, default effects.
    pub fn new() -> Self {
        Self {
            propagator: Propagator::new(),
            clock: StepClock::new(),
            lowpass: LowpassBank::new(),
            bitcrush: BitcrushBank::new(),
        }
    }

    /// Recompute sample-rate-dependent effect coefficients.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.lowpass.set_sample_rate(sample_rate);
    }

    /// Reset the sequence, clock, and filter state. Called on the transport
    /// stopped→playing edge and on the host's `reset()`.
    pub fn reset(&mut self) {
        self.propagator.reset();
        self.clock.reset();
        self.lowpass.reset();
    }

    /// The current wavefront — exposed so the Milestone 1b-ii editor can draw
    /// it.
    pub fn wavefront(&self) -> &Wavefront {
        &self.propagator.wavefront
    }

    /// Bitmask (bit `R`) of rows holding at least one cell that is both lit in
    /// `wf` and `enabled` in `grid`. A disabled lit cell contributes nothing;
    /// two lit cells in one row collapse to a single bit.
    fn active_rows(grid: &Grid, wf: &Wavefront) -> u16 {
        let mut mask = 0u16;
        for r in 0..ROWS {
            for c in 0..COLS {
                if wf.is_lit(r, c) && grid.cell(r, c).enabled {
                    mask |= 1 << r;
                    break;
                }
            }
        }
        mask
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis -E 'test(active_rows) + test(new_engine)'`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/engine.rs multosis/src/lib.rs
git commit -m "feat(multosis): add AudioEngine scaffold and active_rows"
```

---

### Task 7: `AudioEngine::process` — the block processor

**Files:**
- Modify: `multosis/src/engine.rs`

- [ ] **Step 1: Write the failing test**

Add these four test functions INSIDE the existing `#[cfg(test)] mod tests` block in `multosis/src/engine.rs`:

```rust
    #[test]
    fn process_at_mix_zero_is_dry_passthrough() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.1_f32; 64];
        let mut right = [-0.2_f32; 64];
        engine.process(
            &mut left, &mut right, true, 10.0, EffectBank::Lowpass, 0.0, true, &grid,
        );
        assert!(left.iter().all(|&s| (s - 0.1).abs() < 1e-6));
        assert!(right.iter().all(|&s| (s + 0.2).abs() < 1e-6));
    }

    #[test]
    fn process_empty_wavefront_full_wet_is_silent() {
        // Not playing, fresh engine: the wavefront is empty, so no row is
        // active and the fully-wet output is silence.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.5_f32; 64];
        let mut right = [0.5_f32; 64];
        engine.process(
            &mut left, &mut right, false, 10.0, EffectBank::Lowpass, 1.0, true, &grid,
        );
        assert!(left.iter().all(|&s| s == 0.0));
        assert!(right.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn process_arms_and_produces_wet_signal() {
        // Playing, default grid, a short step: once the clock crosses its
        // first boundary the start cells arm, rows become active, and the
        // fully-wet output is no longer silent.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut left = [0.3_f32; 128];
        let mut right = [0.3_f32; 128];
        // samples_per_step = 10 -> first boundary at sample 10 arms the
        // start cells; samples after that see active rows.
        engine.process(
            &mut left, &mut right, true, 10.0, EffectBank::Lowpass, 1.0, true, &grid,
        );
        assert!(
            left[127] != 0.0,
            "after arming, a fully-wet block should not be silent"
        );
    }

    #[test]
    fn process_reset_returns_to_silence() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let grid = Grid::default_routing();
        let mut buf = [0.3_f32; 128];
        let mut buf2 = [0.3_f32; 128];
        engine.process(
            &mut buf, &mut buf2, true, 10.0, EffectBank::Lowpass, 1.0, true, &grid,
        );
        engine.reset();
        // After reset, not playing: empty wavefront -> silent at full wet.
        let mut left = [0.4_f32; 64];
        let mut right = [0.4_f32; 64];
        engine.process(
            &mut left, &mut right, false, 10.0, EffectBank::Lowpass, 1.0, true, &grid,
        );
        assert!(left.iter().all(|&s| s == 0.0));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis process_`
Expected: build failure — `no method named process`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/engine.rs`, inside the `impl AudioEngine` block (after `active_rows`):

```rust
    /// Apply the active rows' effects to one dry stereo sample and sum them.
    fn process_sample(&mut self, dry_l: f32, dry_r: f32, active: u16, bank: EffectBank)
        -> (f32, f32)
    {
        let mut wet_l = 0.0;
        let mut wet_r = 0.0;
        for r in 0..ROWS {
            if active & (1 << r) == 0 {
                continue;
            }
            match bank {
                EffectBank::Lowpass => {
                    wet_l += self.lowpass.process(r, 0, dry_l);
                    wet_r += self.lowpass.process(r, 1, dry_r);
                }
                EffectBank::Bitcrush => {
                    wet_l += self.bitcrush.process(r, dry_l);
                    wet_r += self.bitcrush.process(r, dry_r);
                }
            }
        }
        (wet_l, wet_r)
    }

    /// Process one stereo block in place. `left`/`right` carry the dry input
    /// on entry and the mixed `dry + (wet - dry) * mix` output on return.
    ///
    /// While `playing`, the clock advances and the wavefront propagates at
    /// each step boundary; while stopped, the wavefront is frozen and the
    /// block is processed with the current lit set. `samples_per_step` comes
    /// from `clock::samples_per_step`.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        samples_per_step: f64,
        bank: EffectBank,
        mix: f32,
        auto_restart: bool,
        grid: &Grid,
    ) {
        let n = left.len().min(right.len());

        // Gather this block's step-boundary offsets (only while playing).
        let mut boundaries = [0usize; MAX_BOUNDARIES];
        let mut n_boundaries = 0usize;
        if playing {
            self.clock.advance(n, samples_per_step, |offset| {
                if n_boundaries < MAX_BOUNDARIES {
                    boundaries[n_boundaries] = offset;
                    n_boundaries += 1;
                }
            });
        }

        // Walk the block in segments split at each boundary; the wavefront is
        // constant within a segment.
        let mut active = Self::active_rows(grid, &self.propagator.wavefront);
        let mut cursor = 0usize;
        let mut bi = 0usize;
        while cursor < n {
            let seg_end = if bi < n_boundaries {
                boundaries[bi].clamp(cursor, n)
            } else {
                n
            };
            for i in cursor..seg_end {
                let (dry_l, dry_r) = (left[i], right[i]);
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active, bank);
                left[i] = dry_l + (wet_l - dry_l) * mix;
                right[i] = dry_r + (wet_r - dry_r) * mix;
            }
            cursor = seg_end;
            if bi < n_boundaries {
                self.propagator.tick(grid, auto_restart);
                active = Self::active_rows(grid, &self.propagator.wavefront);
                bi += 1;
            }
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests (the 60 from Task 3 plus Task 4's 3, Task 5's 2, Task 6's 4, and Task 7's 4 — 69 total). Also run `cargo build -p multosis --lib` — clean, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/engine.rs
git commit -m "feat(multosis): add AudioEngine block processor"
```

---

### Task 8: `MultosisParams` and the `Multosis` plugin struct

**Files:**
- Modify: `multosis/src/lib.rs`

This task is plugin glue — verified by compilation, not by a unit test.

- [ ] **Step 1: Add the parameters and plugin struct**

In `multosis/src/lib.rs`, after the `pub mod` declarations, add:

```rust
use crate::clock::Speed;
use crate::effects::EffectBank;
use crate::engine::AudioEngine;
use crate::grid::Grid;
use crate::handoff::GridHandoff;
use nih_plug::prelude::*;
use std::sync::{Arc, Mutex};

/// The Multosis plugin's parameters and persisted state.
#[derive(Params)]
pub struct MultosisParams {
    /// The routing grid — persisted plugin state, edited by the GUI (Milestone
    /// 1b-ii). `Arc<Mutex<Grid>>` is nih-plug's `PersistentField` shape.
    #[persist = "grid"]
    pub grid: Arc<Mutex<Grid>>,

    /// Tempo-synced wavefront advance rate.
    #[id = "speed"]
    pub speed: EnumParam<Speed>,

    /// Dry↔wet blend.
    #[id = "mix"]
    pub mix: FloatParam,

    /// Post-mix output gain.
    #[id = "output_gain"]
    pub output_gain: FloatParam,

    /// Which throwaway effect every row uses.
    #[id = "effect_bank"]
    pub effect_bank: EnumParam<EffectBank>,

    /// When on, a dead-ended wavefront re-arms the start cells.
    #[id = "auto_restart"]
    pub auto_restart: BoolParam,
}

impl Default for MultosisParams {
    fn default() -> Self {
        Self {
            grid: Arc::new(Mutex::new(Grid::default())),
            speed: EnumParam::new("Speed", Speed::Div16),
            mix: FloatParam::new("Mix", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            output_gain: FloatParam::new(
                "Output",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-30.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-30.0, 12.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            effect_bank: EnumParam::new("Effect", EffectBank::Lowpass),
            auto_restart: BoolParam::new("Auto Restart", true),
        }
    }
}

/// The Multosis plugin.
pub struct Multosis {
    params: Arc<MultosisParams>,
    /// GUI→audio handoff of the grid (used by the Milestone 1b-ii editor).
    grid_handoff: Arc<GridHandoff>,
    /// The audio thread's working copy of the grid.
    grid: Grid,
    engine: AudioEngine,
    sample_rate: f32,
    /// Previous block's transport state, for stopped→playing edge detection.
    was_playing: bool,
}

impl Default for Multosis {
    fn default() -> Self {
        Self {
            params: Arc::new(MultosisParams::default()),
            grid_handoff: Arc::new(GridHandoff::new(Grid::default())),
            grid: Grid::default(),
            engine: AudioEngine::new(),
            sample_rate: 44_100.0,
            was_playing: false,
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p multosis --lib`
Expected: compiles. There will be a `dead_code` warning for `Multosis` (no `Plugin` impl yet) — that is expected and Task 9 resolves it. There must be no *errors*.

- [ ] **Step 3: Commit**

```bash
git add multosis/src/lib.rs
git commit -m "feat(multosis): add MultosisParams and plugin struct"
```

---

### Task 9: `impl Plugin for Multosis`

**Files:**
- Modify: `multosis/src/lib.rs`

Plugin glue — verified by compilation.

- [ ] **Step 1: Add the `Plugin` impl**

In `multosis/src/lib.rs`, after the `Multosis` struct and its `Default` impl, add:

```rust
impl Plugin for Multosis {
    const NAME: &'static str = "Multosis";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: std::num::NonZeroU32::new(2),
        main_output_channels: std::num::NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.engine.set_sample_rate(self.sample_rate);
        // Bridge the persisted grid (possibly just restored from project
        // state) into the audio thread's working copy and the handoff.
        if let Ok(grid) = self.params.grid.lock() {
            self.grid = *grid;
            self.grid_handoff.publish(*grid);
        }
        self.was_playing = false;
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
        self.was_playing = false;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        let playing = transport.playing;
        let bpm = transport.tempo.unwrap_or(120.0);

        // Reset the sequence on the transport stopped→playing edge.
        if playing && !self.was_playing {
            self.engine.reset();
        }
        self.was_playing = playing;

        // Pick up the latest grid (non-blocking; keep the last on a miss).
        if let Some(grid) = self.grid_handoff.try_read() {
            self.grid = grid;
        }

        let sps = crate::clock::samples_per_step(
            self.params.speed.value(),
            bpm,
            self.sample_rate as f64,
        );
        let bank = self.params.effect_bank.value();
        let mix = self.params.mix.value();
        let auto_restart = self.params.auto_restart.value();

        let n = buffer.samples();
        let channels = buffer.as_slice();
        let (first, rest) = channels.split_at_mut(1);
        let left = &mut first[0][..n];
        let right = &mut rest[0][..n];

        self.engine.process(
            &mut *left, &mut *right, playing, sps, bank, mix, auto_restart, &self.grid,
        );

        // Post-mix output gain (smoothed per sample).
        for i in 0..n {
            let gain = self.params.output_gain.smoothed.next();
            left[i] *= gain;
            right[i] *= gain;
        }

        ProcessStatus::Normal
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p multosis --lib`
Expected: compiles cleanly. A `dead_code` warning for `grid_handoff` being only written, never `try_read`-distinct, is NOT expected (it is read in `process`). There may be a warning that `Multosis` is unused until the export macros (Task 10) — acceptable; no errors.

- [ ] **Step 3: Commit**

```bash
git add multosis/src/lib.rs
git commit -m "feat(multosis): implement the Plugin trait"
```

---

### Task 10: CLAP/VST3 impls and export macros

**Files:**
- Modify: `multosis/src/lib.rs`

Plugin glue — verified by compilation.

- [ ] **Step 1: Add the format impls and exports**

Append to the end of `multosis/src/lib.rs`:

```rust
impl ClapPlugin for Multosis {
    const CLAP_ID: &'static str = "com.mpd.multosis";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A multi-FX routing sequencer");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Multosis {
    const VST3_CLASS_ID: [u8; 16] = *b"MultosisMpdPlg\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx];
}

nih_export_clap!(Multosis);
nih_export_vst3!(Multosis);
```

- [ ] **Step 2: Verify the whole crate builds — library and bin**

Run: `cargo build -p multosis`
Expected: both the library and the `multosis` bin compile cleanly, with no warnings.

- [ ] **Step 3: Commit**

```bash
git add multosis/src/lib.rs
git commit -m "feat(multosis): add CLAP/VST3 impls and export macros"
```

---

### Task 11: Milestone 1b-i verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (~69: the 59 from Milestone 1a plus 1b-i's `effect_bank`, `lowpass` ×3, `bitcrush` ×2, `active_rows`/`new_engine` ×4, `process_` ×4).

- [ ] **Step 2: Lint and format**

Run: `cargo clippy -p multosis -- -D warnings`
Expected: no warnings.

Run: `cargo fmt -p multosis -- --check`
Expected: clean (exit 0). If it reports a diff, run `cargo fmt -p multosis` and include the change in the final commit.

- [ ] **Step 3: Release build and bundle**

Run: `cargo build --bin multosis --release`
Expected: the standalone binary builds.

Run: `cargo nih-plug bundle multosis --release`
Expected: a VST3 + CLAP bundle is produced with no errors.

- [ ] **Step 4: Manual smoke test**

Run the standalone binary: `cargo run --bin multosis` (or run the release binary directly). Confirm:
- The plugin window opens (nih-plug's standalone wrapper; there is no custom editor — a generic parameter view is expected).
- With audio playing through it and the host transport running, the output is audibly gated/filtered by the sequencer (the default routing sweeps a wavefront left-to-right across the 16 rows).
- Moving the `Mix` parameter to 0% returns the dry signal; `Speed` changes the sweep rate; `Effect` switches between the lowpass and bitcrush character.

Report the smoke-test observations. (This step is a human/aural check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-i"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-i — definition of done

- `multosis` is a real nih-plug plugin: `cargo nih-plug bundle multosis --release` produces a VST3 + CLAP bundle, and `cargo build --bin multosis --release` produces a standalone.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean.
- Loaded in a host, it audibly sequences the dry input through the default routing, with automatable `speed` / `mix` / `output_gain` / `effect_bank` / `auto_restart` parameters and a persisted `grid`.
- No editor yet — the grid is fixed at `default_routing()`. The `GridHandoff` seam and `MultosisParams.grid` persisted field are in place so Milestone 1b-ii's editor drops in without reworking the audio path.

## Spec coverage check (self-review)

- §3.1 params: `speed`/`mix`/`output_gain`/`effect_bank`/`auto_restart` — Tasks 8. `grid` as `#[persist]` `Arc<Mutex<Grid>>` — Task 8. The `Mutex`+`try_lock` handoff — Task 2, wired in Task 9.
- §6 audio engine: `try_read` the grid, advance the clock, sub-block at step boundaries, active-row dedup (lit AND enabled), parallel sum of effects, `lerp(dry, wet, mix)`, then `output_gain` — Tasks 6, 7, 9.
- §6.1 throwaway effects: per-row lowpass and bitcrush, character mapped to row index, `effect_bank` selector — Tasks 3, 4, 5.
- §5.2 clock: tempo-synced `samples_per_step`, advance only while playing, reset on the stopped→playing edge — Task 9 (`process`) using Task 7's engine.
- Out of scope (Milestone 1b-ii): the grid editor UI. The audio→GUI wavefront publishing is deferred — `AudioEngine::wavefront()` is exposed now as the read seam, but no atomic display mirror is built until the editor needs it.
