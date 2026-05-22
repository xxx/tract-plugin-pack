# Cell-Step Modulation Trigger — Design

**Date:** 2026-05-21
**Status:** Approved

## Summary

Add a fourth modulation `TriggerSource` to multosis — **`CellStep`** — that
retriggers a track's three MSEGs on *every* sequencer step the row has a lit
cell under the playhead, including consecutive lit cells. It sits alongside
the existing `Free`, `CellLight`, and `FreeHz`; `CellLight` is unchanged.

The change is confined to `multosis` — `modulation.rs` (the new variant and
the `fire` condition) and `editor/effect_editor.rs` (the trigger dropdown),
with a one-argument extension to the engine→modulation `fire` call.

## Motivation

The `CellLight` trigger fires a track's modulation only on the row's
inactive→active **edge** — the first cell of a contiguous run of lit cells.
A run of four adjacent lit cells fires once. There is no way to retrigger the
modulation on *each* step a row plays: a steady run of cells gives one long
modulation sweep, never a per-step pulse.

`CellStep` fills that gap — it fires on every step the row is lit, so a run
of lit cells produces a fire per cell. It is the per-step counterpart of
`CellLight`'s per-run behaviour.

## Current model (for reference)

`TriggerSource` (`modulation.rs`) is `Free | CellLight | FreeHz { hz }`. It
decides what resets a track's three MSEG phases to 0:

- `Free` — never; the MSEGs free-run on their own clocks.
- `CellLight` — on the row's inactive→active edge at the playhead.
- `FreeHz { hz }` — every `1/hz` seconds, independent of the grid.

At each step boundary the engine (`engine.rs::process`) computes the
pre-tick and post-tick active-row masks, derives `newly = after & !before`,
and calls `Modulation::fire(newly)`. `fire` resets the phases of every
`CellLight` row whose bit is in `newly`. Between fires, `advance_segment`
advances the per-MSEG `phases` clocks of every `Free` and `CellLight` row;
`FreeHz` rows use the separate `hz_phases` clock advanced by `begin_block`.

## Design

### 1. The `CellStep` variant

`TriggerSource` gains a `CellStep` variant, ordered between `CellLight` and
`FreeHz` so the two cell-based modes are adjacent:

```rust
pub enum TriggerSource {
    Free,
    CellLight,
    CellStep,
    FreeHz { hz: f32 },
}
```

`TriggerSource` derives `Serialize`/`Deserialize`; serde tags enum variants
by **name**, so inserting a variant is backward-compatible — existing saved
patches carrying `Free` / `CellLight` / `FreeHz` deserialize unchanged.

A `CellStep` row uses the per-MSEG `phases` clock, exactly like `Free` and
`CellLight`. `advance_segment` already advances every non-`FreeHz` row, so it
advances `CellStep` rows with no change. `begin_block` handles only `FreeHz`
rows and likewise needs no change.

### 2. Fire condition

`CellLight` fires on the inactive→active **edge** (`newly = after & !before`).
`CellStep` fires whenever the row is **active** at the new step — i.e. its bit
is in the post-tick active mask `after`, regardless of `before`. So:

- a run of contiguous lit cells fires `CellStep` once per cell;
- the opening step fires a `CellStep` row if it is lit there (no `before`
  diff is consulted, so no `started()` gating is needed for `CellStep`);
- a step where the row has no lit cell never fires.

`Modulation::fire` gains the post-tick active mask as a second parameter:

```rust
pub fn fire(&mut self, newly_rows: u16, active_rows: u16)
```

Per row: a `CellLight` row resets if its bit is in `newly_rows`; a `CellStep`
row resets if its bit is in `active_rows`; `Free` and `FreeHz` rows are
ignored. A firing row's three MSEG phases reset to 0 and its `fires` bit is
set (multiple fires across a block's boundaries accumulate, as today).

The engine's step-boundary handler already has both masks — it computes
`after` (the post-tick active mask) and `newly`. The single `fire(newly)`
call becomes `fire(newly, after)`. No other engine change.

### 3. Editor

`effect_editor.rs`:

- `trigger_items()` returns four labels: `["Free run", "Cell light",
  "Cell step", "Free Hz"]`.
- `trigger_from_item` / `trigger_to_item` map the new index 2 to/from
  `TriggerSource::CellStep`; `FreeHz` moves to index 3.

The trigger dropdown then offers four options. `CellStep` has no rate dial —
the trigger-rate dial remains `FreeHz`-only (the `is_free_hz` checks that
gate the rate dial already use `matches!(.., FreeHz { .. })`, so they
correctly exclude `CellStep`).

## Testing

- **modulation.rs** — `fire` resets a `CellStep` row whenever its bit is in
  the active mask, including two consecutive steps; `fire` does not reset a
  `CellStep` row whose bit is absent from the active mask; `fire` still
  resets `CellLight` rows from `newly` and ignores `Free`/`FreeHz`.
- **engine.rs** — a `CellStep` row with contiguous lit cells fires on *each*
  cell (assert the fire on a step where the row was already active the
  previous step — the case `CellLight` skips); a `CellStep` row fires on the
  opening step when lit there.
- **effect_editor.rs** — `trigger_items()` lists four entries;
  `trigger_from_item` / `trigger_to_item` round-trip all four variants,
  including `CellStep` at index 2 and `FreeHz` at index 3.
- Existing `fire` call sites (the engine and the modulation tests) updated
  for the new two-argument signature; all existing tests still pass —
  `CellLight`, `Free`, and `FreeHz` behaviour is unchanged.

`cargo build -p multosis`, `cargo clippy -p multosis -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` all clean.

## Out of scope

- No change to `CellLight`, `Free`, or `FreeHz` behaviour.
- No new parameters, no rate dial for `CellStep`.
- No change to `begin_block` / `advance_segment`, the grid, or propagation.
