# Sample-Accurate Cell-Light Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fire the multosis `CellLight` modulation trigger at the exact step-boundary sample instead of up to one process-block late.

**Architecture:** `Modulation::update_block` (one monolithic per-block call) is split into `begin_block` (block-rate FreeHz setup), `advance_segment` (advance the per-MSEG-clock rows for one segment), and `fire` (reset a `CellLight` row's phases). The engine's existing step-boundary segment loop drives them: `begin_block` once, `advance_segment` per segment, `fire` at each boundary. The `AudioEngine::pending_cell_lights` cross-block buffer is deleted.

**Tech Stack:** Rust (nightly), `multosis` crate (a nih-plug plugin) in the `tract-plugin-pack` Cargo workspace. `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-05-21-multosis-sample-accurate-cell-light-design.md`

**Conventions:**
- Run all `cargo`/`git` from the workspace root `/home/mpd/git-sources/tract-plugin-pack`. Branch: `multosis`.
- Build/test/lint just this crate: `cargo build -p multosis`, `cargo nextest run -p multosis`, `cargo clippy -p multosis -- -D warnings`, `cargo fmt --check`.
- Never use `#[allow(...)]` to silence a warning.
- Commit message trailer MUST be exactly: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real build/test.

## File structure

- `multosis/src/modulation.rs` — `Modulation`: replace `update_block` with `begin_block` + `advance_segment` + `fire`, plus a private `apply_mseg` helper. Migrate the unit tests.
- `multosis/src/engine.rs` — `AudioEngine::process` restructured to drive the modulation per-segment; the `pending_cell_lights` field deleted. Cell-light integration tests updated.

No other files change. No new parameters, no UI change.

## Background — the current code

`Modulation` (`modulation.rs`) holds `config: [TrackModulation; ROWS]`, per-row per-MSEG `phases: [[f32; 3]; ROWS]`, the FreeHz clock `hz_phases: [f32; ROWS]`, `amplitudes: [f32; ROWS]`, and `fires: u16`. `ROWS` is 16.

`update_block` today: decides each row's fire (`Free` → never; `CellLight` → a bit in the passed `cell_light_events` mask; `FreeHz` → advance `hz_phases` by the whole block, fire on a wrap past 1.0), resets firing rows' phases, then for every row advances its three MSEGs by the whole block, evaluates each, and applies it — `msegs[0]` (amplitude) writes `amplitudes[row]`, an assigned `msegs[1]`/`msegs[2]` writes its target effect parameter via `set_param`. `Free` and `CellLight` rows use the per-MSEG `phases` clocks; `FreeHz` rows sweep all three MSEGs at the shared `hz_phases` value.

`AudioEngine::process` (`engine.rs`) calls `update_block` once at the top with the *previous* block's accumulated `pending_cell_lights`, then walks the block in segments split at step boundaries; at each boundary it computes `newly = after & !before` and ORs it into `self.pending_cell_lights` for the next block.

---

## Task 1: Add the segment-aware `Modulation` API

Add `begin_block`, `advance_segment`, `fire`, and a private `apply_mseg` helper to `Modulation`. Reimplement `update_block` as a thin wrapper over the three new methods so every existing caller and test keeps working unchanged. This task is purely additive — the workspace compiles and every existing test passes throughout.

**Files:**
- Modify: `multosis/src/modulation.rs`

- [ ] **Step 1: Write the failing unit tests for the new API**

In `multosis/src/modulation.rs`, inside the existing `#[cfg(test)] mod tests` block (anywhere among the other tests, e.g. just before the closing `}` at line 889), add:

```rust
    #[test]
    fn begin_block_zeroes_fires_and_decides_free_hz() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[5].trigger = TriggerSource::FreeHz { hz: 10.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // 10 Hz, 48 kHz, a 4800-sample block = exactly one cycle → one fire.
        m.begin_block(4800, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 1 << 5);
        // begin_block zeroes `fires` each call: a block with no wrap clears it.
        m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(m.fires_last_block(), 0);
    }

    #[test]
    fn fire_resets_cell_light_rows_and_ignores_other_triggers() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[2].trigger = TriggerSource::CellLight;
        cfg[3].trigger = TriggerSource::Free;
        cfg[4].trigger = TriggerSource::FreeHz { hz: 1.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift the per-MSEG-clock rows' phases away from 0.
        for _ in 0..50 {
            m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(3, 0) > 1e-6, "Free row drifted");
        // Fire rows 2, 3, 4 — only the CellLight row (2) resets and reports.
        m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        m.fire((1 << 2) | (1 << 3) | (1 << 4));
        assert_eq!(m.fires_last_block() & (1 << 2), 1 << 2, "CellLight row fired");
        assert_eq!(m.fires_last_block() & (1 << 3), 0, "Free row did not fire");
        assert_eq!(m.phase_for_test(2, 0), 0.0, "CellLight row phases reset");
        assert!(
            m.phase_for_test(3, 0) > 1e-6,
            "fire must not touch a Free row's phase"
        );
    }

    #[test]
    fn advance_segment_skips_free_hz_rows() {
        // FreeHz rows are advanced by begin_block; advance_segment leaves them.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[0].trigger = TriggerSource::FreeHz { hz: 5.0 };
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_begin = m.phase_for_test(0, 0);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(
            m.phase_for_test(0, 0),
            after_begin,
            "advance_segment must leave a FreeHz row's phase as begin_block set it"
        );
    }

    #[test]
    fn advance_segment_zero_length_is_a_noop() {
        let mut m = Modulation::new();
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let phase = m.phase_for_test(3, 1);
        m.advance_segment(0, 120.0, 48_000.0, &mut effects, &track_effects);
        assert_eq!(
            m.phase_for_test(3, 1),
            phase,
            "a zero-length segment must not advance phases"
        );
    }

    #[test]
    fn advance_segment_in_two_halves_around_a_fire_resets_at_the_split() {
        // Advancing a 256-sample block as [100 samples][fire][156 samples]
        // must leave a CellLight row's phase equal to a from-0 advance over
        // only the 156-sample tail — proving the reset lands at the split,
        // not a block late.
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[1].trigger = TriggerSource::CellLight;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 1's phases well away from 0.
        for _ in 0..30 {
            m.begin_block(256, 120.0, 48_000.0, &mut effects, &track_effects);
            m.advance_segment(256, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        // The block: a 100-sample segment, fire row 1, then a 156-sample segment.
        m.begin_block(256, 120.0, 48_000.0, &mut effects, &track_effects);
        m.advance_segment(100, 120.0, 48_000.0, &mut effects, &track_effects);
        m.fire(1 << 1);
        m.advance_segment(156, 120.0, 48_000.0, &mut effects, &track_effects);
        let after_fire = m.phase_for_test(1, 1);
        // Expected: a from-0 advance over just the 156-sample tail.
        let mseg = cfg[1].msegs[1];
        let dt = mseg_phase_delta(&mseg, 156, 120.0, 48_000.0);
        let (expected, _) = advance(&mseg, 0.0, dt, false);
        assert!(
            (after_fire - expected).abs() < 1e-6,
            "post-fire phase {after_fire} should equal a from-0 tail advance {expected}"
        );
    }
```

- [ ] **Step 2: Run the new tests to verify they fail**

Run: `cargo nextest run -p multosis`
Expected: a **compile error** — `begin_block`, `advance_segment`, and `fire` do not exist yet (`no method named ... found`).

- [ ] **Step 3: Add the new methods and reimplement `update_block` as a wrapper**

In `multosis/src/modulation.rs`, replace the entire `update_block` method (it begins with the doc comment `/// Advance every MSEG one process block, evaluate it, and apply:` at line 242 and ends at its closing `}` at line 331) with the following five items:

```rust
    /// Block-rate modulation setup, run once at the top of a process block.
    /// Zeroes `fires`, then for every `FreeHz` row advances its oscillator by
    /// the whole block, decides its fire, and evaluates and applies its three
    /// MSEGs at the oscillator phase. `Free` and `CellLight` rows use the
    /// per-MSEG `phases` clocks and are advanced by `advance_segment`; this
    /// method does not touch them.
    pub fn begin_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        self.fires = 0;
        for row in 0..ROWS {
            let TriggerSource::FreeHz { hz } = self.config[row].trigger else {
                continue;
            };
            // Advance the free oscillator by the whole block; a wrap past 1.0
            // fires the row. The fractional remainder is retained, so multiple
            // wraps in one block still count as a single fire.
            if hz > 0.0 {
                self.hz_phases[row] += (block_len as f32 * hz) / sample_rate as f32;
                if self.hz_phases[row] >= 1.0 {
                    self.hz_phases[row] -= self.hz_phases[row].floor();
                    self.fires |= 1 << row;
                }
            }
            // FreeHz tracks sweep all three MSEGs in lockstep at the
            // oscillator phase, ignoring each MSEG's own sync/length.
            let phase = self.hz_phases[row];
            for k in 0..3 {
                self.phases[row][k] = phase;
                self.apply_mseg(row, k, effects, track_effects);
            }
        }
    }

    /// Advance every `Free` and `CellLight` row's three MSEGs by one segment
    /// of `seg_len` samples, then evaluate and apply them. Called once per
    /// segment from the engine's step-boundary segment loop. `FreeHz` rows are
    /// handled by `begin_block` and skipped here. A zero-length segment is a
    /// no-op.
    pub fn advance_segment(
        &mut self,
        seg_len: usize,
        bpm: f64,
        sample_rate: f64,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        if seg_len == 0 {
            return;
        }
        for row in 0..ROWS {
            if matches!(self.config[row].trigger, TriggerSource::FreeHz { .. }) {
                continue;
            }
            for k in 0..3 {
                let mseg = self.config[row].msegs[k];
                let dt = mseg_phase_delta(&mseg, seg_len, bpm, sample_rate);
                let (next, _finished) = advance(&mseg, self.phases[row][k], dt, false);
                self.phases[row][k] = next;
                self.apply_mseg(row, k, effects, track_effects);
            }
        }
    }

    /// Fire the `CellLight` trigger for the rows flagged in `newly_rows` (bit
    /// `r` set = row `r` had a lit, enabled cell first appear under the
    /// playhead). A firing row's three MSEG phases reset to 0 and its `fires`
    /// bit is set. Rows whose trigger is not `CellLight` are ignored. Called
    /// at a step boundary, so the reset takes effect on the very next segment.
    pub fn fire(&mut self, newly_rows: u16) {
        for row in 0..ROWS {
            if newly_rows & (1 << row) != 0
                && matches!(self.config[row].trigger, TriggerSource::CellLight)
            {
                self.phases[row] = [0.0; 3];
                self.fires |= 1 << row;
            }
        }
    }

    /// Evaluate MSEG `k` on `row` at its current phase and apply it: the
    /// amplitude MSEG (`k == 0`) sets `amplitudes[row]`; an assignable MSEG
    /// with a target writes that effect parameter via `set_param`.
    fn apply_mseg(
        &mut self,
        row: usize,
        k: usize,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        // `MsegData` is `Copy`; copy what we need so the immutable
        // `self.config` borrow does not span the `self.amplitudes` / `effects`
        // writes below.
        let mseg = self.config[row].msegs[k];
        let value = value_at_phase(&mseg, self.phases[row][k]);
        if k == 0 {
            // Amplitude MSEG.
            self.amplitudes[row] = value;
        } else if let Some(target) = self.config[row].targets[k - 1] {
            // Assignable MSEG -> a target effect parameter.
            if let Some(&spec) = effects[row].parameters().get(target) {
                let base = track_effects[row].params[target];
                let depth = self.config[row].depths[k - 1];
                effects[row].set_param(
                    target,
                    assignable_value(value, base, depth, spec, mseg.polarity),
                );
            }
        }
    }

    /// Advance every MSEG one process block and apply. Thin wrapper over
    /// `begin_block` + `fire` + `advance_segment`, kept while the engine
    /// migrates to driving the modulation per-segment.
    pub fn update_block(
        &mut self,
        block_len: usize,
        bpm: f64,
        sample_rate: f64,
        cell_light_events: u16,
        effects: &mut [EffectInstance; ROWS],
        track_effects: &[TrackEffect; ROWS],
    ) {
        self.begin_block(block_len, bpm, sample_rate, effects, track_effects);
        self.fire(cell_light_events);
        self.advance_segment(block_len, bpm, sample_rate, effects, track_effects);
    }
```

This is behaviour-preserving for `update_block`: `begin_block` does the FreeHz rows and zeroes `fires`; `fire` resets the `CellLight` rows named in the mask; `advance_segment` advances the `Free`/`CellLight` rows by the whole block. A `CellLight` row that fired is reset by `fire` then advanced from 0 by `advance_segment` — exactly as the old monolithic body did.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis`
Expected: PASS — the five new tests pass and every pre-existing test (which still calls `update_block`) is unaffected.

- [ ] **Step 5: Build, lint, format**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt --check`
Expected: clean, no warnings. (If `cargo fmt --check` reports drift, run `cargo fmt` and re-run.)

- [ ] **Step 6: Commit**

```bash
git add multosis/src/modulation.rs
git commit -m "$(cat <<'EOF'
feat(multosis): segment-aware modulation API

Split Modulation::update_block into begin_block (block-rate FreeHz
setup), advance_segment (advance the per-MSEG-clock rows for one
segment), and fire (reset a CellLight row's phases). update_block
becomes a thin wrapper over the three, so every existing caller is
unchanged; the engine adopts the per-segment API next.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Drive the modulation per-segment from the engine

Restructure `AudioEngine::process` to call `begin_block` once, `advance_segment` per segment, and `fire` at each step boundary — so a `CellLight` edge resets the row's phases in the same block, at the boundary sample. Delete `AudioEngine::pending_cell_lights` and `Modulation::update_block`, and migrate the modulation unit tests to the new API.

**Files:**
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/modulation.rs`

- [ ] **Step 1: Rewrite the engine cell-light tests for same-block firing**

In `multosis/src/engine.rs`, in the `#[cfg(test)] mod tests` block, replace the entire `cell_light_trigger_fires_on_the_sequences_first_step` test (its `#[test]` line through its closing `}`, lines 440-474) with these two tests:

```rust
    #[test]
    fn cell_light_trigger_fires_on_the_sequences_first_step() {
        // The cell-light edge detector diffs the active-row mask before/after
        // each playhead tick. On the FIRST tick the unstarted playhead reports
        // column 0 and `tick()` snaps it to `col0`; the `Playhead::started()`
        // gate makes the pre-start `before` set empty so the opening step's
        // CellLight event is not suppressed. With per-segment firing the
        // trigger fires in the SAME block the cell lights — no one-block lag.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[5].trigger = crate::modulation::TriggerSource::CellLight;
        engine.set_modulation(&mod_cfg);
        // The default grid has every cell enabled and a full loop region, so
        // row 5's cell at the opening column (col0 == 0) is lit.
        let grid = Grid::default();
        // One block: the playhead starts, row 5's cell lights on the opening
        // step, and its CellLight trigger fires within this same block.
        let mut l1 = [0.0_f32; 64];
        let mut r1 = [0.0_f32; 64];
        engine.process(&mut l1, &mut r1, true, 1000.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 5) != 0,
            "row 5's CellLight trigger should fire in the same block the cell lights"
        );
    }

    #[test]
    fn cell_light_fires_same_block_for_a_mid_block_step() {
        // A row that only becomes active several columns in still fires in the
        // block whose segment loop crosses that step boundary — not a block
        // later. A small samples_per_step makes one block sweep several
        // columns.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[8].trigger = crate::modulation::TriggerSource::CellLight;
        engine.set_modulation(&mod_cfg);
        // Row 8 is disabled for columns 0..3 and enabled (the default) from
        // column 3 on — so it goes newly-active partway through the block,
        // not on the opening step.
        let mut grid = Grid::default();
        grid.cell_mut(8, 0).enabled = false;
        grid.cell_mut(8, 1).enabled = false;
        grid.cell_mut(8, 2).enabled = false;
        // samples_per_step 10 over a 128-sample block crosses ~12 boundaries,
        // so the playhead reaches column 3 well inside the block.
        let mut l = [0.0_f32; 128];
        let mut r = [0.0_f32; 128];
        engine.process(&mut l, &mut r, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 8) != 0,
            "row 8's CellLight trigger fires in the same block its cell first lights"
        );
    }
```

- [ ] **Step 2: Run the rewritten tests to verify they fail**

Run: `cargo nextest run -p multosis`
Expected: `cell_light_trigger_fires_on_the_sequences_first_step` and `cell_light_fires_same_block_for_a_mid_block_step` FAIL (every other test passes) — the current engine buffers the edge into `pending_cell_lights` and only consumes it the *next* block, so after a single `process()` call `modulation_fires_for_test()` does not yet show the fire.

- [ ] **Step 3: Restructure `engine.process()` and delete `pending_cell_lights`**

In `multosis/src/engine.rs`:

**3a.** Delete the `pending_cell_lights` field and its doc comment from the `AudioEngine` struct — remove these lines (36-40):

```rust
    /// Per-row "had a new lit-and-enabled cell light up at a step-boundary
    /// tick this block" mask. Accumulated within `process()` as each tick's
    /// post-tick set is diffed against its pre-tick set, then consumed by the
    /// next block's `Modulation::update_block` for the `CellLight` trigger.
    pending_cell_lights: u16,
```

**3b.** In `AudioEngine::new`, remove the line `pending_cell_lights: 0,` from the struct literal.

**3c.** In `AudioEngine::reset`, remove the line `self.pending_cell_lights = 0;`.

**3d.** Replace the entire `process` method (its doc comment beginning `/// Process one stereo block in place.` at line 189 through the method's closing `}` at line 277) with:

```rust
    /// Process one stereo block in place. `left`/`right` carry the dry input
    /// on entry and the mixed `dry + (wet - dry) * mix` output on return.
    ///
    /// While `playing`, the clock advances and the playhead advances at each
    /// step boundary; while stopped, the playhead is frozen and the block is
    /// processed at the current column. `samples_per_step` comes from
    /// `clock::samples_per_step`.
    ///
    /// The modulation is driven from the per-segment loop: `begin_block` once,
    /// `advance_segment` per segment, and `fire` at each step boundary — so a
    /// `CellLight` edge resets the row at the exact boundary sample.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        playing: bool,
        samples_per_step: f64,
        bpm: f64,
        mix: f32,
        grid: &Grid,
    ) {
        let n = left.len().min(right.len());
        let sr = self.sample_rate as f64;

        // Block-rate modulation setup: FreeHz fires, and zero the fire mask.
        // `begin_block` takes no `bpm` — FreeHz advancing is sample-rate-only.
        self.modulation
            .begin_block(n, sr, &mut self.effects, &self.track_effects);

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

        // Walk the block in segments split at each boundary. Per segment:
        // advance the per-MSEG-clock modulation, then render the audio. At a
        // boundary, fire any newly-lit CellLight rows so the phase reset lands
        // on the very next segment — sample-accurate, no cross-block delay.
        let mut active = active_rows(grid, &grid.loop_region, self.playhead.column());
        let mut cursor = 0usize;
        let mut bi = 0usize;
        while cursor < n {
            let seg_end = if bi < n_boundaries {
                boundaries[bi].clamp(cursor, n)
            } else {
                n
            };
            self.modulation.advance_segment(
                seg_end - cursor,
                bpm,
                sr,
                &mut self.effects,
                &self.track_effects,
            );
            for i in cursor..seg_end {
                let (dry_l, dry_r) = (left[i], right[i]);
                let (wet_l, wet_r) = self.process_sample(dry_l, dry_r, active);
                // Tame the parallel-row sum on the wet bus, then mix dry/wet.
                let (cw_l, cw_r) = self.compressor.process_sample(wet_l, wet_r);
                left[i] = dry_l + (cw_l - dry_l) * mix;
                right[i] = dry_r + (cw_r - dry_r) * mix;
            }
            cursor = seg_end;
            if bi < n_boundaries {
                // Snapshot the active-row mask BEFORE and AFTER the tick;
                // rows that became active fire their CellLight trigger now.
                // Before the first tick the playhead has not started — nothing
                // was playing, so the pre-tick set is empty. Without this gate
                // an unstarted playhead reports column 0 and `tick()` snaps it
                // to `col0`; for a `col0 == 0` loop zone `before == after` and
                // the opening step's CellLight events would be suppressed.
                let before = if self.playhead.started() {
                    active_rows(grid, &grid.loop_region, self.playhead.column())
                } else {
                    0
                };
                self.playhead.tick(&grid.loop_region);
                self.step += 1;
                let after = active_rows(grid, &grid.loop_region, self.playhead.column());
                let newly = after & !before;
                self.modulation.fire(newly);
                active = after;
                bi += 1;
            }
        }
        self.last_active = active;
    }
```

- [ ] **Step 4: Run the tests to verify the behaviour change**

Run: `cargo nextest run -p multosis`
Expected: PASS — both rewritten cell-light tests now pass, and every other test (engine audio mix, playhead, per-track mix; the modulation tests still calling the `update_block` wrapper) is unaffected. Do not run `cargo clippy` yet: `update_block` is now unused by non-test code and is removed in the next step.

- [ ] **Step 5: Delete `update_block` and migrate the modulation tests**

In `multosis/src/modulation.rs`:

**5a.** Delete the entire `update_block` method (the doc comment `/// Advance every MSEG one process block and apply. Thin wrapper over` through its closing `}`) added in Task 1.

**5b.** Update three stale doc comments on the `Modulation` struct and its methods:

- The `amplitudes` field comment — change `/// Latest per-row amplitude gain, set by `update_block`.` to:
  ```rust
      /// Latest per-row amplitude gain, set by `begin_block` / `advance_segment`.
  ```
- The `fires` field comment — change `/// The rows that fired this block (bit `r` set). Set by `update_block`.` to:
  ```rust
      /// The rows that fired this block (bit `r` set). Zeroed by `begin_block`,
      /// then accumulated by the FreeHz path and by `fire`.
  ```
- The `amplitude` method comment — change `/// The latest amplitude gain for `row` (set by the previous `update_block`).` to:
  ```rust
      /// The latest amplitude gain for `row` (set by the modulation update).
  ```

**5c.** Migrate every `update_block` call in the `#[cfg(test)] mod tests` block to the new API. The rule: a call with a `0` event mask becomes `begin_block` + `advance_segment`; a call with a non-zero mask becomes `begin_block` + `fire(mask)` + `advance_segment`. Apply it to each test:

- `modulation_amplitude_reflects_the_amplitude_mseg` — replace
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `modulation_amplitude_flat_zero_mseg_silences_the_row` — replace
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `modulation_applies_an_assignable_mseg_to_its_effect` — it has two calls. Replace the first
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```
  and the second (inside the `for _ in 0..400` loop) likewise with:
  ```rust
              m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
              m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `modulation_reset_zeroes_phases` — replace the call inside the `for _ in 0..100` loop
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
              m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
              m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `cell_light_fires_on_each_cell_light_event` — replace its four calls. The block sequence becomes:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.fire(1 << 3);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
          assert_eq!(m.fires_last_block(), 1 << 3);
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
          assert_eq!(m.fires_last_block(), 0);
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.fire(1 << 3);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
          assert_eq!(m.fires_last_block(), 1 << 3);
          // A row that didn't get an event doesn't fire even if another did.
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.fire(1 << 7);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
          assert_eq!(m.fires_last_block(), 0);
  ```

- `free_hz_fires_at_roughly_the_expected_rate` — replace the call inside the `for _ in 0..100` loop
  `m.update_block(4800, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
              m.begin_block(4800, 120.0, 48_000.0, &mut effects, &track_effects);
              m.advance_segment(4800, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `free_hz_nonpositive_never_fires` — replace the call inside the `for _ in 0..50` loop
  `m.update_block(480, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
              m.begin_block(480, 120.0, 48_000.0, &mut effects, &track_effects);
              m.advance_segment(480, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `fire_zeros_the_rows_three_phases` — it has two calls. Replace the first (inside the `for _ in 0..50` loop)
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
              m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
              m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```
  and the second `m.update_block(64, 120.0, 48_000.0, 1 << 2, &mut effects, &track_effects);` with:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.fire(1 << 2);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- `free_source_does_not_fire` — replace the call inside the `for _ in 0..20` loop
  `m.update_block(64, 120.0, 48_000.0, 0xFFFF, &mut effects, &track_effects);` with:
  ```rust
              m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
              m.fire(0xFFFF);
              m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```
  (`fire` ignores non-`CellLight` rows, so the all-ones mask still produces no fires for these `Free` rows — this exercises `fire`'s trigger filter.)

- `reset_zeroes_hz_phases` — replace
  `m.update_block(64, 120.0, 48_000.0, 0, &mut effects, &track_effects);` with:
  ```rust
          m.begin_block(64, 120.0, 48_000.0, &mut effects, &track_effects);
          m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
  ```

- [ ] **Step 6: Build, lint, format, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: clean, no warnings, all tests pass. (If `cargo fmt --check` reports drift, run `cargo fmt` and re-run.)

- [ ] **Step 7: Commit**

```bash
git add multosis/src/engine.rs multosis/src/modulation.rs
git commit -m "$(cat <<'EOF'
feat(multosis): sample-accurate cell-light trigger

The engine now drives the modulation from its per-segment loop:
begin_block once, advance_segment per segment, fire at each step
boundary. A CellLight row's phase reset lands at the exact boundary
sample instead of up to one process block late. The pending_cell_lights
cross-block buffer and Modulation::update_block are deleted.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**Spec coverage:**
- §1 the API split — `begin_block` / `advance_segment` / `fire` → Task 1 Step 3. ✓
- §2 restructured `engine.process()` (begin_block, per-segment advance, fire at boundary) → Task 2 Step 3d. ✓
- §3 removed state — `pending_cell_lights` field/init/reset/`mem::take`, `update_block` → Task 2 Steps 3a–3c, 5a. ✓
- §4 edge cases — zero-length segment (`advance_segment` early return, Task 1 Step 3; test Task 1 Step 1 `advance_segment_zero_length_is_a_noop`); boundary at block end (the reset carries over — no special code, covered by the unchanged loop); opening-step `started()` gate (kept verbatim in Task 2 Step 3d; test `cell_light_trigger_fires_on_the_sequences_first_step`); multiple boundaries (the loop handles each — unchanged structure). ✓
- Testing — `fire` resets/ignores (`fire_resets_cell_light_rows_and_ignores_other_triggers`); two-half advance equals tail-from-0 (`advance_segment_in_two_halves_around_a_fire_resets_at_the_split`); `advance_segment` skips FreeHz; FreeHz unchanged (existing tests, migrated); engine same-block fire on the opening step and on a mid-block step (Task 2 Step 1). ✓
- Out of scope — FreeHz stays per-block (handled in `begin_block`, never segmented); no per-sample modulation; no new params. ✓

**Placeholder scan:** No TBD/TODO. Every code step shows complete code. Every test migration in Task 2 Step 5c is spelled out per test, no "similar to".

**Type consistency:** `begin_block(block_len: usize, sample_rate: f64, effects: &mut [EffectInstance; ROWS], track_effects: &[TrackEffect; ROWS])` (no `bpm` — FreeHz advancing is sample-rate-only), `advance_segment(seg_len: usize, bpm: f64, sample_rate: f64, …)`, `fire(newly_rows: u16)`, private `apply_mseg(row: usize, k: usize, …)` — used consistently in Task 1's wrapper, Task 2's `process`, and every test. `mseg_phase_delta` / `advance` / `value_at_phase` / `assignable_value` are existing items reused unchanged. `modulation_fires_for_test()` (engine) and `fires_last_block()` / `phase_for_test()` (modulation) are existing `#[cfg(test)]` helpers, kept.
