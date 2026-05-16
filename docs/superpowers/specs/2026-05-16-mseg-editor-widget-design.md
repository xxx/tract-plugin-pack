# MSEG Editor Widget — Design

**Date:** 2026-05-16
**Status:** Approved
**Crate:** `tiny-skia-widgets`

## Purpose

A full-featured MSEG (multi-stage envelope generator) editor widget for the
CPU-rendered nih-plug plugin GUIs in this workspace. It lets a user draw and
edit a free-form multi-segment envelope: add/move/delete nodes, bend segments,
draw stepped sections, snap to a grid, switch between time-based and
beat-synced timing, and generate randomized envelopes in several styles.

It is a generic, reusable widget in `tiny-skia-widgets` — like `dropdown` —
built on the crate's existing primitives (`draw_rect`, `TextRenderer`,
`DragState`, `DropdownState`, `TextEditState`).

## Scope

In scope:

- The `MsegData` document model (the editable, serializable envelope).
- A pure sampler (`value_at_phase`) and a pure playback-rule function
  (`advance`) — the single source of truth shared by rendering and any
  consuming plugin's DSP.
- The randomizer.
- The editor: rendering + interaction (`MsegEditState`, draw + event handlers),
  including a **curve-only mode** (see Editor: Curve-Only Mode) for consumers
  that use the MSEG purely as a static shape editor.
- Inline unit tests and render smoke tests; optionally a standalone example
  for live visual checks.

Out of scope:

- Wiring the widget into any plugin.
- A stateful DSP playback struct (`MsegPlayer`). Per the chosen architecture
  the consuming plugin owns a ~10-line phase loop built on `value_at_phase` +
  `advance`.
- Automatable rate/sync as nih-plug params — timing lives in `MsegData`; a
  plugin that wants automatable rate layers that on top.

## Architecture

The MSEG has three parts: the **data model** (edited and persisted), the
**sampler** (model + playhead → envelope value), and the **editor**
(rendering + interaction + randomizer).

DSP playback *state* does not live in the widget crate. `tiny-skia-widgets`
provides `MsegData` plus two **pure** functions — `value_at_phase` (raw shape)
and `advance` (loop/sustain playback rules). The consuming plugin persists
`MsegData` as plugin state, owns a phase counter it advances per sample, and
calls these functions — roughly ten lines plugin-side. Rendering and the DSP
both call `value_at_phase`, so the drawn curve and the heard envelope cannot
drift. This keeps the widget crate UI + pure data/functions, with no
audio-rate playback state (whose trigger/release semantics are
plugin-specific anyway).

Because `MsegData` is `Copy` and heap-free (see the data model), the
GUI→audio handoff is a plain lock-free copy — e.g. a triple buffer of
pre-allocated `MsegData` slots — that never allocates or deallocates on the
audio thread. The handoff mechanism itself is the plugin's concern; the
widget only guarantees the document is cheap and safe to copy.

## Module Layout

A new `mseg/` folder in `tiny-skia-widgets/src/`:

- `mseg/mod.rs` — model types + the pure sampler (tightly coupled; one unit).
- `mseg/randomize.rs` — the randomizer.
- `mseg/render.rs` — `draw_mseg` and drawing helpers.
- `mseg/editor.rs` — `MsegEditState` and event handlers.

`tiny-skia-widgets/src/lib.rs` gains `pub mod mseg;` and `pub use mseg::*;`.

## Data Model

```rust
/// The editable, serializable envelope document.
///
/// Fixed-capacity and `Copy` — no heap. This is deliberate: the GUI thread
/// edits it and the audio thread reads it. A `Copy`, `Vec`-free document
/// (~2 KB) crosses that boundary with a plain lock-free copy (e.g. a triple
/// buffer) and never allocates *or deallocates* on the audio thread — a
/// `Vec`-backed model would allocate on clone and free on drop, both
/// forbidden inside `process()`.
pub struct MsegData {
    /// Storage for up to `MAX_NODES` nodes; only `nodes[..node_count]` are
    /// active, ordered by time. `nodes[0].time == 0.0` and
    /// `nodes[node_count - 1].time == 1.0` (endpoints move in value only);
    /// interior nodes move freely in 0..1. Slots `>= node_count` are unused.
    pub nodes: [MsegNode; MAX_NODES],
    pub node_count: usize,
    pub play_mode: PlayMode,             // Triggered | Cyclic
    pub hold: HoldMode,                  // None | Sustain(idx) | Loop { start, end }
    pub sync_mode: SyncMode,             // Time | Beat — interprets the length
    pub time_seconds: f32,               // active length when sync_mode == Time
    pub beats: f32,                      // active length when sync_mode == Beat
    pub time_divisions: u32,             // horizontal grid: N divisions of the span
    pub value_steps: u32,                // vertical grid: N value levels
    pub snap: bool,
}

pub struct MsegNode {
    pub time: f32,    // 0..1, normalized phase position
    pub value: f32,   // 0..1, normalized level (plugin scales to its range)
    /// Shape of the segment FROM this node to the next (last node's unused).
    pub tension: f32, // -1..1, concave/convex bow
    pub stepped: bool,// segment is an instant jump + flat hold
}

pub enum PlayMode { Triggered, Cyclic }
pub enum SyncMode { Time, Beat }
pub enum HoldMode {
    None,
    Sustain(usize),               // node index — triggered: hold here until release
    Loop { start: usize, end: usize }, // node indices — loop this sub-range
}

pub const MAX_NODES: usize = 128;
```

- Coordinates are normalized: time 0..1, value 0..1. The widget is
  range-agnostic; the plugin scales the sampled value to its own range.
- Segment shape (`tension`, `stepped`) lives on the *starting* node — one
  `Vec`, no parallel arrays.
- `play_mode` (Triggered/Cyclic) and `sync_mode` (Time/Beat) are independent
  axes: `play_mode` is *how playback behaves*, `sync_mode` is *how the length
  is interpreted*.
- `hold` unifies sustain and loop. They are mutually exclusive — you cannot
  simultaneously hold one value and loop a range — so a single enum is the
  honest model.
- `MsegData`, `MsegNode`, and the enums derive `Copy + Clone + Serialize +
  Deserialize` (adds a `serde` dependency to `tiny-skia-widgets`; the plugins
  already use serde). To keep the persisted blob compact and sidestep any
  large-array serde limitation, the plan may give `MsegData` a small
  hand-written serde impl that (de)serializes only `nodes[..node_count]`.
- `MsegData::default()` is a sensible starting envelope: `node_count == 2` —
  `nodes[0] = (time 0.0, value 0.0)` and `nodes[1] = (time 1.0, value 1.0)` —
  a rising ramp; remaining slots default; `Triggered`;
  `SyncMode::Time`, `time_seconds 1.0`, `beats 1.0`; `time_divisions 16`,
  `value_steps 8`, `snap true`; `hold None`.

**Validity invariant** — `MsegData` is valid iff: `node_count` in
`2..=MAX_NODES`; the active nodes `nodes[..node_count]` are sorted strictly
ascending by time; `nodes[0].time == 0.0` and `nodes[node_count - 1].time ==
1.0`; all active `time`/`value` in 0..1; `tension` in -1..1; any `hold` node
indices are `< node_count` and (for `Loop`) `start < end`. Slots
`>= node_count` are not constrained. A debug-only
`MsegData::debug_assert_valid()` checks this; tests assert it after every
mutation path.

## Sampling & Playback

### Pure shape lookup

```rust
pub fn value_at_phase(data: &MsegData, phase: f32) -> f32
```

Clamps `phase` to 0..1, finds the segment `(n0, n1)` whose time range
straddles `phase`, then:

- if `n0.stepped` → return `n0.value` (flat hold across the whole segment;
  the discontinuity is the jump at the next node);
- else → `lerp(n0.value, n1.value, warp(t, n0.tension))`, where `t` is the
  local 0..1 position within the segment.

Used by both rendering (to draw the curve) and the consuming plugin's DSP.

### Tension curve

`warp(t, tension)` shapes the interpolation:

- `tension == 0.0` → linear (`t` unchanged).
- otherwise → exponential bow `(e^{k·t} − 1) / (e^k − 1)` with `k = tension *
  5.0`. Maps 0→0 and 1→1; positive tension = slow-start (concave), negative =
  fast-start (convex). The `5.0` constant is the curve's expressive range and
  is tunable.

### Playback rules

The consuming plugin owns `phase: f32` (0..1) and `released: bool`. The crate
provides one pure rule function:

```rust
pub fn advance(data: &MsegData, phase: f32, dt: f32, released: bool) -> (f32, bool)
// returns (next_phase, finished)
```

- **Cyclic** (`play_mode == Cyclic`): `phase + dt` wrapped within `[0, 1]`, or
  within the `Loop` region's node times if `hold` is `Loop`. `finished` is
  always `false`.
- **Triggered**: advances `phase + dt`.
  - `hold == Sustain(i)` and `!released`: clamp the result so it does not pass
    `nodes[i].time` (hold there).
  - `hold == Loop { start, end }` and `!released`: when the result reaches
    `nodes[end].time`, wrap back to `nodes[start].time` (loop while held).
  - once `released` (or `hold == None`): advance freely; when the result
    reaches 1.0, clamp to 1.0 and report `finished == true`.

The plugin's per-sample loop is then ~10 lines:

```rust
let v = value_at_phase(&data, phase);          // envelope value, 0..1
let (next, finished) = advance(&data, phase, dt, released);
phase = next;
```

`dt` is `1.0 / duration_in_samples`, where `duration_in_samples` is
`time_seconds * sample_rate` (Time mode) or `beats * (60/bpm) * sample_rate`
(Beat mode) — computed plugin-side from host tempo.

## Editor: Rendering

`draw_mseg(pixmap, text_renderer, rect, &MsegData, &MsegEditState, scale)`
draws the whole widget into `rect` (physical pixels). It composes:

- **Canvas** (the bulk of `rect`): the time + value grid lines; the envelope
  as an interpolated polyline (stepped segments draw flat-then-jump); a node
  dot per node; a small draggable tension handle at each non-stepped
  segment's midpoint; the hovered/selected node highlight; and an optional
  playhead line (the plugin passes a phase in, or `None`).
- **Marker lane**: a thin strip along the canvas top holding the `hold`
  marker(s) — a sustain triangle, or loop start/end brackets — depending on
  `hold`. A separate lane keeps the curve area uncluttered and the markers
  discoverable and draggable.
- **Control strip** (bottom, single packed strip — layout A): sync mode
  `Time|Beat`, a duration field, time-grid and value-grid size fields, a snap
  toggle, a hold-mode selector `None|Sustain|Loop`, and the randomizer (style
  selector + Randomize button). It reuses existing widgets — stepped selector,
  buttons, the `dropdown` widget for the style selector, and `TextEditState`
  numeric fields.

Rendering uses the crate's theme colors. All fills use `draw_rect` (opaque
fast path). The curve polyline is drawn as line segments; the planning step
decides whether direct pixel writes are warranted (the dropdown review
established that `draw_rect` with opaque colors is already efficient — no
premature pixel-pushing).

## Editor: Interaction

`MsegEditState` is transient editor state (not persisted), analogous to
`DragState` / the dropdown's state. It holds:

- the current drag target — `Node(idx) | Tension(seg_idx) | Marker(which) |
  StepDraw`;
- the hovered element (for highlight);
- the in-progress stepped-draw node buffer;
- a `stepped_draw` modifier flag (set by the plugin from whatever key it
  chooses — the widget does not hardcode a key);
- composed sub-widget state: a `DropdownState` for the style selector and a
  `TextEditState` for the numeric strip fields;
- a `curve_only` flag, set at construction (see Editor: Curve-Only Mode).

Event handlers — representative shape; exact parameters are settled in the
implementation plan:

```rust
impl MsegEditState {
    pub fn on_mouse_down(&mut self, x: f32, y: f32, data: &mut MsegData,
                         rect: (f32,f32,f32,f32), fine: bool) -> Option<MsegEdit>;
    pub fn on_mouse_move(&mut self, x: f32, y: f32, data: &mut MsegData,
                         rect: (f32,f32,f32,f32), fine: bool) -> Option<MsegEdit>;
    pub fn on_mouse_up(&mut self, data: &mut MsegData) -> Option<MsegEdit>;
    pub fn on_double_click(&mut self, x: f32, y: f32, data: &mut MsegData,
                           rect: (f32,f32,f32,f32)) -> Option<MsegEdit>;
    pub fn set_stepped_draw(&mut self, held: bool);
}

pub enum MsegEdit { Changed }
```

Unlike the dropdown (which reports a param selection), the MSEG editor *owns
a document*: handlers mutate `&mut MsegData` directly and return
`MsegEdit::Changed` when something changed, so the plugin knows to re-persist.

**Gestures:**

- **Add node** — click empty canvas → insert a node at the click (grid-snapped
  when `snap`); refused at `MAX_NODES`.
- **Move node** — drag a node. Interior nodes move in (time, value), clamped
  strictly between their neighbors in time; endpoints move in value only.
  Snaps to the grid; a `fine` modifier (the plugin passes its shift state)
  bypasses snap for precise adjustment, matching the crate's shift-for-fine
  convention.
- **Tension** — drag a segment's midpoint tension handle to bow it.
- **Delete node** — double-click a node; endpoints cannot be deleted.
- **Toggle stepped** — right-click a segment toggles its `stepped` flag.
- **Stepped-draw** — while the `stepped_draw` modifier is held, dragging across
  the canvas lays down grid-snapped stepped nodes following the cursor (like
  painting a step sequence); committed on mouse-up.
- **Hold markers** — the strip's hold-mode selector sets `None/Sustain/Loop`;
  the corresponding marker(s) appear in the marker lane and are dragged to
  snap onto nodes. Changing the count of nodes can invalidate marker indices
  — see the randomizer note; manual edits clamp markers similarly.

Snap, when on, applies to the time axis (grid divisions) and the value axis
(value steps) for node placement and movement.

## Editor: Curve-Only Mode

Some consumers use the MSEG purely as a static *shape* editor — a hand-drawn
curve with no playback (the `miff` plugin uses the curve as an FIR filter
kernel). For them the playback/timing UI is dead weight. The editor therefore
supports a **curve-only mode**.

`MsegEditState` carries a `curve_only` flag, fixed at construction:

- `MsegEditState::new()` — full editor (default).
- `MsegEditState::new_curve_only()` — curve-only editor.

`draw_mseg` and every event handler read the flag from `MsegEditState` — no
signature changes. In curve-only mode:

- The **playback / timing controls are omitted** from the bottom control
  strip — no `play_mode`, `sync_mode`, `hold-mode`, or duration controls.
  The grid (time + value divisions), snap toggle, and the randomizer (style +
  Randomize) **remain**.
- The **marker lane is not drawn and not interactive** — there are no
  sustain/loop markers (the `Marker` drag target is unreachable). The vertical
  space the marker lane would occupy is reclaimed by the canvas.
- All curve editing is unchanged: add/move/delete nodes, tension handles,
  toggle stepped, freehand stepped-draw, grid snapping.
- `MsegData`'s `play_mode` / `sync_mode` / `hold` fields still exist and
  serialize as normal; a curve-only consumer simply ignores them.

`draw_mseg`'s `rect` argument plus the absence of the marker lane and the
trimmed strip are the only layout differences; the canvas occupies the freed
space. Curve-only mode is exercised by its own render smoke test.

## Randomizer

`mseg/randomize.rs`:

```rust
pub enum RandomStyle { Smooth, Ramps, Stepped, Spiky, Chaos }

pub fn randomize(data: &mut MsegData, style: RandomStyle, seed: u32)
```

Regenerates `data.nodes` and `data.node_count` only — `play_mode`,
`sync_mode`, timing, and grid settings are left untouched. Deterministic given `seed` (a tiny private
xorshift PRNG, no new dependency — the same pattern the profiling harnesses
use). The editor bumps a seed counter on each "Randomize" click.

Node count is **not** a separate control — it falls out of grid + style:

- **Stepped / Spiky** — one node per time-grid cell (capped at `MAX_NODES`
  if the grid is set very fine); node values snapped to the value grid. The
  time-grid divisions *are* the density.
- **Smooth / Ramps** — a style-inherent sparse node count (~3–6); node times
  snapped to the time grid; values continuous.
- **Chaos** — randomizes the node count too.

Per-style character:

- **Smooth** — mid-range values, gentle non-zero tensions, no stepped segments.
- **Ramps** — linear segments (tension ≈ 0), values alternating up/down.
- **Stepped** — every segment `stepped: true`; a random step sequence.
- **Spiky** — values alternating between near-0 and near-1, short segments,
  a mix of sharp curves and steps.
- **Chaos** — everything randomized: count, per-node tension, stepped flags,
  values.

Generated node times always snap to the time grid when `data.snap` is on;
values snap to the value grid for Stepped and Spiky (the other styles keep
continuous values to stay expressive). Endpoints remain at time 0.0 and 1.0.
If regeneration changes the node count, any `hold` referencing a now-invalid
node index is reset to `HoldMode::None`.

## Public API

Re-exported from the `tiny-skia-widgets` crate root:

```rust
// model + sampler  (mseg/mod.rs)
struct MsegData;  struct MsegNode;
enum PlayMode;  enum SyncMode;  enum HoldMode;
const MAX_NODES;
MsegData::default() -> MsegData
value_at_phase(&MsegData, phase: f32) -> f32
advance(&MsegData, phase: f32, dt: f32, released: bool) -> (f32, bool)

// randomizer  (mseg/randomize.rs)
enum RandomStyle;
randomize(&mut MsegData, RandomStyle, seed: u32)

// editor  (mseg/render.rs, mseg/editor.rs)
struct MsegEditState;  enum MsegEdit;
MsegEditState::new() -> MsegEditState              // full editor
MsegEditState::new_curve_only() -> MsegEditState   // playback/marker UI hidden
draw_mseg(pixmap, text_renderer, rect, &MsegData, &MsegEditState, scale)
MsegEditState::on_mouse_down/on_mouse_move/on_mouse_up/on_double_click(...) -> Option<MsegEdit>
MsegEditState::set_stepped_draw(&mut self, held: bool)
```

## Testing

Inline `#[cfg(test)]` modules, consistent with the crate's existing widget
tests.

- **Sampler** (`mseg/mod.rs`): `value_at_phase` — linear segment; tension bow
  (monotonic, exact at endpoints); stepped segment (flat across); phase clamp
  outside 0..1; degenerate single segment. `advance` — cyclic wrap; triggered
  run-to-end with `finished`; sustain hold while `!released` then resume on
  release; loop wrap while held then exit on release. `warp` — 0→0, 1→1,
  monotonic for representative tensions.
- **Randomizer** (`mseg/randomize.rs`): determinism (same seed → identical
  output); per-style invariants — Stepped → every segment stepped and values
  on the value grid; Smooth → no stepped segments and sparse; node count
  within `2..=MAX_NODES`; nodes strictly time-ordered; endpoints at time 0.0
  and 1.0; `hold` reset when node count changes.
- **Editor** (`mseg/editor.rs`, `mseg/render.rs`): hit-testing math (which
  node / tension handle / marker is under a point); snap math; add / move /
  delete-node operations on `MsegData`; `MsegEditState` drag-target
  transitions; stepped-draw producing the expected node sequence. Render
  smoke tests — `draw_mseg` into a `Pixmap`, asserting non-panic and a
  sentinel painted pixel.
- **Invariant**: `MsegData::debug_assert_valid()` is asserted after every
  mutation path exercised in the editor and randomizer tests.

## File Changes

- New: `tiny-skia-widgets/src/mseg/mod.rs`, `mseg/randomize.rs`,
  `mseg/render.rs`, `mseg/editor.rs`.
- Modify: `tiny-skia-widgets/src/lib.rs` — `pub mod mseg;` + `pub use
  mseg::*;`.
- Modify: `tiny-skia-widgets/Cargo.toml` — add a `serde` dependency
  (`features = ["derive"]`) for `MsegData` persistence.
