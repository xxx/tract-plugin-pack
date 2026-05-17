# Multosis — Phase 1 design

A multi-FX routing sequencer for the Tract Plugin Pack, inspired by Devious
Machines' Infiltrator 2. This document specifies **Phase 1**; it opens with the
whole-project context and phasing so later phases have a fixed reference.

## 1. Project context

Multosis is a 16-track × 32-step grid sequencer. Unlike Infiltrator's
left-to-right step scan, each grid cell can **send a trigger in any of 8
directions** (N, NE, E, SE, S, SW, W, NW). The grid is therefore a directed
routing graph: a "wavefront" of lit cells propagates through it one step at a
time, and routing can split the wavefront, merge it, or loop it.

Each of the 16 tracks (rows) carries an audio effect. When a cell lights, that
row's effect processes the audio for that step.

This is the largest plugin in the collection and is built in phases.

### 1.1 Foundational model decisions (settled in brainstorming)

- **The grid routes triggers, not audio.** An arrow means "when I fire, the
  cell I point at fires on the next step." It is a control-flow graph, not an
  audio graph. Audio-wise, every currently-lit cell runs its row's effect on
  the plugin's **dry input**, and all lit cells' outputs are **summed**.
  Consequence: a routing loop simply cycles the wavefront forever — there is no
  audio feedback to tame, and the audio-thread topology stays static.
- **Routing is independent of audio output.** A lit cell always propagates the
  wavefront per its `sends`, even when it produces no sound. Two conditions
  silence a lit cell *without* affecting its routing: the cell's `enabled`
  flag is false (a per-cell mute), or the cell's track has no effect assigned
  (an empty track — relevant in Phase 2, where effects become assignable). In
  Phase 1 every track always carries a throwaway effect, so only the `enabled`
  case occurs; the empty-track case is recorded here so the model is complete.
- **Envelopes run on their own clock**, decoupled from the step grid
  (Infiltrator-style). The grid only gates an effect on/off; each effect's
  MSEGs sweep underneath at their own Time/Beat length. *(Phase 2 concern —
  recorded here for context.)*
- **Loop region geometry:** a loop region is a sub-rectangle of the grid. It
  defines its own wrap edges — a send from a region cell that would leave the
  rectangle wraps back inside it, so a wavefront inside the region can never
  escape. Cells outside route normally and *may* send into the region.
  Outside the region, the normal full-grid edge wrap applies. The region is at
  least 1×1 and defaults to the entire grid.
- **Cell editing — the octant cell:** each cell's 8 edge/corner zones each
  toggle one send direction on a single click; a center click toggles
  `enabled`; a right-click on the center toggles `start`. No edit modes.

### 1.2 Phasing

- **Phase 1 — routing core + audible sequencer.** This document. The grid
  model, propagation engine, clock, a minimal editor, and two throwaway
  hardwired effects so the sequencer is audible. Split into milestone **1a**
  (headless pure-logic model) and **1b** (editor + audio).
- **Phase 2 — effect abstraction + modulation + effect editor.** A
  standardized effect trait/registry, per-track effect instances, the
  3-MSEG-per-effect modulation engine (one amplitude + two assignable), the
  effect-editor UI tab, and the tabbed shell. Replaces the Phase 1 throwaway
  effects.
- **Phase 3 — presets, seed, advanced triggers, scale-out.** Human-readable
  presets, optional seed, pluggable wavefront trigger sources (MIDI note,
  audio transient, free Hz), envelope retrigger variety, more effects,
  randomization/copy-paste UX polish, docs/manual.

Each phase ends in something that builds, tests, and plays.

## 2. Phase 1 scope

Deliver a working routing sequencer: draw a routing on a 16×32 grid, press
play in a host, and hear the wavefront glitch the audio as it propagates. The
real effect abstraction and modulation engine are explicitly **deferred to
Phase 2** — Phase 1 uses two hardwired throwaway effects purely to make the
sequencer audible.

## 3. Crate & plugin skeleton

- New workspace member `multosis/`, added to the root `Cargo.toml` members.
- A standard nih-plug plugin `MultosisPlugin`: stereo insert effect, VST3 +
  CLAP bundle + standalone bin, following the established per-plugin shape
  (`lib.rs`, `editor.rs`, DSP modules).
- A resizable softbuffer/baseview/tiny-skia editor built on
  `tiny-skia-widgets` (`editor_base` for size persistence, `primitives`,
  `controls`, `drag`).

### 3.1 Parameters vs. state

**nih-plug params** (`MultosisParams`, automatable):

- `speed` — stepped enum: `1/32, 1/16, 1/8, 1/4, 1/2, 1/1`. The tempo-synced
  interval between wavefront advances (straight divisions only in Phase 1).
- `mix` — dry↔wet blend, 0–100%.
- `output_gain` — post-mix gain, in dB.
- `effect_bank` — stepped enum selecting which throwaway effect all rows use:
  `Lowpass` or `Bitcrush`.

**Plugin state** (not params — too large and structural to be params):

- The `Grid` (see §4) is serialized via nih-plug's `#[persist]` JSON.
- GUI→audio handoff: the editor edits its own `Grid`; a `Mutex<Grid>` +
  `try_lock()` snapshot publishes it to the audio thread (the miff
  `KernelHandoff` pattern). The audio thread copies the snapshot when the lock
  is free; it never blocks.
- Audio→GUI: the audio thread owns the live wavefront and publishes it to the
  editor through an atomic bitset (8 × `AtomicU64` = 512 bits), so the editor
  can draw the wavefront without locking (the pope-scope / warp-zone
  atomic-display pattern).

## 4. Grid data model — milestone 1a (pure logic)

All types are `Copy`, heap-free, and serde-serializable; all operations are
pure and exhaustively unit-tested.

### 4.1 Types

- `Direction` — 8-variant enum (N, NE, E, SE, S, SW, W, NW). Each maps to a
  `(drow: i8, dcol: i8)` delta.
- `Cell` (`Copy`) — `enabled: bool`, `is_start: bool`, `sends: u8` (bitmask of
  the 8 directions).
- `LoopRegion` (`Copy`) — `row0, row1, col0, col1` (inclusive bounds). Always
  at least 1×1. Default = the full grid (`0..=15`, `0..=31`).
- `Grid` (`Copy`, ~1.5 KB — same fixed-capacity approach as `MsegData`) —
  `cells: [Cell; 512]` indexed `row * 32 + col` (16 rows × 32 cols) plus a
  `loop_region: LoopRegion`. Helper accessors `cell(row, col)` /
  `cell_mut(row, col)`.

### 4.2 Routing geometry

`next_cell(grid, row, col, dir) -> (row, col)` — the single load-bearing
geometry function:

- Apply `dir`'s delta to `(row, col)`.
- If `(row, col)` is **inside** `loop_region`, wrap the result within the
  region's bounds.
- Otherwise wrap within the full grid (`0..16`, `0..32`).

This is the only place loop-region containment is enforced; everything else
composes it.

### 4.3 Operations (pure, tested, serde-safe)

- `Grid::default_routing()` — every cell sends `E` only; the entire left
  column (`col == 0`) has `is_start = true`; every cell `enabled = true`.
- `reset_routing()` — restore default sends on every cell; leave `enabled` and
  `is_start` untouched. (Recovers from user-created dead ends.)
- `reinit_activations()` — restore default `enabled` (all true) and `is_start`
  (left column) on every cell; leave `sends` untouched.
- `randomize_activations(seed)` — randomize `enabled` for cells within the
  loop region; deterministic in `seed`.
- `randomize_routing(seed)` — randomize `sends` for cells within the loop
  region; **guarantees no dead ends** by ensuring every randomized cell keeps
  `sends != 0`. Deterministic in `seed`.
- `copy_region()` — snapshot the cells covered by the current loop region.
- `paste_region(snapshot)` — write a copied snapshot anchored at the current
  loop region's top-left corner `(row0, col0)`, **truncating** any cells that
  fall outside the grid. To relocate a paste, move the loop region between
  copying and pasting.

The randomization RNG is a small deterministic PRNG seeded by the `seed`
argument (no external crate; same spirit as the MSEG `randomize` module).

## 5. Propagation engine + clock — milestone 1a

### 5.1 Wavefront & lifecycle

- `Wavefront` (`Copy`) — a `[bool; 512]` (or packed bitset) of currently-lit
  cells.
- Sequence state machine:
  - **Initial** — wavefront empty. The next tick *arms* it: wavefront = every
    cell with `is_start = true`. (If there are no start cells, the wavefront
    stays empty and nothing plays — this is how the plugin knows what is next
    in its initial state.)
  - **Running** — each tick computes
    `next = ⋃ { next_cell(grid, r, c, dir) | cell (r,c) lit, dir ∈ cell.sends }`.
    If `next` is non-empty it becomes the wavefront. If `next` is empty (every
    lit cell is a dead end) the wavefront **dies** and the state returns to
    *Initial* — output is silent until the sequence is reset.
- `reset()` — empty the wavefront and return to *Initial*, re-arming start
  cells on the following tick.

### 5.2 Clock

- Tempo-synced. `samples_per_step` is derived from the host BPM and `speed`.
- The wavefront advances **only while host transport is playing**. The
  stopped→playing transition triggers `reset()`.
- A manual **Reset** button in the editor also triggers `reset()`.
- Step counting tolerates a process block containing zero or several step
  boundaries.

## 6. Audio engine + throwaway effects — milestone 1b

Propagation runs on the **audio thread** for sample-accurate step timing — it
is cheap, fixed-size, allocation-free pure logic.

Each process block:

1. `try_lock()` the `Grid` handoff; copy the snapshot if the lock is free.
2. Advance the clock; split the block into sub-blocks at each step boundary
   and advance propagation at each boundary.
3. Determine **active rows**: a row is active if *any* of its cells is lit
   (dedupe per tick — a row's effect runs at most once per step regardless of
   how many of its cells are lit).
4. For each active **enabled** row, process a copy of the dry input through
   that row's throwaway effect and sum the results. A disabled-but-lit row
   contributes nothing; if every lit row is disabled the step is silent.
5. `out = lerp(dry, wet_sum, mix)`, then apply `output_gain`.
6. Publish the wavefront to the atomic display bitset.

Per-row effect state lives in 16 pre-allocated slots and **persists across
steps** so filters do not click when a row reactivates. No allocations on the
audio thread; `try_lock()` only — per CLAUDE.md.

Wet level is a plain sum in Phase 1; heavy wavefront splits can therefore get
loud. This is documented and left for the user to manage with `mix` /
`output_gain` (consistent with the `imagine` Width-law level note). Automatic
level compensation is out of scope for Phase 1.

### 6.1 Throwaway effects

Hardwired, with no shared abstraction (the standardized trait is Phase 2):

- **Lowpass** — a resonant lowpass (SVF or one-pole+resonance). Cutoff is
  mapped from row index: row 0 darkest → row 15 fully open.
- **Bitcrush** — sample-rate / bit-depth reduction. Crush amount mapped from
  row index: row 0 most crushed → row 15 nearly clean.

Mapping the character to row index makes the wavefront's vertical motion
immediately audible. `effect_bank` selects which effect all rows use.

## 7. Grid editor UI — milestone 1b

A single screen (the tabbed shell is Phase 2):

- **Top toolbar:** sequence status (Initial / Running + step count), a
  **Reset** button, the six grid operations (Reset routing, Reinit cells,
  Randomize activations, Randomize routing, Copy, Paste), and the
  `speed` / `mix` / `effect_bank` / `output_gain` controls.
- **Main area:** the 16-row × 32-column grid of **octant cells**. Each cell
  draws its 8 send arrows (lit/unlit), a center fill for `enabled`, and a ring
  marker for `is_start`. The live wavefront is drawn in orange.
- **Loop region:** drawn as a highlighted rectangle; drag handles on the
  grid's top edge (column range) and left edge (row range) resize it.
- **Interaction:** a click is hit-tested to a cell and an octant/center zone
  and toggles the corresponding bit; a right-click on a center toggles
  `start`. Copy snapshots the current loop region; Paste writes the last
  snapshot anchored at the loop region's current top-left corner, truncating
  at the grid edges — move the loop region between copy and paste to relocate.
- Resizable: `scale = physical_width / WINDOW_WIDTH`, size persisted via
  `EditorState`; host resize via the packed-`AtomicU64` `pending_resize`
  pattern. Redraws at ~60 fps to animate the wavefront.

## 8. Testing & milestones

Per CLAUDE.md: TDD, inline `#[cfg(test)]` modules, `cargo nextest`.

- **Milestone 1a — headless model.** Exhaustive unit tests: `next_cell` edge
  wrap; loop-region containment (no send escapes a region); propagation
  (split, merge, loop); dead-end death returns to Initial; start-cell arming;
  `randomize_routing` never produces a dead end; `copy_region` /
  `paste_region` truncation; serde round-trips of `Grid`. `cargo nextest run`
  green; no GUI, no audio.
- **Milestone 1b — editor + audio.** Octant hit-testing math and clock
  `samples_per_step` math unit-tested. The editor and audio path are verified
  by running the standalone debug bin (`cargo build --bin multosis`) in a host
  and by ear.

## 9. Out of scope for Phase 1

The standardized effect trait/registry; per-track effect instances; the 3-MSEG
modulation engine and envelope assignment; the effect-editor UI and tabbed
shell; human-readable presets and the seed; pluggable/advanced wavefront
trigger sources; automatic wet-level compensation; undo/redo. All are Phase 2
or Phase 3.
