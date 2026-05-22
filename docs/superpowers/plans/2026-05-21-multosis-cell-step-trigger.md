# Cell-Step Modulation Trigger Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fourth multosis modulation trigger, `CellStep`, that retriggers a track's MSEGs on every sequencer step the row has a lit cell — including consecutive cells.

**Architecture:** A new `TriggerSource::CellStep` variant. The engine→modulation `fire` call, run at each step boundary, gains the post-tick active-row mask as a second argument: a `CellLight` row fires on the inactive→active edge (`newly`), a `CellStep` row fires whenever it is active (`after`). The editor's trigger dropdown grows from three options to four.

**Tech Stack:** Rust (nightly), `multosis` crate (a nih-plug plugin) in the `tract-plugin-pack` Cargo workspace. `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-05-21-multosis-cell-step-trigger-design.md`

**Conventions:**
- Run all `cargo`/`git` from the workspace root `/home/mpd/git-sources/tract-plugin-pack`. Branch: `multosis`.
- Build/test/lint just this crate: `cargo build -p multosis`, `cargo nextest run -p multosis`, `cargo clippy -p multosis -- -D warnings`, `cargo fmt --check`.
- Never use `#[allow(...)]` to silence a warning.
- Commit message trailer MUST be exactly: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Editor diagnostics are often stale — verify with a real build/test.

## File structure

This is a single atomic change — adding the `TriggerSource::CellStep` variant forces the editor's two exhaustive `match` expressions to update in the same commit, so it cannot be split.

- `multosis/src/modulation.rs` — the `TriggerSource::CellStep` variant; `Modulation::fire` rewritten to take the active-row mask and fire `CellStep` rows from it; the test-module `fire` callers updated to the new arity; a new `CellStep` unit test.
- `multosis/src/engine.rs` — the `fire` call at the step boundary passes the post-tick `after` mask; a new engine integration test.
- `multosis/src/editor/effect_editor.rs` — `trigger_items` (3→4 entries), `trigger_from_item`, `trigger_to_item`, the `draw_trigger_controls` label `match`; the two trigger unit tests updated.

## Background — the current code

`TriggerSource` (`modulation.rs`, lines ~20-28) is:

```rust
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    #[default]
    Free,
    CellLight,
    FreeHz {
        hz: f32,
    },
}
```

`Modulation::fire` (currently `pub fn fire(&mut self, newly_rows: u16)`) resets the three MSEG phases of every `CellLight` row whose bit is in `newly_rows`. It is called once per step boundary from `AudioEngine::process` as `self.modulation.fire(newly)`, where `newly = after & !before` and `after` is the post-tick active-row mask. `CellStep` rows will be advanced by the existing `advance_segment` with no change (it advances every non-`FreeHz` row).

`TriggerSource` derives `Serialize`/`Deserialize`; serde tags enum variants by name, so inserting a variant is backward-compatible — old saved patches load unchanged.

---

## Task 1: Add the `CellStep` trigger

**Files:**
- Modify: `multosis/src/modulation.rs`
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/editor/effect_editor.rs`

- [ ] **Step 1: Write the failing tests**

**1a.** In `multosis/src/modulation.rs`, inside the `#[cfg(test)] mod tests` block (e.g. just before its closing `}`), add:

```rust
    #[test]
    fn fire_resets_cell_step_rows_on_every_active_step() {
        let mut m = Modulation::new();
        let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
        cfg[6].trigger = TriggerSource::CellStep;
        m.set_config(&cfg);
        let mut effects: [EffectInstance; ROWS] =
            std::array::from_fn(|_| EffectInstance::new(EffectKind::Lowpass));
        let track_effects: [TrackEffect; ROWS] = std::array::from_fn(TrackEffect::default_for_row);
        // Drift row 6's phases away from 0.
        for _ in 0..50 {
            m.begin_block(64, 48_000.0, &mut effects, &track_effects);
            m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        }
        assert!(m.phase_for_test(6, 0) > 1e-6, "row 6 drifted");
        // A step where row 6 is active (in `active_rows`) but NOT newly-lit
        // (absent from `newly_rows`) still fires CellStep — the case CellLight
        // skips.
        m.begin_block(64, 48_000.0, &mut effects, &track_effects);
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires on an active, non-newly step"
        );
        assert_eq!(
            m.fires_last_block() & (1 << 6),
            1 << 6,
            "CellStep sets its fires bit"
        );
        // It advances after the reset, then fires again on the next active step.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        assert!(m.phase_for_test(6, 0) > 1e-6, "row 6 advanced after the reset");
        m.fire(0, 1 << 6);
        assert_eq!(
            m.phase_for_test(6, 0),
            0.0,
            "CellStep fires again on the next consecutive active step"
        );
        // A step where row 6 is NOT active does not fire.
        m.advance_segment(64, 120.0, 48_000.0, &mut effects, &track_effects);
        let drifted = m.phase_for_test(6, 0);
        assert!(drifted > 1e-6);
        m.fire(0, 0);
        assert_eq!(
            m.phase_for_test(6, 0),
            drifted,
            "no fire on a step where the row is inactive"
        );
    }
```

**1b.** In `multosis/src/engine.rs`, inside its `#[cfg(test)] mod tests` block, add:

```rust
    #[test]
    fn cell_step_trigger_fires_on_every_step_not_just_the_edge() {
        // The default grid enables every cell, so every row is active at
        // every column. After the opening step a row stays continuously
        // active — no inactive->active edge — so CellLight fires only once
        // (block 1) while CellStep fires on every step (both blocks).
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut mod_cfg: [crate::modulation::TrackModulation; ROWS] =
            std::array::from_fn(crate::modulation::TrackModulation::default_for_row);
        mod_cfg[4].trigger = crate::modulation::TriggerSource::CellLight;
        mod_cfg[9].trigger = crate::modulation::TriggerSource::CellStep;
        engine.set_modulation(&mod_cfg);
        let grid = Grid::default();
        // Block 1: the playhead starts; both rows fire on the opening step.
        let mut l1 = [0.0_f32; 64];
        let mut r1 = [0.0_f32; 64];
        engine.process(&mut l1, &mut r1, true, 10.0, 120.0, 1.0, &grid);
        assert!(
            engine.modulation_fires_for_test() & (1 << 4) != 0,
            "CellLight fires on the opening step"
        );
        assert!(
            engine.modulation_fires_for_test() & (1 << 9) != 0,
            "CellStep fires on the opening step"
        );
        // Block 2: both rows stay continuously active (no new edge).
        let mut l2 = [0.0_f32; 64];
        let mut r2 = [0.0_f32; 64];
        engine.process(&mut l2, &mut r2, true, 10.0, 120.0, 1.0, &grid);
        assert_eq!(
            engine.modulation_fires_for_test() & (1 << 4),
            0,
            "CellLight does not fire on a non-edge step"
        );
        assert!(
            engine.modulation_fires_for_test() & (1 << 9) != 0,
            "CellStep fires on every step, including non-edge steps"
        );
    }
```

**1c.** In `multosis/src/editor/effect_editor.rs`, in its `#[cfg(test)] mod tests` block, **replace** the existing `trigger_items_lists_three_sources` test and the existing `trigger_from_and_to_item_round_trip` test with:

```rust
    #[test]
    fn trigger_items_lists_the_four_sources() {
        let items = trigger_items();
        assert_eq!(items, ["Free run", "Cell light", "Cell step", "Free Hz"]);
    }

    #[test]
    fn trigger_from_and_to_item_round_trip() {
        // 0 -> Free, 1 -> CellLight, 2 -> CellStep, 3 -> FreeHz{<carried hz>}.
        assert_eq!(trigger_from_item(0, 1.0), TriggerSource::Free);
        assert_eq!(trigger_from_item(1, 1.0), TriggerSource::CellLight);
        assert_eq!(trigger_from_item(2, 1.0), TriggerSource::CellStep);
        assert_eq!(trigger_from_item(3, 3.5), TriggerSource::FreeHz { hz: 3.5 });
        assert_eq!(trigger_to_item(TriggerSource::Free), 0);
        assert_eq!(trigger_to_item(TriggerSource::CellLight), 1);
        assert_eq!(trigger_to_item(TriggerSource::CellStep), 2);
        assert_eq!(trigger_to_item(TriggerSource::FreeHz { hz: 99.0 }), 3);
    }
```

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo nextest run -p multosis`
Expected: a **compile error** — `TriggerSource::CellStep` does not exist, and `fire` is being called with two arguments but currently takes one.

- [ ] **Step 3: Add the `CellStep` variant**

In `multosis/src/modulation.rs`, replace the `TriggerSource` enum and its doc comment with:

```rust
/// The event that causes a track's three MSEG phases to reset to 0.
/// `Free` is the free-running default; `CellLight` fires on the row's
/// inactive→active edge at the playhead; `CellStep` fires on every step the
/// row is lit; `FreeHz` fires every `1.0/hz` seconds independently of sync.
#[derive(Clone, Copy, PartialEq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum TriggerSource {
    #[default]
    Free,
    CellLight,
    CellStep,
    FreeHz {
        hz: f32,
    },
}
```

- [ ] **Step 4: Rewrite `Modulation::fire`**

In `multosis/src/modulation.rs`, replace the entire `fire` method (its doc comment beginning `/// Fire the \`CellLight\` trigger` through its closing `}`) with:

```rust
    /// Fire the per-step modulation triggers at a step boundary. `newly_rows`
    /// is the inactive→active edge mask (bit `r` = row `r` first lit this
    /// step); `active_rows` is the post-tick active mask (bit `r` = row `r`
    /// has a lit, enabled cell under the playhead now). A `CellLight` row
    /// fires if it is in `newly_rows`; a `CellStep` row fires if it is in
    /// `active_rows`; `Free` and `FreeHz` rows never fire. A firing row's
    /// three MSEG phases reset to 0 and its `fires` bit is set. Called at a
    /// step boundary, so the reset takes effect on the very next segment.
    pub fn fire(&mut self, newly_rows: u16, active_rows: u16) {
        for row in 0..ROWS {
            let reset = match self.config[row].trigger {
                TriggerSource::CellLight => newly_rows & (1 << row) != 0,
                TriggerSource::CellStep => active_rows & (1 << row) != 0,
                TriggerSource::Free | TriggerSource::FreeHz { .. } => false,
            };
            if reset {
                self.phases[row] = [0.0; 3];
                self.fires |= 1 << row;
            }
        }
    }
```

- [ ] **Step 5: Update the engine `fire` call**

In `multosis/src/engine.rs`, in `process()`'s step-boundary block, find:

```rust
                // Snapshot the active-row mask BEFORE and AFTER the tick;
                // rows that became active fire their CellLight trigger now.
```

Replace those two comment lines with:

```rust
                // Snapshot the active-row mask BEFORE and AFTER the tick.
                // `newly` (became-active) fires CellLight; the post-tick
                // `after` mask fires CellStep.
```

Then change the `fire` call a few lines below from:

```rust
                self.modulation.fire(newly);
```

to:

```rust
                self.modulation.fire(newly, after);
```

(`after` is the `let after = active_rows(...)` already computed two lines above the call.)

- [ ] **Step 6: Update the existing `fire` callers in the modulation tests**

In `multosis/src/modulation.rs`'s `#[cfg(test)] mod tests` block, every existing `m.fire(MASK)` call must become `m.fire(MASK, MASK)` (the rows passed are both newly-lit and active — a newly-lit row is always active). There are seven such calls across these tests:

- `cell_light_fires_on_each_cell_light_event` — `m.fire(1 << 3);` appears twice → both become `m.fire(1 << 3, 1 << 3);`; `m.fire(1 << 7);` → `m.fire(1 << 7, 1 << 7);`
- `fire_zeros_the_rows_three_phases` — `m.fire(1 << 2);` → `m.fire(1 << 2, 1 << 2);`
- `free_source_does_not_fire` — `m.fire(0xFFFF);` → `m.fire(0xFFFF, 0xFFFF);`
- `fire_resets_cell_light_rows_and_ignores_other_triggers` — `m.fire((1 << 2) | (1 << 3) | (1 << 4));` → `m.fire((1 << 2) | (1 << 3) | (1 << 4), (1 << 2) | (1 << 3) | (1 << 4));`
- `advance_segment_in_two_halves_around_a_fire_resets_at_the_split` — `m.fire(1 << 1);` → `m.fire(1 << 1, 1 << 1);`

(The new `fire_resets_cell_step_rows_on_every_active_step` test from Step 1a already uses the two-argument form — leave it as written.)

- [ ] **Step 7: Update the editor trigger functions**

In `multosis/src/editor/effect_editor.rs`:

**7a.** Replace `trigger_items` with (note the return type changes from `[&'static str; 3]` to `[&'static str; 4]`):

```rust
/// The trigger-source dropdown items, in `TriggerSource` discriminant order.
pub fn trigger_items() -> [&'static str; 4] {
    ["Free run", "Cell light", "Cell step", "Free Hz"]
}
```

**7b.** Replace `trigger_from_item` with:

```rust
/// Build a `TriggerSource` from a dropdown item index. `carried_hz` is the
/// `hz` to seed `FreeHz` with (the dial's current value, or a default).
pub fn trigger_from_item(item: usize, carried_hz: f32) -> TriggerSource {
    match item {
        0 => TriggerSource::Free,
        1 => TriggerSource::CellLight,
        2 => TriggerSource::CellStep,
        _ => TriggerSource::FreeHz { hz: carried_hz },
    }
}
```

**7c.** Replace `trigger_to_item` with:

```rust
/// The dropdown item index for a `TriggerSource`.
pub fn trigger_to_item(src: TriggerSource) -> usize {
    match src {
        TriggerSource::Free => 0,
        TriggerSource::CellLight => 1,
        TriggerSource::CellStep => 2,
        TriggerSource::FreeHz { .. } => 3,
    }
}
```

**7d.** In `draw_trigger_controls`, the `let label = match trigger { ... }` expression has arms for `Free`, `CellLight`, and `FreeHz`. Add a `CellStep` arm so the match stays exhaustive:

```rust
    let label = match trigger {
        TriggerSource::Free => "Free run",
        TriggerSource::CellLight => "Cell light",
        TriggerSource::CellStep => "Cell step",
        TriggerSource::FreeHz { .. } => "Free Hz",
    };
```

- [ ] **Step 8: Build, lint, format, test**

Run: `cargo build -p multosis && cargo clippy -p multosis -- -D warnings && cargo fmt --check && cargo nextest run -p multosis`
Expected: clean, no warnings, all tests pass. (If `cargo fmt --check` reports drift, run `cargo fmt` and re-run.) If the compiler flags any *other* non-exhaustive `match` on `TriggerSource` beyond `trigger_to_item` and `draw_trigger_controls`, add a `CellStep` arm there too — but none is expected.

- [ ] **Step 9: Commit**

```bash
git add multosis/src/modulation.rs multosis/src/engine.rs multosis/src/editor/effect_editor.rs
git commit -m "$(cat <<'EOF'
feat(multosis): CellStep modulation trigger

A fourth TriggerSource that retriggers a track's MSEGs on every
sequencer step the row has a lit cell — including consecutive cells,
which CellLight (edge-only) skips. Modulation::fire gains the post-tick
active-row mask: CellLight fires from the newly-lit mask, CellStep from
the active mask. The editor's trigger dropdown grows to four options.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

**Spec coverage:**
- §1 the `CellStep` variant, ordered between `CellLight` and `FreeHz`, serde-compatible → Step 3. ✓
- §2 fire condition — `CellLight` from `newly`, `CellStep` from `active`; `fire` gains the active mask; the engine passes `after`; opening step + consecutive cells fire `CellStep` → Steps 4, 5; covered by the Step 1a/1b tests. ✓
- §3 editor — `trigger_items` (4 entries), `trigger_from_item`/`trigger_to_item`, no rate dial for `CellStep` (the `FreeHz`-only `matches!` gates are untouched and correctly exclude `CellStep`) → Step 7. ✓
- Testing — modulation `fire` test, engine contiguous-cell + opening-step test, editor four-item + round-trip tests, existing `fire` callers updated → Steps 1, 6. ✓
- Out of scope — no change to `CellLight`/`Free`/`FreeHz` behaviour, no new params, `begin_block`/`advance_segment` untouched. ✓

**Placeholder scan:** No TBD/TODO. Every step shows complete code. Step 6 enumerates all seven `fire` call sites explicitly.

**Type consistency:** `TriggerSource::CellStep` (fieldless, like `CellLight`). `fire(&mut self, newly_rows: u16, active_rows: u16)` — used consistently in the engine call (Step 5), the migrated test callers (Step 6), and the new test (Step 1a). `trigger_items() -> [&'static str; 4]`, `trigger_from_item`/`trigger_to_item` map index 2 ↔ `CellStep` and index 3 ↔ `FreeHz` consistently across Steps 1c and 7. `draw_trigger_controls`'s label match covers all four variants (Step 7d).
