# Sample-Accurate Cell-Light Detection — Design

**Date:** 2026-05-21
**Status:** Approved

## Summary

Remove the up-to-one-process-block latency in the multosis `CellLight`
modulation trigger. Today a cell lighting at a step boundary is detected at
the exact tick but its effect — resetting the row's MSEG phases — is buffered
and not applied until the *next* process block. This design drives the
modulation update from the engine's existing per-segment loop, so a
`CellLight` row's phase reset lands at the exact step-boundary sample.

The change is confined to `multosis` — `engine.rs` and `modulation.rs`. No
new parameters, no UI change, no audio-thread allocation.

## Motivation

The `CellLight` trigger fires a track's modulation when a lit, enabled cell
first appears under the playhead. The edge is detected sample-accurately in
the engine's segment loop (`newly = after & !before` at each step-boundary
tick). But the detected mask is accumulated into `AudioEngine::pending_cell_lights`
and handed to `Modulation::update_block` only at the *start of the next
block*. `update_block` is monolithic — it advances every MSEG by the whole
block length in one shot.

Consequences:

- The phase reset lags the cell light by up to one block — ~1.3 ms at
  64-sample blocks, ~21 ms at 1024.
- When the reset finally runs, `update_block` advances the firing row's MSEGs
  by a *full* block from phase 0, even though the cell lit partway through
  the previous block.

For a rhythmic modulation source meant to lock to the step grid, a variable
1–21 ms slip is audible as timing jitter.

## Current interaction model (for reference)

Per process block, `AudioEngine::process` (`engine.rs`):

1. `cell_light_events = std::mem::take(&mut self.pending_cell_lights)` — takes
   the *previous* block's accumulated edges.
2. `modulation.update_block(n, bpm, sr, cell_light_events, …)` — one call
   advances all MSEGs by the full block, decides fires, resets firing rows'
   phases, evaluates and applies amplitude + target parameters.
3. The block is walked in segments split at each step boundary; the audio mix
   runs per sample. At each boundary tick the engine computes `before` /
   `after` active-row masks and accumulates `newly = after & !before` into
   `self.pending_cell_lights` — for the *next* block to consume.

`Modulation` state: `config: [TrackModulation; ROWS]`, per-row per-MSEG
`phases: [[f32; 3]; ROWS]`, the FreeHz clock `hz_phases: [f32; ROWS]`,
`amplitudes: [f32; ROWS]`, and `fires: u16`. Free and `CellLight` tracks use
the per-MSEG `phases` clocks; `FreeHz` tracks use the shared `hz_phases`
clock instead.

## Design

### 1. The `Modulation` API split

`Modulation::update_block` is replaced by three methods.

**`begin_block(block_len, bpm, sr, effects, track_effects)`** — block-rate
setup, called once at the top of `process()`:

- Zeroes `self.fires`.
- Handles every `FreeHz` row exactly as `update_block` does today: advance
  `hz_phases[row]` by the full block, decide the fire (wrap past 1.0,
  retaining the fractional remainder; multiple wraps still count as one fire),
  reset the firing row, then evaluate and apply all three MSEGs at the
  `hz_phases` value. `FreeHz` is a free-running clock, not edge-driven — it
  remains per-block. Out of scope for this change.

**`advance_segment(seg_len, bpm, sr, effects, track_effects)`** — called once
per segment from the engine's segment loop:

- For every `Free` and `CellLight` row, advance each of the three MSEGs'
  `phases[row][k]` by `mseg_phase_delta(mseg, seg_len, bpm, sr)`, evaluate,
  and apply: the amplitude MSEG (`k == 0`) writes `amplitudes[row]`; an
  assigned assignable MSEG writes its target effect parameter via `set_param`.
- `FreeHz` rows are skipped — they were fully handled in `begin_block`.
- A `seg_len` of 0 is a no-op.

Because `mseg_phase_delta` is linear in length, advancing a block in segments
sums to the same phase as one whole-block advance when no reset intervenes —
so a block with no step boundary behaves exactly as `update_block` did.

**`fire(newly_rows: u16)`** — called at a step-boundary tick:

- For each row in `newly_rows` whose `config[row].trigger` is `CellLight`:
  reset `phases[row]` to `[0.0; 3]` and set its bit in `self.fires`.
- Rows in `newly_rows` with any other trigger are ignored — trigger policy
  stays inside `modulation.rs`, where the config lives.

Rationale for the split: only the per-MSEG-clock path (`Free` / `CellLight`)
can be reset mid-block by a `CellLight` edge, so only it needs segmenting.
The `hz_phases` path is untouched. This mirrors the `FreeHz`-vs-rest branch
already present in `update_block`.

### 2. Restructured `engine.process()`

```
begin_block(n, bpm, sr, …)               // FreeHz fires; fires = 0
gather step-boundary offsets (while playing) — unchanged
active = active_rows(grid, loop_region, playhead.column())
cursor = 0; bi = 0
while cursor < n:
    seg_end = boundary[bi].clamp(cursor, n)  if bi < n_boundaries  else  n
    seg_len = seg_end − cursor
    advance_segment(seg_len, bpm, sr, …)  // Free/CellLight MSEGs for this segment
    for i in cursor..seg_end:             // audio mix — unchanged
        process_sample → compressor → dry/wet mix
    cursor = seg_end
    if bi < n_boundaries:
        before = active_rows(…) if playhead.started() else 0   // gate unchanged
        playhead.tick(&grid.loop_region); step += 1
        after  = active_rows(…)
        newly  = after & !before
        modulation.fire(newly)            // ← immediate; replaces pending_cell_lights
        active = after
        bi += 1
last_active = active
```

`advance_segment` runs at the **start** of each segment iteration, before the
segment's audio is rendered, so the segment's audio reflects that segment's
advance — matching the old ordering, where `update_block` ran before the
block's audio. After a boundary `fire`, the next iteration's
`advance_segment` advances from the just-reset phase 0; the boundary-crossing
sample onward is rendered with the post-reset modulation.

### 3. Removed state

- `AudioEngine::pending_cell_lights` — deleted. The cross-block buffer and the
  `std::mem::take` that drained it are gone.
- `Modulation::update_block` — deleted, replaced by the three methods above.
- The `cell_light_events: u16` parameter threaded from the engine into the
  modulation update — gone; the engine calls `fire(newly)` directly.

`Modulation::fires` and the `fires_last_block()` test helper are kept. `fires`
now accumulates across the block: `FreeHz` bits set in `begin_block`,
`CellLight` bits set in `fire`. At block end it still reports every row that
fired during the block.

### 4. Edge cases

- **Zero-length segment** — a boundary exactly at a segment start yields
  `seg_len == 0`; `advance_segment(0)` is a no-op. The boundary handler still
  advances `bi`, so there is no infinite loop (unchanged from today).
- **Boundary at block end** — `fire` resets the phases; with no further
  segment this block, the reset carries into the next block's first
  `advance_segment`. Correct.
- **Opening step** — when the playhead starts, the `started()` gate keeps
  `before = 0`, so the opening step's lit cells register as `newly` and fire.
  Same as today; only the timing moves from next-block to same-block.
- **Multiple boundaries per block** — each boundary runs its own `fire`; each
  inter-boundary segment gets its own `advance_segment`. `MAX_BOUNDARIES` is
  unchanged.

## Testing

`modulation.rs` unit tests (the existing `update_block` callers are rewritten
to the new API):

- `fire` resets the phases of a `CellLight` row passed in `newly_rows` and
  sets its `fires` bit; a `Free` or `FreeHz` row in `newly_rows` is ignored.
- `advance_segment` over a whole block (one segment, no boundary) leaves the
  same MSEG phases as the old `update_block` did — no-boundary equivalence.
- Advancing a block as two segments around a mid-block `fire` leaves each
  firing row's phase equal to an advance-from-0 over `block_len − offset`
  samples — the reset is sample-accurate.
- `FreeHz` rows still fire once per block on a wrap, retaining the fractional
  remainder — existing `FreeHz` tests pass unchanged.

`engine.rs` integration tests:

- A `CellLight` row whose cell lights at a step boundary partway through a
  block fires **in that block** — the row's MSEG phases are reset before the
  block returns. (The current tests assert the one-block delay; they are
  rewritten for same-block firing.)
- The opening step still fires for a `CellLight` row.
- The existing cell-light edge-detector regression test (the `after & !before`
  diff, the `started()` gate) still holds — detection is unchanged; only
  consumption timing moves.

`cargo build -p multosis`, `cargo clippy -p multosis -- -D warnings`,
`cargo fmt --check`, and `cargo nextest run -p multosis` all clean.

## Out of scope

- **`FreeHz` timing** — it remains a per-block free clock. Making `FreeHz`
  fires sample-accurate is a separate concern, not a cell-light edge.
- **Per-sample modulation output** — the MSEGs are still evaluated at block
  and step-boundary granularity. This change fixes *trigger timing* only; it
  does not render the modulation curve continuously.
- No new parameters, no UI changes, no changes to effects, the grid, or
  propagation.
