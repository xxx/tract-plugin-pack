# Multosis Phase 2, Milestone 2c — Effect Editor & Tabbed Shell — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Multosis a persistent left-edge track listing and a per-track effect editor (effect kind, parameters, 3 MSEGs), with a live config handoff so GUI edits are heard immediately.

**Architecture:** `MultosisWindow` gains a `View` (Grid/Effect) and a `selected_track`; a new `track_list` module draws the always-visible left panel; a new `effect_editor` module draws the per-track editor. The grid shifts right by the panel width. The editor mutates the already-persisted `track_effects`/`track_modulation` mutexes and raises a `config_dirty` flag; `process()` re-bridges the config into the engine via `try_lock`. `set_effects` becomes incremental so parameter edits don't reset DSP state.

**Tech Stack:** Rust (nightly), nih-plug, `tiny-skia-widgets` (`dropdown`, `param_dial`, `controls`, `mseg`), `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message ends with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` lines below omit it — add it to each.

---

## Pre-existing state (170 multosis tests, all green)

- `multosis/src/editor.rs` — `MultosisWindow` (baseview handler: `draw`, `on_event`, `commit_click`, `paint_cells`, the toolbar/region handlers); `MultosisEditor`; `pub fn create(params, wavefront_display, seq_status, grid_handoff, reset_request) -> Option<Box<dyn Editor>>`. `pub const WINDOW_WIDTH: u32 = 1336; pub const WINDOW_HEIGHT: u32 = 758;`. The window holds `surface`, `physical_width/height`, `scale_factor`, `params`, `wavefront_display`, `seq_status`, `grid_handoff`, `mouse_pos`, `text_renderer`, `gui_context`, `reset_request`, `toolbar_drag`, `clipboard`, `rng_seed`, `left_gesture`, `grid_cache`.
- `multosis/src/editor/grid_view.rs` — layout constants `STATUS_H = 88.0`, `TOOLBAR_ROW_H = 44.0`, `CELL = 40.0`, `MARGIN = 16.0`, `GUTTER = 14.0`, `GROUP_SIZE = 8`, `GROUP_GAP = 8.0`. Geometry fns: `cell_rect(row, col, scale)`, `cell_at(px, py, scale)`, `cell_zone(...)`, `region_handle_hit(...)`, `column_at(px, scale)`, `row_at(py, scale)`, `region_grip_rect/hit`. `GridCache`. Draw fns: `draw_grid_cells`, `draw_region_overlay`, `draw_wavefront`, `draw_status`. The grid's logical X origin is `MARGIN`; X is computed in `cell_rect` (`x = (MARGIN + col*CELL + group*GROUP_GAP)*scale`), `cell_at` / `column_at` (`lx = px/scale - MARGIN`), and `region_handle_hit` (`grid_left = MARGIN*scale`, `grid_right = (MARGIN + COLS*CELL + (COLS/GROUP_SIZE-1)*GROUP_GAP)*scale`).
- `multosis/src/editor/toolbar.rs` — `ToolbarControl`, `ToolbarOp`, `control_rect`, `toolbar_hit`, `op_rect`, `op_hit`, `apply_grid_op`, `draw_toolbar`. The toolbar remaps a logical 1044-unit span into `[MARGIN, WINDOW_WIDTH - MARGIN]`, so it auto-rescales to a wider `WINDOW_WIDTH` and stays full-width.
- `multosis/src/effects.rs` — `Effect` trait (`process_sample`, `set_sample_rate`, `reset`, `parameters(&self) -> &'static [ParamSpec]`, `set_param(&mut self, usize, f32)`); `ParamSpec { name: &'static str, min: f32, max: f32, default: f32 }` (`Copy`); `EffectKind { Lowpass, Bitcrush }` (`Copy`, `PartialEq`, serde) with `ALL: [EffectKind; 2]` and `name(self) -> &'static str`; `EffectInstance` (`new(kind)`, `kind() -> EffectKind`, implements `Effect`); `MAX_EFFECT_PARAMS = 4`; `TrackEffect { kind: EffectKind, params: [f32; MAX_EFFECT_PARAMS] }` (`Copy`, serde), `TrackEffect::default_for_row(row)`.
- `multosis/src/modulation.rs` — `TrackModulation { msegs: [MsegData; 3], targets: [Option<usize>; 2], depths: [f32; 2] }` (serde), `default_for_row(row)`; `Modulation`.
- `multosis/src/engine.rs` — `AudioEngine { propagator, clock, effects: [EffectInstance; ROWS], sample_rate, track_effects: [TrackEffect; ROWS], modulation: Modulation }`; `ROWS = 16` (`use crate::grid::{... ROWS}`). `new()`, `set_effects(&mut self, config: &[TrackEffect; ROWS])` (rebuilds every `EffectInstance`), `set_modulation(&mut self, config: &[TrackModulation; ROWS])`, `set_sample_rate`, `reset`, `wavefront()`, `sequence_state()`, `step()`, `active_rows(grid, wf) -> u16` (a private associated fn), `process_sample`, `process(left, right, playing, samples_per_step, bpm, mix, auto_restart, grid)`. Inside `process`, a local `active: u16` is recomputed by `Self::active_rows(grid, &self.propagator.wavefront)` at the start and after each boundary `tick`.
- `multosis/src/lib.rs` — `MultosisParams` (`#[persist]` `editor_state`, `grid`, `track_effects`, `track_modulation`; params `speed`, `mix`, `output_gain`, `auto_restart`); `MultosisParams::default()` sets `editor_state: tiny_skia_widgets::EditorState::from_size(1336, 758)`. `Multosis` plugin struct holds `params`, `grid_handoff`, `grid`, `engine`, `sample_rate`, `was_playing`, `wavefront_display`, `seq_status`, `reset_request`. `editor()` calls `editor::create(...)`. `initialize()` bridges `grid` / `track_effects` / `track_modulation` into the engine. `process()` reads transport, handles `reset_request` (an `Arc<AtomicBool>`, `.swap(false, Relaxed)`), the grid handoff, runs `engine.process(...)`, publishes the wavefront + seq status, applies output gain.
- `tiny-skia-widgets` (imported as `use tiny_skia_widgets as widgets;`) — `widgets::dropdown::{DropdownState, DropdownEvent, draw_dropdown_trigger, draw_dropdown_popup}`; `widgets::param_dial::{draw_dial, draw_dial_ex}`; `widgets::controls::{draw_stepped_selector, draw_button}`; `widgets::mseg::{MsegData, MsegEditState, MsegEdit, draw_mseg}`; `widgets::TextRenderer`; `widgets::DragState<A>`.
  - `DropdownState<A: Copy + PartialEq>::new()`; `is_open()`, `is_open_for(A)`; `open(action: A, anchor: (f32,f32,f32,f32), items: &[&str], current: usize, filter_enabled: bool, window_size: (f32,f32))`; `close()`; `on_mouse_down(x, y, items, window_size) -> Option<DropdownEvent<A>>`; `on_mouse_move(...)`; `on_mouse_up()`. `DropdownEvent<A> { Closed(A), HighlightChanged(A, usize), Selected(A, usize) }`. `draw_dropdown_trigger(pixmap, tr, rect, label, is_open)`; `draw_dropdown_popup(pixmap, tr, &state, items, window_size)` (no-op when closed).
  - `draw_dial(pixmap, tr, cx, cy, radius, label, value_text, normalized: f32)` — `normalized` in `[0,1]`. (Drag interaction is the consumer's job.)
  - `draw_stepped_selector(pixmap, tr, x, y, w, h, options: &[&str], active_index: usize)`.
  - `draw_button(pixmap, tr, x, y, w, h, label, active: bool, _hovered: bool)`.
  - `MsegEditState::new()` (full editor); `on_mouse_down(x, y, &mut MsegData, rect, scale, fine) -> Option<MsegEdit>`, `on_mouse_move(...)`, `on_mouse_up(&mut MsegData) -> Option<MsegEdit>`, `on_double_click(x, y, &mut MsegData, rect, scale)`, `on_right_click(x, y, &mut MsegData, rect, scale)`. `MsegEdit::Changed` is the only variant; check `== Some(MsegEdit::Changed)`. `draw_mseg(pixmap, tr, rect, &MsegData, &MsegEditState, scale)`. `mseg_layout`, `phase_to_x`, `value_to_y` are public in `widgets::mseg`.

All geometry below is in **logical** units unless a function name or comment says "physical"; physical = logical × `scale`.

---

### Task 1: Grid coordinate shift — make room for the track panel

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

The grid moves right by a fixed panel width. Nothing fills the new left strip yet (Task 3/4 do) — an empty strip is the expected intermediate state.

- [ ] **Step 1: Write the failing test**

Add to `grid_view.rs`'s `#[cfg(test)] mod tests` block (create the block at the end of the file if it does not exist — check first; if it exists, append):

```rust
    #[test]
    fn cell_rect_is_offset_by_the_track_panel() {
        // Cell (0,0)'s left edge sits a full panel-width past the margin.
        let (x, _, _, _) = cell_rect(0, 0, 1.0);
        assert!((x - (MARGIN + TRACK_PANEL_W)).abs() < 1e-3, "got {x}");
    }

    #[test]
    fn cell_at_round_trips_through_the_panel_offset() {
        // A point in the middle of cell (3, 5) maps back to (3, 5).
        let (x, y, w, h) = cell_rect(3, 5, 1.0);
        assert_eq!(cell_at(x + w / 2.0, y + h / 2.0, 1.0), Some((3, 5)));
        // A point left of the grid (in the panel strip) is off-grid.
        assert_eq!(cell_at((MARGIN + 1.0) * 1.0, y + h / 2.0, 1.0), None);
    }

    #[test]
    fn column_at_accounts_for_the_panel_offset() {
        let (x, _, w, _) = cell_rect(0, 9, 1.0);
        assert_eq!(column_at(x + w / 2.0, 1.0), 9);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(cell_rect_is_offset) + test(cell_at_round_trips) + test(column_at_accounts)'`
Expected: build failure — `cannot find value TRACK_PANEL_W in this scope`.

- [ ] **Step 3: Write minimal implementation**

In `grid_view.rs`, add the constant beside the other layout constants (after `GROUP_GAP`):

```rust
/// Logical width of the always-visible left-edge track listing panel
/// (`editor::track_list`). The grid is drawn to its right.
pub const TRACK_PANEL_W: f32 = 120.0;
```

In `cell_rect`, change the `x` line to include the panel offset:

```rust
    let x = (MARGIN + TRACK_PANEL_W + col as f32 * CELL + group * GROUP_GAP) * scale;
```

In `cell_at`, change the `lx` line:

```rust
    let lx = px / scale - MARGIN - TRACK_PANEL_W;
```

In `column_at`, change the `lx` line:

```rust
    let lx = px / scale - MARGIN - TRACK_PANEL_W;
```

In `region_handle_hit`, change `grid_left` and `grid_right`:

```rust
    let grid_left = (MARGIN + TRACK_PANEL_W) * scale;
    let grid_right = (MARGIN
        + TRACK_PANEL_W
        + COLS as f32 * CELL
        + (COLS / GROUP_SIZE - 1) as f32 * GROUP_GAP)
        * scale;
```

In `editor.rs`, widen the window and recompute the documented arithmetic comment. Replace the `WINDOW_WIDTH` constant and its doc comment:

```rust
/// Editor window size. Derived from the grid layout in `grid_view`:
/// width  = 2*MARGIN + TRACK_PANEL_W + COLS*CELL + 3*GROUP_GAP
///        = 16 + 120 + 1280 + 24 + 16 = 1456
/// height = STATUS_H + GUTTER + ROWS*CELL + MARGIN = 88 + 14 + 640 + 16 = 758
/// (kept in sync by the `window_size_matches_the_grid` test).
pub const WINDOW_WIDTH: u32 = 1456;
pub const WINDOW_HEIGHT: u32 = 758;
```

In `lib.rs`, update the persisted default editor size in `MultosisParams::default()`:

```rust
            editor_state: tiny_skia_widgets::EditorState::from_size(1456, 758),
```

If `toolbar.rs` or `grid_view.rs` has a `window_size_matches_the_grid` test, update its expected width to `1456` (search for `1336` across `multosis/src/` and fix every occurrence that means the window width — there is a layout-consistency test that hardcodes it).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis`
Expected: PASS — 173 tests (170 + 3 new). If a layout-consistency test fails on the old `1336`, fix the expected value to `1456` and re-run.
Run: `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): shift the grid right to make room for the track panel"
```

---

### Task 2: Publish the per-track "currently sounding" mask

**Files:**
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/lib.rs`
- Modify: `multosis/src/editor.rs`

The engine retains the last block's active-row mask; the plugin publishes it to an `Arc<AtomicU16>` the editor will read in Task 3.

- [ ] **Step 1: Write the failing test**

Add to `engine.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn active_mask_reports_the_last_blocks_active_rows() {
        // A fresh engine has processed nothing — mask is empty.
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        assert_eq!(engine.active_mask(), 0);
        // After a playing block on the default grid, some rows are active.
        let grid = Grid::default_routing();
        let mut left = [0.2_f32; 128];
        let mut right = [0.2_f32; 128];
        engine.process(&mut left, &mut right, true, 10.0, 120.0, 1.0, true, &grid);
        assert!(
            engine.active_mask() != 0,
            "after arming, at least one row should be active"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(active_mask_reports)'`
Expected: build failure — `no method named active_mask`.

- [ ] **Step 3: Write minimal implementation**

In `engine.rs`, add a field to `AudioEngine` (after `modulation`):

```rust
    /// The most recent process block's active-row bitmask (bit `r` = row `r`
    /// had a lit, enabled cell under the wavefront). Published to the editor.
    last_active: u16,
```

In `AudioEngine::new()`, initialise it: `last_active: 0,`.

In `process`, after the segment `while` loop ends (just before the closing brace of `process`), record the final `active`:

```rust
        self.last_active = active;
```

Add the getter (after `step()`):

```rust
    /// The last process block's active-row bitmask — bit `r` set when row `r`
    /// had a lit, enabled cell under the wavefront. For the editor's track
    /// listing "currently sounding" indicator.
    pub fn active_mask(&self) -> u16 {
        self.last_active
    }
```

In `lib.rs`, add `use std::sync::atomic::AtomicU16;` to the imports (the file already imports from `std::sync`; add `atomic::{AtomicU16}` — or a fresh `use std::sync::atomic::AtomicU16;` line). Add a field to the `Multosis` struct (after `reset_request`):

```rust
    /// Audio→GUI mirror of the engine's active-row mask, shared with the editor.
    active_rows: Arc<AtomicU16>,
```

Initialise it in `impl Default for Multosis` (after `reset_request`):

```rust
            active_rows: Arc::new(AtomicU16::new(0)),
```

In `process()`, after the `seq_status.publish(...)` call, publish the mask:

```rust
        self.active_rows.store(
            self.engine.active_mask(),
            std::sync::atomic::Ordering::Relaxed,
        );
```

In `editor()`, pass it to `create`:

```rust
        editor::create(
            self.params.clone(),
            self.wavefront_display.clone(),
            self.seq_status.clone(),
            self.grid_handoff.clone(),
            self.reset_request.clone(),
            self.active_rows.clone(),
        )
```

In `editor.rs`, extend `create`'s signature and the `MultosisEditor` struct. Add `use std::sync::atomic::AtomicU16;` (the file already has `AtomicBool, AtomicU64`). Add a field to `MultosisEditor`:

```rust
    active_rows: Arc<AtomicU16>,
```

Update `create` to take and store it:

```rust
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    seq_status: Arc<SeqStatusDisplay>,
    grid_handoff: Arc<GridHandoff>,
    reset_request: Arc<AtomicBool>,
    active_rows: Arc<AtomicU16>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        seq_status,
        grid_handoff,
        reset_request,
        active_rows,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}
```

In `MultosisEditor::spawn`, clone `active_rows` alongside the other fields and pass it into `MultosisWindow::new`. Add an `active_rows: Arc<AtomicU16>` field to `MultosisWindow` and an `active_rows` parameter to `MultosisWindow::new` (place it after `reset_request`, before `scale_factor`); store it. The window does not yet read it — Task 3 draws with it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(active_mask_reports)'` — PASS.
Run: `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/engine.rs multosis/src/lib.rs multosis/src/editor.rs
git commit -m "feat(multosis): publish the engine's active-row mask to the editor"
```

---

### Task 3: The track listing module — layout, hit-testing, draw

**Files:**
- Create: `multosis/src/editor/track_list.rs`
- Modify: `multosis/src/editor.rs`

A new module: the geometry of the 16-entry left panel and its draw. Not yet wired into the window's `draw`/`on_event` (Task 4 does that).

- [ ] **Step 1: Write the failing test**

Create `multosis/src/editor/track_list.rs`:

```rust
//! The always-visible left-edge track listing — Phase 2 Milestone 2c. One
//! entry per track row: number, effect-kind name, and a "currently sounding"
//! dot. Clicking an entry opens that track's effect editor.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §2.

use crate::editor::grid_view::{CELL, GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::effects::EffectKind;
use crate::grid::ROWS;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// The physical-pixel rectangle `(x, y, w, h)` of track-listing entry `row`
/// at `scale`. Entries align vertically with the grid rows.
pub fn track_entry_rect(row: usize, scale: f32) -> (f32, f32, f32, f32) {
    let x = MARGIN * scale;
    let y = (STATUS_H + GUTTER + row as f32 * CELL) * scale;
    (x, y, TRACK_PANEL_W * scale, CELL * scale)
}

/// The track-listing entry under physical-pixel point `(px, py)` at `scale`,
/// or `None` if the point is outside the panel.
pub fn track_at(px: f32, py: f32, scale: f32) -> Option<usize> {
    if scale <= 0.0 {
        return None;
    }
    for row in 0..ROWS {
        let (x, y, w, h) = track_entry_rect(row, scale);
        if px >= x && px < x + w && py >= y && py < y + h {
            return Some(row);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_at_round_trips_each_entry() {
        for row in 0..ROWS {
            let (x, y, w, h) = track_entry_rect(row, 1.5);
            assert_eq!(track_at(x + w / 2.0, y + h / 2.0, 1.5), Some(row));
        }
    }

    #[test]
    fn track_at_misses_outside_the_panel() {
        // Above the first entry (in the toolbar strip) — no hit.
        assert_eq!(track_at(MARGIN + 1.0, 1.0, 1.0), None);
        // Right of the panel (over the grid) — no hit.
        let (x, y, w, h) = track_entry_rect(0, 1.0);
        assert_eq!(track_at(x + w + 50.0, y + h / 2.0, 1.0), None);
    }
}
```

Register the module in `editor.rs` — beside `pub mod grid_view; pub mod toolbar;` add:

```rust
pub mod track_list;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(track_at_round_trips) + test(track_at_misses)'`
Expected: it should actually PASS once the file compiles (the tested fns are written in Step 1). First confirm it *compiles and the tests pass* — if `cargo build -p multosis` fails, the module is not wired in. This task's "failing" state is the missing `draw_track_list` referenced in Step 3; proceed.

- [ ] **Step 3: Write minimal implementation**

Add the draw function to `track_list.rs` (after `track_at`, before the test module):

```rust
/// Draw the track listing into `pixmap`. `kinds[r]` is row `r`'s effect kind;
/// bit `r` of `active_mask` lights row `r`'s "sounding" dot; `selected` is the
/// row highlighted while the effect editor is open (`None` in the grid view).
pub fn draw_track_list(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    kinds: &[EffectKind; ROWS],
    active_mask: u16,
    selected: Option<usize>,
    scale: f32,
) {
    use tiny_skia::Color;
    let panel_bg = Color::from_rgba8(0x15, 0x12, 0x0F, 0xFF);
    let sel_bg = Color::from_rgba8(0x3A, 0x2F, 0x22, 0xFF);
    let border = Color::from_rgba8(0x3A, 0x34, 0x2E, 0xFF);
    let num_col = Color::from_rgba8(0x6A, 0x60, 0x52, 0xFF);
    let name_col = Color::from_rgba8(0x9A, 0x8A, 0x70, 0xFF);
    let sel_col = Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF);
    let dot_dark = Color::from_rgba8(0x2A, 0x24, 0x1E, 0xFF);
    let dot_live = Color::from_rgba8(0x5F, 0xC9, 0x6A, 0xFF);

    let text_size = 11.0 * scale;
    for row in 0..ROWS {
        let (x, y, w, h) = track_entry_rect(row, scale);
        let is_sel = selected == Some(row);
        widgets::draw_rect(pixmap, x, y, w, h, if is_sel { sel_bg } else { panel_bg });
        // bottom hairline
        widgets::draw_rect(pixmap, x, y + h - scale, w, scale, border);
        // Vertically centred text baseline — same formula `draw_button` uses.
        let ty = y + (h + text_size) * 0.5 - 2.0;
        // track number (1-based)
        tr.draw_text(
            pixmap,
            x + 6.0 * scale,
            ty,
            &format!("{}", row + 1),
            text_size,
            if is_sel { sel_col } else { num_col },
        );
        // effect kind name
        tr.draw_text(
            pixmap,
            x + 30.0 * scale,
            ty,
            kinds[row].name(),
            text_size,
            if is_sel { sel_col } else { name_col },
        );
        // "sounding" dot — a small square at the right edge
        let lit = (active_mask >> row) & 1 != 0;
        let d = 8.0 * scale;
        widgets::draw_rect(
            pixmap,
            x + w - 16.0 * scale,
            y + (h - d) / 2.0,
            d,
            d,
            if lit { dot_live } else { dot_dark },
        );
    }
}
```

The primitives above are verified against `tiny-skia-widgets`: `widgets::draw_rect(pixmap, x, y, w, h, Color)` (crate-root re-export, as `grid_view.rs` uses it); `TextRenderer::draw_text(pixmap, x, y, text, size, Color)` — note the argument order and that `y` is the text **baseline** (as `toolbar.rs` calls it); custom colours via `tiny_skia::Color::from_rgba8(r, g, b, a)` (as `grid_view.rs` does). There is no circle primitive — the "sounding" indicator is a small filled square.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(track_at)'` — PASS (2 tests).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/track_list.rs multosis/src/editor.rs
git commit -m "feat(multosis): add the track-listing panel module"
```

---

### Task 4: View state — draw the panel, route events, switch views

**Files:**
- Modify: `multosis/src/editor.rs`

The window gains a `View` and a `selected_track`; `draw` always draws the toolbar + track listing and dispatches the main area by view; clicking a track-list entry opens the (placeholder) Effect view. The effect editor itself is Tasks 8–9 — for now `View::Effect` draws a simple "Back to Grid" bar so the view switch is verifiable.

- [ ] **Step 1: Write the failing test**

`MultosisWindow` is not unit-testable (it owns a baseview surface), so this task is verified by `cargo build` + the manual check in Task 10. Add a small pure helper and test it instead. Add to `editor.rs` a free function and a test module entry (create the `#[cfg(test)] mod tests` block at the end of `editor.rs` if absent):

```rust
/// Clamp a candidate selected-track index into `0..ROWS`.
fn clamp_track(row: usize) -> usize {
    row.min(crate::grid::ROWS - 1)
}
```

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_track_keeps_indices_in_range() {
        assert_eq!(clamp_track(0), 0);
        assert_eq!(clamp_track(15), 15);
        assert_eq!(clamp_track(99), 15);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(clamp_track_keeps)'`
Expected: build failure — `cannot find function clamp_track` (until Step 3 keeps it; the test is mostly a smoke check that the new code compiles).

- [ ] **Step 3: Write minimal implementation**

In `editor.rs`, add the view enum near `LeftGesture`:

```rust
/// Which screen the window's main area shows. The toolbar and track listing
/// are drawn in both; only the main area to the right of the panel swaps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Grid,
    Effect,
}
```

Add two fields to `MultosisWindow` (after `grid_cache`):

```rust
    /// Which screen is showing.
    view: View,
    /// The track the Effect view edits (`0..ROWS`).
    selected_track: usize,
```

Initialise them in `MultosisWindow::new` — `view: View::Grid, selected_track: 0,`.

Add a helper method to `impl MultosisWindow`:

```rust
    /// The track-effect kinds, for the track listing. Reads the persisted
    /// config; falls back to row defaults on lock contention.
    fn track_kinds(&self) -> [crate::effects::EffectKind; crate::grid::ROWS] {
        if let Ok(cfg) = self.params.track_effects.lock() {
            std::array::from_fn(|r| cfg[r].kind)
        } else {
            std::array::from_fn(|r| {
                crate::effects::TrackEffect::default_for_row(r).kind
            })
        }
    }
```

Rework `MultosisWindow::draw`. The current `draw` clears via the grid cache blit and draws the grid + overlays + toolbar. Restructure so the toolbar and track listing always draw, and the main area dispatches:

```rust
    fn draw(&mut self) {
        match self.view {
            View::Grid => {
                let grid = self.params.grid.lock().map(|g| *g).unwrap_or_default();
                self.grid_cache.update(&grid, self.scale_factor);
                self.surface
                    .pixmap
                    .data_mut()
                    .copy_from_slice(self.grid_cache.pixmap().data());
                grid_view::draw_region_overlay(
                    &mut self.surface.pixmap,
                    &grid,
                    self.scale_factor,
                    Some(self.mouse_pos),
                );
                grid_view::draw_wavefront(
                    &mut self.surface.pixmap,
                    &self.wavefront_display,
                    self.scale_factor,
                );
            }
            View::Effect => {
                // The grid cache already paints the full window background;
                // reuse it as the backdrop, then draw the effect editor over
                // the main area. (Task 8 replaces this with the real editor.)
                let grid = self.params.grid.lock().map(|g| *g).unwrap_or_default();
                self.grid_cache.update(&grid, self.scale_factor);
                self.surface
                    .pixmap
                    .data_mut()
                    .copy_from_slice(self.grid_cache.pixmap().data());
            }
        }
        // Track listing — both views.
        let kinds = self.track_kinds();
        let active = self
            .active_rows
            .load(std::sync::atomic::Ordering::Relaxed);
        let selected = match self.view {
            View::Grid => None,
            View::Effect => Some(self.selected_track),
        };
        track_list::draw_track_list(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &kinds,
            active,
            selected,
            self.scale_factor,
        );
        // Toolbar — both views.
        toolbar::draw_toolbar(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &self.params,
            &self.seq_status,
            self.scale_factor,
        );
    }
```

Add the `clamp_track` helper and `#[cfg(test)] mod tests` block from Step 1.

In `on_event`, route by view. In the `ButtonPressed` Left arm, the dispatch currently checks the toolbar, then ops, then region handles, then grid cells. Restructure the press handling: after the toolbar/op checks (which apply in both views), check the **track listing** first (both views):

```rust
                    None => {
                        if let Some(row) = track_list::track_at(px, py, self.scale_factor) {
                            self.selected_track = clamp_track(row);
                            self.view = View::Effect;
                        } else if self.view == View::Grid {
                            // existing grid: region handle / region move / cell pending
                            if let Some(handle) = self.region_handle_under_cursor() {
                                self.left_gesture = Some(LeftGesture::ResizeRegion(handle));
                            } else if self.try_begin_region_move() {
                            } else if let Some((row, col, zone)) =
                                grid_view::cell_zone(px, py, self.scale_factor)
                            {
                                self.left_gesture =
                                    Some(LeftGesture::GridPending { row, col, zone });
                            }
                        }
                        // View::Effect main-area hits are wired in Task 8.
                    }
```

Guard the **right-click** handler (`handle_grid_click`) and the **CursorMoved** grid/region/paint gestures so they only run in `View::Grid` — wrap their bodies in `if self.view == View::Grid`. The toolbar drag handling stays unconditional (the toolbar is in both views).

For `View::Effect`, the only interaction this task adds is a temporary "Back to Grid" affordance. Draw a back button in the `View::Effect` arm of `draw` and hit-test it. Add, in the `View::Effect` draw arm, after the grid-cache blit:

```rust
                // Temporary "Back to Grid" button — replaced by the real
                // editor bar in Task 8.
                let bx = (grid_view::MARGIN + grid_view::TRACK_PANEL_W) * self.scale_factor;
                let by = (grid_view::STATUS_H + grid_view::GUTTER) * self.scale_factor;
                widgets::draw_button(
                    &mut self.surface.pixmap,
                    &mut self.text_renderer,
                    bx,
                    by,
                    90.0 * self.scale_factor,
                    26.0 * self.scale_factor,
                    "< Grid",
                    false,
                    false,
                );
```

(If `widgets::draw_button` is not re-exported at the crate root, use `widgets::controls::draw_button` — confirm against `tiny-skia-widgets/src/lib.rs`'s `pub use`.)

And in `on_event`'s Left `ButtonPressed`, before the `None =>` track-list branch, when `self.view == View::Effect`, hit-test that button rect and return to the grid:

```rust
            // inside the press dispatch, when toolbar/op miss:
            if self.view == View::Effect {
                let bx = (grid_view::MARGIN + grid_view::TRACK_PANEL_W) * self.scale_factor;
                let by = (grid_view::STATUS_H + grid_view::GUTTER) * self.scale_factor;
                let bw = 90.0 * self.scale_factor;
                let bh = 26.0 * self.scale_factor;
                if px >= bx && px < bx + bw && py >= by && py < by + bh {
                    self.view = View::Grid;
                    return baseview::EventStatus::Captured;
                }
            }
```

Place this check so it runs after the toolbar/op miss and before the track-list check (returning to the grid takes priority over re-selecting a track). Keep the structure clean — the implementer may factor the back-button rect into a small helper `fn back_button_rect(&self) -> (f32,f32,f32,f32)` used by both draw and hit-test (DRY).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis` — PASS (177 tests: 176 + `clamp_track`).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor.rs
git commit -m "feat(multosis): add the view shell — grid/effect routing + track panel"
```

---

### Task 5: The live config handoff — incremental `set_effects` + dirty flag

**Files:**
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/lib.rs`
- Modify: `multosis/src/editor.rs`

`set_effects` becomes incremental (kind-unchanged → keep the instance, just re-apply params; kind-changed → rebuild). A `config_dirty` flag drives a re-bridge in `process()`.

- [ ] **Step 1: Write the failing test**

Add to `engine.rs`'s test module:

```rust
    #[test]
    fn set_effects_preserves_dsp_state_when_kind_is_unchanged() {
        // A lowpass with running state; re-applying the same kind with a new
        // cutoff must not reset the filter (no zeroed history => no transient).
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(|_| crate::effects::TrackEffect {
                kind: crate::effects::EffectKind::Lowpass,
                params: [800.0, 0.2, 0.0, 0.0],
            });
        engine.set_effects(&cfg);
        // Drive row 0's effect so it has non-zero internal state.
        for _ in 0..256 {
            engine.effects_mut_for_test(0).process_sample(1.0, 1.0);
        }
        let before = engine.effects_mut_for_test(0).process_sample(1.0, 1.0).0;
        // Re-apply with only a parameter change, same kind.
        cfg[0].params[0] = 900.0;
        engine.set_effects(&cfg);
        let after = engine.effects_mut_for_test(0).process_sample(1.0, 1.0).0;
        // Continuity: state was preserved, so the two consecutive samples are
        // close — a full rebuild would have snapped `after` toward 0.
        assert!(
            (after - before).abs() < 0.2,
            "kind-unchanged set_effects must keep DSP state: {before} -> {after}"
        );
    }

    #[test]
    fn set_effects_rebuilds_on_a_kind_change() {
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        let mut cfg: [crate::effects::TrackEffect; ROWS] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        cfg[0] = crate::effects::TrackEffect {
            kind: crate::effects::EffectKind::Lowpass,
            params: [2000.0, 0.1, 0.0, 0.0],
        };
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Lowpass
        );
        cfg[0].kind = crate::effects::EffectKind::Bitcrush;
        engine.set_effects(&cfg);
        assert_eq!(
            engine.effects_mut_for_test(0).kind(),
            crate::effects::EffectKind::Bitcrush
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(set_effects_preserves) + test(set_effects_rebuilds)'`
Expected: build failure — `no method named effects_mut_for_test`; and `set_effects_preserves...` would fail anyway since today's `set_effects` rebuilds.

- [ ] **Step 3: Write minimal implementation**

In `engine.rs`, add a test-only accessor to `impl AudioEngine`:

```rust
    /// Test-only: mutable access to a row's live effect instance.
    #[cfg(test)]
    pub fn effects_mut_for_test(&mut self, row: usize) -> &mut EffectInstance {
        &mut self.effects[row]
    }
```

Rewrite `set_effects` to be incremental:

```rust
    /// Bridge `config` into the engine. For each row: if the effect kind is
    /// unchanged, the live instance is kept and only its parameters are
    /// re-applied (DSP state survives — a parameter edit does not click); a
    /// kind change rebuilds that row's instance. `track_effects` is stored
    /// unconditionally so the modulation engine reads fresh base values.
    pub fn set_effects(&mut self, config: &[TrackEffect; ROWS]) {
        for (r, cfg) in config.iter().enumerate() {
            if self.effects[r].kind() != cfg.kind {
                self.effects[r] = EffectInstance::new(cfg.kind);
                self.effects[r].set_sample_rate(self.sample_rate);
            }
            for i in 0..self.effects[r].parameters().len() {
                self.effects[r].set_param(i, cfg.params[i]);
            }
        }
        self.track_effects = *config;
    }
```

(Confirm the current `set_effects` body and match its existing `set_sample_rate` handling — the new version applies the sample rate on rebuild; the old version called `e.set_sample_rate(self.sample_rate)` for every row. Applying it only on rebuild is correct because a kept instance already has the right sample rate. If `AudioEngine::new()` relies on `set_effects` to seed the initial array, leave `new()`'s own construction untouched — it builds the array directly.)

In `lib.rs`, add a `config_dirty` field to the `Multosis` struct (after `active_rows`):

```rust
    /// Set by the editor on any effect/modulation edit; consumed by `process`
    /// to re-bridge the persisted config into the engine.
    config_dirty: Arc<std::sync::atomic::AtomicBool>,
```

Initialise it in `impl Default for Multosis`:

```rust
            config_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
```

In `process()`, before the `engine.process(...)` call (after the grid handoff `try_read`), add the re-bridge:

```rust
        // Re-bridge edited config into the engine. Clear the dirty flag only
        // after a successful re-bridge so no edit is lost on lock contention.
        if self
            .config_dirty
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            if let (Ok(eff), Ok(modu)) = (
                self.params.track_effects.try_lock(),
                self.params.track_modulation.try_lock(),
            ) {
                self.engine.set_effects(&eff);
                self.engine.set_modulation(&modu);
                self.config_dirty
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
```

In `editor()`, pass `config_dirty` to `create`:

```rust
        editor::create(
            self.params.clone(),
            self.wavefront_display.clone(),
            self.seq_status.clone(),
            self.grid_handoff.clone(),
            self.reset_request.clone(),
            self.active_rows.clone(),
            self.config_dirty.clone(),
        )
```

In `editor.rs`, add `config_dirty: Arc<AtomicBool>` to `MultosisEditor` and to `MultosisWindow`; thread it through `create` (new last parameter), `spawn`, and `MultosisWindow::new` (after `active_rows`). Add a helper to `impl MultosisWindow` that the effect editor will call after every edit:

```rust
    /// Mark the persisted effect/modulation config dirty so the audio thread
    /// re-bridges it on the next process block.
    fn mark_config_dirty(&self) {
        self.config_dirty
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(set_effects_preserves) + test(set_effects_rebuilds)'` — PASS.
Run: `cargo nextest run -p multosis` — PASS (179 tests).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/engine.rs multosis/src/lib.rs multosis/src/editor.rs
git commit -m "feat(multosis): live config handoff — incremental set_effects + dirty flag"
```

---

### Task 6: Effect-kind switch logic

**Files:**
- Modify: `multosis/src/effects.rs`
- Modify: `multosis/src/modulation.rs`

Pure logic, used by the EFFECT-section dropdown in Task 8: changing a track's effect kind resets its parameters to the new kind's defaults and clears any now-out-of-range MSEG target.

- [ ] **Step 1: Write the failing test**

Add to `effects.rs`'s test module:

```rust
    #[test]
    fn default_params_for_kind_matches_the_kinds_specs() {
        let lp = default_params_for_kind(EffectKind::Lowpass);
        assert_eq!(lp[0], LowpassEffect::new().parameters()[0].default);
        assert_eq!(lp[1], LowpassEffect::new().parameters()[1].default);
        // Slots past the kind's parameter count are zero.
        assert_eq!(lp[2], 0.0);
        assert_eq!(lp[3], 0.0);
        let bc = default_params_for_kind(EffectKind::Bitcrush);
        assert_eq!(bc[0], BitcrushEffect::new().parameters()[0].default);
    }

    #[test]
    fn param_count_reports_each_kinds_arity() {
        assert_eq!(param_count(EffectKind::Lowpass), 2);
        assert_eq!(param_count(EffectKind::Bitcrush), 2);
    }
```

Add to `modulation.rs`'s test module:

```rust
    #[test]
    fn clamp_targets_clears_out_of_range_targets() {
        let mut tm = TrackModulation::default_for_row(0);
        tm.targets = [Some(0), Some(5)];
        // An effect with 2 parameters: target 0 survives, target 5 is cleared.
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [Some(0), None]);
        // A target exactly at the count is out of range.
        tm.targets = [Some(2), None];
        tm.clamp_targets(2);
        assert_eq!(tm.targets, [None, None]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(default_params_for_kind) + test(param_count) + test(clamp_targets)'`
Expected: build failure — `cannot find function default_params_for_kind` / `param_count` / no method `clamp_targets`.

- [ ] **Step 3: Write minimal implementation**

In `effects.rs`, add two free functions (after the `EffectKind` impl block):

```rust
/// The number of modulatable parameters effect `kind` declares.
pub fn param_count(kind: EffectKind) -> usize {
    EffectInstance::new(kind).parameters().len()
}

/// The default parameter values for effect `kind`, laid out in the
/// `TrackEffect::params` slot order (slots past the kind's parameter count are
/// zero). Used when a track switches effect kind.
pub fn default_params_for_kind(kind: EffectKind) -> [f32; MAX_EFFECT_PARAMS] {
    let instance = EffectInstance::new(kind);
    let specs = instance.parameters();
    let mut params = [0.0; MAX_EFFECT_PARAMS];
    for (i, spec) in specs.iter().enumerate() {
        params[i] = spec.default;
    }
    params
}
```

In `modulation.rs`, add a method to `impl TrackModulation` (after `default_for_row`):

```rust
    /// Clear any assignable-MSEG target that points past `param_count`
    /// parameters. Called after a track's effect kind changes so a target can
    /// never reference a parameter the new effect does not have.
    pub fn clamp_targets(&mut self, param_count: usize) {
        for target in &mut self.targets {
            if let Some(i) = *target {
                if i >= param_count {
                    *target = None;
                }
            }
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(default_params_for_kind) + test(param_count) + test(clamp_targets)'` — PASS (3 tests).
Run: `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs multosis/src/modulation.rs
git commit -m "feat(multosis): add effect-kind-switch helpers"
```

---

### Task 7: Ghost-curve renderer for the MSEG widget

**Files:**
- Modify: `tiny-skia-widgets/src/mseg/render.rs`
- Modify: `tiny-skia-widgets/src/mseg/mod.rs` (only if a re-export is needed)

A new public function draws just a `MsegData`'s curve polyline, faintly, inside the editor's plot rect — for the two inactive MSEGs behind the active one.

- [ ] **Step 1: Write the failing test**

Add to `render.rs`'s `#[cfg(test)] mod tests` block (create it if absent — check the end of the file):

```rust
    #[test]
    fn ghost_curve_draws_some_pixels() {
        let mut pm = Pixmap::new(200, 120).unwrap();
        let data = MsegData::default(); // a 0->1 ramp
        let state = MsegEditState::new();
        let rect = (0.0, 0.0, 200.0, 120.0);
        draw_mseg_ghost(&mut pm, rect, &data, &state, 1.0, 0x5A5040FF);
        // The ghost curve strokes a polyline — some pixels are non-transparent.
        let any_drawn = pm.pixels().iter().any(|p| p.alpha() != 0);
        assert!(any_drawn, "the ghost curve should draw some pixels");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p tiny-skia-widgets --lib -E 'test(ghost_curve_draws_some_pixels)'`
Expected: build failure — `cannot find function draw_mseg_ghost`.

- [ ] **Step 3: Write minimal implementation**

In `render.rs`, add the ghost renderer. It draws a thin polyline of the curve sampled across the plot, in `color` (an `0xRRGGBBAA` packed value), with no nodes, markers, or strip. Use the existing `mseg_layout` / `phase_to_x` / `value_to_y` geometry and the same curve sampling `draw_canvas` uses (sample `value_at_phase` or walk the segments — match how `draw_canvas` plots the curve; read `draw_canvas` in this file and reuse its sampling approach):

```rust
/// Draw only `data`'s curve polyline — faint, no nodes/markers/strip — inside
/// the MSEG editor plot rect `rect`. For rendering inactive MSEGs as ghost
/// context behind an active `draw_mseg`. `color` is packed `0xRRGGBBAA`.
pub fn draw_mseg_ghost(
    pixmap: &mut Pixmap,
    rect: (f32, f32, f32, f32),
    data: &MsegData,
    state: &MsegEditState,
    scale: f32,
    color: u32,
) {
    let layout = mseg_layout(rect, state.is_curve_only(), scale);
    // Sample the curve across the plot width and stroke a polyline. Reuse the
    // same per-pixel sampling `draw_canvas` uses for the active curve so the
    // ghost aligns exactly.
    let (px, _py, pw, _ph) = layout.plot;
    let steps = pw.max(1.0) as usize;
    let mut prev: Option<(f32, f32)> = None;
    for s in 0..=steps {
        let phase = s as f32 / steps as f32;
        let value = crate::mseg::value_at_phase(data, phase);
        let x = phase_to_x(&layout, phase);
        let y = value_to_y(&layout, value);
        if let Some((x0, y0)) = prev {
            // stroke segment (x0,y0)->(x,y) in `color`
            stroke_line(pixmap, x0, y0, x, y, 1.5 * scale, color);
        }
        prev = Some((x, y));
        let _ = px;
    }
}
```

`stroke_line` stands in for whatever thin-line primitive the widget crate already has. **Read `render.rs` and `tiny-skia-widgets/src/primitives.rs` first**: `draw_canvas` already strokes the active curve — use the *same* line-drawing routine it calls (tiny-skia `Paint`+`PathBuilder`, or a primitive helper). Do not invent a primitive; match `draw_canvas`'s existing stroking code, just with the faint `color` and no node markers. If `draw_canvas` builds a single `Path` for the whole curve, build the ghost the same way (one path, one stroke) rather than per-segment — whichever the file already does. The signature and doc comment above are the contract; the body must use the file's real drawing approach.

If `draw_mseg_ghost` needs to be reachable as `widgets::mseg::draw_mseg_ghost`, confirm `mod.rs` does `pub use render::*;` (it does, per the pre-existing state) — no re-export change needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p tiny-skia-widgets --lib -E 'test(ghost_curve_draws_some_pixels)'` — PASS.
Run: `cargo build -p tiny-skia-widgets` — compiles, no warnings.
Run: `cargo clippy -p tiny-skia-widgets -- -D warnings` — clean.

- [ ] **Step 5: Commit**

```bash
git add tiny-skia-widgets/src/mseg/render.rs tiny-skia-widgets/src/mseg/mod.rs
git commit -m "feat(tiny-skia-widgets): add a ghost-curve renderer for the MSEG editor"
```

---

### Task 8: The effect editor — EFFECT section (kind + parameters)

**Files:**
- Create: `multosis/src/editor/effect_editor.rs`
- Modify: `multosis/src/editor.rs`

A new module owns the effect-editor layout, hit-testing, and draw. This task builds the editor bar and the EFFECT section (kind dropdown + parameter dials). The MODULATION section is Task 9.

- [ ] **Step 1: Write the failing test**

Create `multosis/src/editor/effect_editor.rs`:

```rust
//! The per-track effect editor — Phase 2 Milestone 2c. Drawn in the main area
//! (right of the track panel) when the window is in `View::Effect`. The EFFECT
//! section holds the kind dropdown and parameter dials; the MODULATION section
//! holds the MSEG selector, target/depth controls, and the MSEG pane.
//!
//! See `docs/superpowers/specs/2026-05-19-multosis-phase-2c-design.md` §3.

use crate::editor::grid_view::{GUTTER, MARGIN, STATUS_H, TRACK_PANEL_W};
use crate::editor::WINDOW_WIDTH;
use crate::effects::MAX_EFFECT_PARAMS;

/// The number of parameter-dial slots the layout reserves. Matches the widest
/// effect's parameter count headroom.
pub const DIAL_SLOTS: usize = MAX_EFFECT_PARAMS;

/// All physical-pixel sub-rects of the effect editor at `scale`. Computed once;
/// consumed by both hit-testing and drawing.
#[derive(Clone, Copy, Debug)]
pub struct EffectLayout {
    /// `< Grid` back button.
    pub back: (f32, f32, f32, f32),
    /// Effect-kind dropdown trigger.
    pub kind: (f32, f32, f32, f32),
    /// Parameter dial bounding boxes, slot order. Only the first
    /// `parameters().len()` are used by the current effect.
    pub dials: [(f32, f32, f32, f32); DIAL_SLOTS],
    /// MSEG selector (stepped) — laid out here, used in Task 9.
    pub mseg_selector: (f32, f32, f32, f32),
    /// Target dropdown trigger — used in Task 9.
    pub target: (f32, f32, f32, f32),
    /// Depth dial — used in Task 9.
    pub depth: (f32, f32, f32, f32),
    /// MSEG editor pane — used in Task 9.
    pub mseg_pane: (f32, f32, f32, f32),
}

/// Compute the effect-editor layout at `scale`.
pub fn effect_layout(scale: f32) -> EffectLayout {
    // Logical main-area origin (right of the track panel, below the toolbar).
    let ox = MARGIN + TRACK_PANEL_W;
    let oy = STATUS_H + GUTTER;
    let mw = WINDOW_WIDTH as f32 - MARGIN - ox;
    let l = |x: f32, y: f32, w: f32, h: f32| {
        (x * scale, y * scale, w * scale, h * scale)
    };
    // Editor bar.
    let back = l(ox, oy + 4.0, 90.0, 26.0);
    // EFFECT section.
    let kind = l(ox, oy + 50.0, 150.0, 28.0);
    let dials = std::array::from_fn(|i| {
        l(ox + 180.0 + i as f32 * 96.0, oy + 44.0, 80.0, 80.0)
    });
    // MODULATION section.
    let mseg_selector = l(ox, oy + 168.0, 240.0, 26.0);
    let target = l(ox + 470.0, oy + 167.0, 170.0, 28.0);
    let depth = l(ox + 660.0, oy + 162.0, 70.0, 70.0);
    let mseg_pane = l(ox, oy + 208.0, mw, 422.0);
    EffectLayout {
        back,
        kind,
        dials,
        mseg_selector,
        target,
        depth,
        mseg_pane,
    }
}

/// True when physical-pixel point `(px, py)` is inside `rect`.
pub fn in_rect((rx, ry, rw, rh): (f32, f32, f32, f32), px: f32, py: f32) -> bool {
    px >= rx && px < rx + rw && py >= ry && py < ry + rh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_rects_are_disjoint_for_the_main_controls() {
        let lay = effect_layout(1.0);
        // The back button, kind dropdown, and first dial do not overlap.
        assert!(!rects_overlap(lay.back, lay.kind));
        assert!(!rects_overlap(lay.kind, lay.dials[0]));
        assert!(!rects_overlap(lay.back, lay.dials[0]));
        // The MSEG pane sits below the EFFECT controls.
        assert!(lay.mseg_pane.1 > lay.kind.1 + lay.kind.3);
    }

    #[test]
    fn layout_scales_linearly() {
        let a = effect_layout(1.0);
        let b = effect_layout(2.0);
        assert!((b.kind.0 - a.kind.0 * 2.0).abs() < 1e-3);
        assert!((b.kind.2 - a.kind.2 * 2.0).abs() < 1e-3);
    }

    fn rects_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }
}
```

Register the module in `editor.rs`: `pub mod effect_editor;` beside the other `pub mod` lines.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo build -p multosis` first — the module must compile. Then `cargo nextest run -p multosis --lib -E 'test(layout_rects_are_disjoint) + test(layout_scales)'`
Expected: PASS (the layout fns are written in Step 1). The "failing" part of this task is the missing `draw_effect_editor` / hit-testing referenced below — proceed to Step 3.

- [ ] **Step 3: Write minimal implementation**

Add the EFFECT-section hit-test enum and function, and the draw function, to `effect_editor.rs` (before the test module):

```rust
use crate::effects::{EffectKind, TrackEffect};
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// A control the user pressed in the effect editor. `Dial`/`Depth` carry the
/// slot index; `MsegPane` carries nothing (handled by the MSEG widget).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EffectHit {
    Back,
    Kind,
    Dial(usize),
    MsegSelector(usize),
    Target,
    Depth,
    MsegPane,
}

/// The effect-editor control under physical-pixel point `(px, py)` at `scale`,
/// given the current effect's parameter count `param_count` and the active
/// MSEG `selected_mseg` (0 = amplitude, 1/2 = assignable). Returns `None` for
/// a point over no control.
pub fn effect_hit(
    px: f32,
    py: f32,
    scale: f32,
    param_count: usize,
    selected_mseg: usize,
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
    // MSEG selector — three equal segments.
    let (sx, sy, sw, sh) = lay.mseg_selector;
    if px >= sx && px < sx + sw && py >= sy && py < sy + sh {
        let seg = (((px - sx) / (sw / 3.0)) as usize).min(2);
        return Some(EffectHit::MsegSelector(seg));
    }
    // Target dropdown + depth dial only exist for an assignable MSEG.
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

/// Draw the editor bar and EFFECT section. The MODULATION section is drawn by
/// `draw_modulation_section` (Task 9). `track` is the edited track's effect
/// config; `track_index` is its row (0-based) for the title.
pub fn draw_effect_section(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    track: &TrackEffect,
    track_index: usize,
    kind_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    // Editor bar.
    widgets::controls::draw_button(
        pixmap,
        tr,
        lay.back.0,
        lay.back.1,
        lay.back.2,
        lay.back.3,
        "< Grid",
        false,
        false,
    );
    let title_size = 13.0 * scale;
    tr.draw_text(
        pixmap,
        lay.back.0 + lay.back.2 + 16.0 * scale,
        lay.back.1 + (lay.back.3 + title_size) * 0.5 - 2.0,
        &format!("Editing Track {}", track_index + 1),
        title_size,
        tiny_skia::Color::from_rgba8(0xE8, 0xC9, 0x8A, 0xFF),
    );
    // EFFECT section: kind dropdown trigger.
    widgets::dropdown::draw_dropdown_trigger(
        pixmap,
        tr,
        lay.kind,
        track.kind.name(),
        kind_dropdown_open,
    );
    // Parameter dials.
    let instance = crate::effects::EffectInstance::new(track.kind);
    let specs = instance.parameters();
    for (i, spec) in specs.iter().enumerate() {
        let (dx, dy, dw, dh) = lay.dials[i];
        let value = track.params[i];
        let norm = if spec.max > spec.min {
            ((value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            spec.name,
            &format!("{value:.0}"),
            norm,
        );
    }
}

/// The list of effect-kind names for the kind dropdown, in `EffectKind::ALL`
/// order.
pub fn kind_items() -> Vec<&'static str> {
    EffectKind::ALL.iter().map(|k| k.name()).collect()
}
```

The widget calls above are verified: `widgets::controls::draw_button(pixmap, tr, x, y, w, h, label, active, _hovered)`, `widgets::dropdown::draw_dropdown_trigger(pixmap, tr, rect, label, is_open)`, `widgets::param_dial::draw_dial(pixmap, tr, cx, cy, radius, label, value_text, normalized)`, `TextRenderer::draw_text(pixmap, x, baseline_y, text, size, Color)`. All three `draw_*` widget fns are also re-exported at the crate root, so `widgets::draw_button` etc. work too — match `toolbar.rs`'s existing call style. Custom colours: `tiny_skia::Color::from_rgba8(r, g, b, a)`.

In `editor.rs`, wire the EFFECT view. `MultosisWindow` gains:
- `selected_mseg: usize` (0..3, default 0),
- `kind_dropdown: widgets::dropdown::DropdownState<EffectAction>` where `EffectAction` is a new enum in `editor.rs`: `#[derive(Clone, Copy, PartialEq, Eq, Debug)] enum EffectAction { Kind, Target }`,
- `effect_dial_drag: widgets::DragState<effect_editor::EffectHit>` for dial drags.

Replace Task 4's temporary "Back to Grid" button in the `View::Effect` draw arm with a call to `effect_editor::draw_effect_section(...)`, reading the selected track's `TrackEffect` from `self.params.track_effects.lock()`. Draw the kind dropdown popup last (`widgets::dropdown::draw_dropdown_popup`).

In `on_event`, when `self.view == View::Effect`, route presses through `effect_editor::effect_hit(...)`:
- `EffectHit::Back` → `self.view = View::Grid`.
- `EffectHit::Kind` → open `kind_dropdown` (`open(EffectAction::Kind, lay.kind, &kind_items, current_kind_index, false, window_size)`).
- `EffectHit::Dial(i)` → begin an `effect_dial_drag` for that dial; CursorMoved updates the parameter (map the drag delta to the `ParamSpec` range), writes `track_effects[selected_track].params[i]`, and calls `self.mark_config_dirty()`.
- Other hits — handled in Task 9; ignore for now.

Route the kind dropdown's mouse events to `kind_dropdown.on_mouse_down/move/up`; on `DropdownEvent::Selected(EffectAction::Kind, idx)`, set `track_effects[selected_track].kind = EffectKind::ALL[idx]`, reset its `params` to `crate::effects::default_params_for_kind(kind)`, call `track_modulation[selected_track].clamp_targets(crate::effects::param_count(kind))`, and `mark_config_dirty()`.

The dial drag maps a vertical drag to a normalized delta (use `widgets::DragState`'s drag-delta convention exactly as `toolbar.rs` uses it for the Mix/Output sliders — read that code and mirror it); the new normalized value maps back to the parameter range via `spec.min + norm * (spec.max - spec.min)`.

Keep this task's editor wiring focused on the EFFECT section + Back. The MODULATION controls (`MsegSelector`, `Target`, `Depth`, `MsegPane`) are wired in Task 9 — leaving their `EffectHit` arms as `{}` for now is correct.

This is the largest task; if the editor wiring grows unwieldy, factor the `View::Effect` press handling into a `MultosisWindow::on_effect_press(&mut self, px, py)` method and the draw into `draw_effect_view(&mut self)`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis` — PASS (184 tests: 182 after Task 6 + 2 layout tests; the suite must be all-green with no failures).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean (run `cargo fmt -p multosis` if not).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/effect_editor.rs multosis/src/editor.rs
git commit -m "feat(multosis): add the effect editor EFFECT section"
```

---

### Task 9: The effect editor — MODULATION section & MSEG pane

**Files:**
- Modify: `multosis/src/editor/effect_editor.rs`
- Modify: `multosis/src/editor.rs`

The MODULATION section: the Amp/M1/M2 selector, the target dropdown + depth dial (for assignable MSEGs), and the MSEG editor pane with the two inactive MSEGs ghosted behind the active one.

- [ ] **Step 1: Write the failing test**

Add to `effect_editor.rs`'s test module:

```rust
    #[test]
    fn target_items_lists_none_then_each_parameter() {
        let items = target_items(crate::effects::EffectKind::Lowpass);
        assert_eq!(items[0], "(none)");
        assert_eq!(items[1], "Cutoff");
        assert_eq!(items[2], "Resonance");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn target_index_round_trips_through_the_dropdown_indexing() {
        // Dropdown item 0 => None; item i+1 => Some(i).
        assert_eq!(target_from_item(0), None);
        assert_eq!(target_from_item(1), Some(0));
        assert_eq!(target_from_item(3), Some(2));
        // And back: None => 0; Some(i) => i+1.
        assert_eq!(target_to_item(None), 0);
        assert_eq!(target_to_item(Some(0)), 1);
        assert_eq!(target_to_item(Some(2)), 3);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(target_items_lists) + test(target_index_round_trips)'`
Expected: build failure — `cannot find function target_items` / `target_from_item` / `target_to_item`.

- [ ] **Step 3: Write minimal implementation**

Add to `effect_editor.rs`:

```rust
/// The target-dropdown items for `kind`: `(none)` followed by each parameter
/// name, in parameter-index order.
pub fn target_items(kind: EffectKind) -> Vec<&'static str> {
    let instance = crate::effects::EffectInstance::new(kind);
    let mut items = vec!["(none)"];
    items.extend(instance.parameters().iter().map(|s| s.name));
    items
}

/// The `targets` value for target-dropdown item `item` (0 = `(none)`).
pub fn target_from_item(item: usize) -> Option<usize> {
    item.checked_sub(1)
}

/// The target-dropdown item index for a `targets` value.
pub fn target_to_item(target: Option<usize>) -> usize {
    match target {
        None => 0,
        Some(i) => i + 1,
    }
}

/// Draw the MODULATION section: the MSEG selector, and — for an assignable
/// MSEG — the target dropdown trigger and depth dial. `selected_mseg` is
/// 0 (amplitude) / 1 / 2; `target` and `depth` belong to the active assignable
/// MSEG (ignored when `selected_mseg == 0`). The MSEG pane itself is drawn by
/// the caller (it needs the window's `MsegEditState`).
#[allow(clippy::too_many_arguments)]
pub fn draw_modulation_controls(
    pixmap: &mut Pixmap,
    tr: &mut widgets::TextRenderer,
    selected_mseg: usize,
    kind: EffectKind,
    target: Option<usize>,
    depth: f32,
    target_dropdown_open: bool,
    scale: f32,
) {
    let lay = effect_layout(scale);
    widgets::controls::draw_stepped_selector(
        pixmap,
        tr,
        lay.mseg_selector.0,
        lay.mseg_selector.1,
        lay.mseg_selector.2,
        lay.mseg_selector.3,
        &["Amp", "MSEG 1", "MSEG 2"],
        selected_mseg.min(2),
    );
    if selected_mseg != 0 {
        let label = match target {
            None => "(none)",
            Some(i) => crate::effects::EffectInstance::new(kind)
                .parameters()
                .get(i)
                .map(|s| s.name)
                .unwrap_or("(none)"),
        };
        widgets::dropdown::draw_dropdown_trigger(
            pixmap,
            tr,
            lay.target,
            label,
            target_dropdown_open,
        );
        // depth dial: bipolar -1..1 mapped to a 0..1 arc.
        let (dx, dy, dw, dh) = lay.depth;
        let norm = ((depth + 1.0) / 2.0).clamp(0.0, 1.0);
        widgets::param_dial::draw_dial(
            pixmap,
            tr,
            dx + dw / 2.0,
            dy + dh / 2.0,
            (dw.min(dh) / 2.0) - 8.0 * scale,
            "Depth",
            &format!("{depth:+.2}"),
            norm,
        );
    }
}
```

In `editor.rs`, complete the `View::Effect` wiring:

**Window state.** Ensure `MultosisWindow` has `mseg_edit: widgets::mseg::MsegEditState` (`MsegEditState::new()`), the `kind_dropdown` and a `target_dropdown: widgets::dropdown::DropdownState<EffectAction>` (or reuse one `DropdownState<EffectAction>` — `EffectAction` already distinguishes `Kind` vs `Target`, and only one dropdown is open at a time, so **one** `DropdownState<EffectAction>` suffices; use one), a `depth_drag: widgets::DragState<()>` or fold depth into the existing `effect_dial_drag` by adding a `Depth` arm.

**Draw.** In the `View::Effect` draw, after `draw_effect_section`, draw the MODULATION section:
1. Read `track_modulation[selected_track]`.
2. Draw the two inactive MSEGs as ghosts into the `mseg_pane` rect: for each `m` in `0..3` with `m != selected_mseg`, call `widgets::mseg::draw_mseg_ghost(pixmap, mseg_pane, &msegs[m], &self.mseg_edit, scale, 0x5A5040FF)`.
3. Draw the active MSEG: `widgets::mseg::draw_mseg(pixmap, tr, mseg_pane, &msegs[selected_mseg], &self.mseg_edit, scale)`.
4. `draw_modulation_controls(...)` for the selector + target + depth.
5. Draw any open dropdown popup last.

**Events.** Extend the `View::Effect` press routing (`effect_hit`):
- `EffectHit::MsegSelector(seg)` → `self.selected_mseg = seg`.
- `EffectHit::Target` → open the dropdown for `EffectAction::Target` with `target_items(kind)` and `target_to_item(current_target)` as `current`.
- `EffectHit::Depth` → begin a depth drag; CursorMoved maps the drag delta to a −1..1 value, writes `track_modulation[selected_track].depths[selected_mseg-1]`, `mark_config_dirty()`.
- `EffectHit::MsegPane` → forward to `self.mseg_edit.on_mouse_down(px, py, &mut msegs[selected_mseg], mseg_pane, scale, shift)`; an `MsegEdit::Changed` result calls `mark_config_dirty()`. Wire `on_mouse_move` (in CursorMoved), `on_mouse_up` (in ButtonReleased), `on_double_click`, and `on_right_click` the same way `miff/src/editor.rs` does — each locks `track_modulation`, calls the handler on `msegs[selected_mseg]`, and on `Some(MsegEdit::Changed)` calls `mark_config_dirty()`. Reuse miff's double-click detection helper pattern (a last-click time + position).
- On `DropdownEvent::Selected(EffectAction::Target, item)` → `track_modulation[selected_track].targets[selected_mseg-1] = effect_editor::target_from_item(item)`, `mark_config_dirty()`.

The dropdown's `on_mouse_down/move/up` must be routed in `View::Effect` *before* `effect_hit`, so clicks on an open popup hit the popup, not the controls behind it — mirror how the dropdown is consumed elsewhere (check an existing `DropdownState` consumer in the workspace, e.g. via `rg "DropdownState" --type rust`, and follow that ordering).

Keep the audio-thread rules irrelevant here (this is all GUI thread); but every mutation path must end in `mark_config_dirty()` so the handoff (Task 5) re-bridges it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(target_items_lists) + test(target_index_round_trips)'` — PASS.
Run: `cargo nextest run -p multosis` — PASS (all green).
Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo fmt -p multosis -- --check` — clean.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/effect_editor.rs multosis/src/editor.rs
git commit -m "feat(multosis): add the effect editor MODULATION section + MSEG pane"
```

---

### Task 10: Verification

**Files:** none — checks and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS, all green.
Run: `cargo nextest run --workspace` — PASS, all green.
Run: `cargo clippy --workspace -- -D warnings` — no warnings.
Run: `cargo fmt --check` — clean (if a diff, `cargo fmt` and commit it in Step 4).

- [ ] **Step 2: Release build and bundle**

Run: `cargo xtask native build --bin multosis --release` — the standalone binary builds.
Run: `cargo xtask native nih-plug bundle multosis --release` — VST3 + CLAP + standalone bundle, no errors.

- [ ] **Step 3: Manual smoke test**

Run `cargo run --bin multosis` (or load the bundle in a host). Confirm:
- The grid is shifted right; the left panel lists 16 tracks with their effect kinds; a track's dot lights green while it is sounding.
- Clicking a track opens its effect editor; the track is highlighted in the listing; `< Grid` returns to the grid; the listing stays in both views.
- In the editor: changing the effect kind swaps the effect (and resets its parameter dials to the new defaults); turning a parameter dial is heard immediately and does not click; the Amp/MSEG 1/MSEG 2 selector switches the active MSEG; the two inactive MSEGs show as faint ghosts behind it; editing the MSEG curve, its target, and its depth are all heard immediately.
- The grid still edits correctly in its shifted position (cell toggles, drag-paint, loop region, toolbar).

Report the smoke-test observations.

- [ ] **Step 4: Commit (only if Step 1 required formatting edits)**

```bash
git add -A
git commit -m "style(multosis): apply rustfmt for the effect editor"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- Multosis has a persistent left-edge track listing (number, effect kind, live "sounding" dot) shown in both views, and a per-track effect editor reached by clicking a track — kind dropdown, parameter dials, a single MSEG pane with a 3-way selector and the two inactive MSEGs ghosted behind, plus target/depth controls for the assignable MSEGs.
- Edits are bridged live into the audio engine through a `config_dirty` flag; `set_effects` is incremental so parameter edits do not reset DSP state.
- `cargo nextest run -p multosis` is green; `cargo clippy --workspace -- -D warnings` is clean; the plugin bundles and the editor behaves per the smoke test.

## Spec coverage check (self-review)

- §1 View shell — `View` enum + `selected_track`, toolbar shared, window widened by `TRACK_PANEL_W`, grid coordinates shifted, draw/event routing by view (Tasks 1, 4).
- §2 Track listing — `track_list` module: number + effect name + sounding dot, hit-testing, drawn in both views; the dot fed by the engine's `active_mask` via `Arc<AtomicU16>` (Tasks 2, 3, 4).
- §3 Effect editor — EFFECT section (kind dropdown + param dials) and MODULATION section (MSEG selector, target dropdown, depth dial) (Tasks 8, 9).
- §4 MSEG pane & ghosts — the `mseg` widget in full mode, one shared `MsegEditState`, the ghost-curve renderer (Tasks 7, 9).
- §5 Live handoff — `config_dirty` flag, re-bridge in `process()` clearing the flag only on a successful `try_lock`, incremental `set_effects` (Task 5).
- §6 Module structure — `track_list.rs`, `effect_editor.rs`, the `editor.rs`/`grid_view.rs`/`engine.rs`/`lib.rs` edits, the `mseg` widget addition (Tasks 1–9).
- §7 Defaults & kind switch — opens in Grid; kind change resets params + clamps targets (`default_params_for_kind`, `clamp_targets`) (Task 6, applied in Task 8).
- §8 Out of scope — no new effects, no log dials, no bypass, no retriggering, no presets, no view persistence: none added.
- §9 Testing — grid-shift round-trips, `track_at`, `active_mask`, incremental `set_effects`, kind-switch helpers, the ghost renderer, the effect-editor layout/hit-test/target indexing; the smoke test (Tasks 1–10).

## Note on task sequencing

Tasks 1–7 each end with a green build and green tests; the editor is in a working intermediate state throughout (the grid shifts in Task 1 with a blank strip until Task 4 fills it; `View::Effect` shows only a Back button until Task 8). Tasks 8–9 are the large editor-wiring tasks — UI draw code is verified by the Task 10 smoke test, while layout, hit-testing, and the kind-switch / target-indexing logic are unit-tested. Task 6 (kind-switch helpers) and Task 7 (ghost renderer) are deliberately ordered before Task 8/9 so their consumers can call them.
