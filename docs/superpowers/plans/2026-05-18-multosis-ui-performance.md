# Multosis Editor UI Performance Pass — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the editor redrawing all 512 grid cells on every mouse event — cache the rendered grid and re-render only what changed.

**Architecture:** Task 1 adds a permanent `bench_editor_draw` benchmark and records the baseline render cost. Task 2 introduces a `GridCache` that owns a rendered-grid `Pixmap`; `draw()` blits the cache and re-renders into it only the cells that actually changed (or all of them on a scale change). Task 3 re-measures and verifies. The fix targets the observed symptom directly — per-mouse-event work becomes proportional to what changed, not a fixed 512-cell repaint — without guessing at any micro-hotspot.

**Tech Stack:** Rust (nightly), nih-plug, tiny-skia, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-ui-performance-design.md`.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state (142 multosis tests, 919 workspace tests green):**
- `multosis/src/editor.rs`: `MultosisWindow::on_frame` (per baseview event) → `draw()`. `draw()` does `widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg())`, then `grid_view::draw_grid(&mut self.surface.pixmap, &grid, self.scale_factor, Some(self.mouse_pos))`, `grid_view::draw_wavefront(...)`, `toolbar::draw_toolbar(...)`. `grid` is obtained by `self.params.grid.lock().map(|g| *g).unwrap_or_default()`. No caching, no change detection — every event repaints everything.
- `multosis/src/editor/grid_view.rs`: `draw_grid(pixmap, grid, scale, cursor)` iterates all 16×32 cells calling the private `draw_cell(pixmap, row, col, cell, scale)`, then draws the loop-region outline, the four edge nubs, and (when `cursor` is inside the region) the move grip. `draw_cell` draws the cell background (`widgets::draw_rect`, opaque fast path), one `fill_triangle` per lit send direction (anti-aliased tiny-skia path fill), and the start-marker outline. `draw_wavefront(pixmap, wf, scale)` iterates the cells and fills the lit ones. Layout consts `STATUS_H`, `TOOLBAR_ROW_H`, `CELL`, `MARGIN`, `GUTTER`, `GROUP_SIZE`, `GROUP_GAP`; `ROWS`/`COLS` from `crate::grid`. `cell_rect(row, col, scale)` gives a cell's physical rect.
- `crate::grid::Grid` derives `Clone, Copy, PartialEq, Eq`; has `cell(row,col) -> &Cell`, `cell_mut(row,col) -> &mut Cell`; `Cell` has public `enabled: bool`, `sends: u8` (an 8-direction bitmask), `is_start: bool`. `Grid::default_routing()` builds the default grid.
- `crate::editor::WINDOW_WIDTH = 1336`, `WINDOW_HEIGHT = 758` (`u32`).
- `widgets::fill_pixmap_opaque`, `widgets::color_bg`, `widgets::TextRenderer`. `tiny_skia::Pixmap::new(w, h) -> Option<Pixmap>`, `Pixmap::data()`/`data_mut()` (the raw RGBA byte slice), `Pixmap::draw_pixmap(...)` for blitting one pixmap onto another.
- `wavetable-filter/src/lib.rs` has `bench_wavetable_draw` — a plain `#[test]` using `std::time::Instant` over N iterations, with a `print_timing_stats` helper printing min/avg/p50/p95/p99. This is the benchmark pattern to mirror.
- `MultosisParams`, `WavefrontDisplay`, `SeqStatusDisplay` are all constructible without a window (`MultosisParams::default()` is used by the plugin's own `Default`).

---

### Task 1: `bench_editor_draw` benchmark and baseline

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`

This task adds the measurement instrument and records the baseline. A benchmark `#[test]` is not classic red-green TDD — it is written, run, and its printed numbers recorded.

- [ ] **Step 1: Read the reference benchmark**

Read `wavetable-filter/src/lib.rs`'s `bench_wavetable_draw` and its `print_timing_stats` helper. Note the structure: warm-up iterations, N timed iterations with `std::time::Instant`, collecting `Vec<f64>` of microsecond samples, then a stats print (min / avg / p50 / p95). Also confirm the real constructors for `widgets::TextRenderer`, `MultosisParams`, `WavefrontDisplay`, `SeqStatusDisplay` by reading `multosis/src/editor.rs` (`MultosisWindow::new`) and the respective modules.

- [ ] **Step 2: Add the benchmark**

Add to `grid_view.rs`'s `#[cfg(test)] mod tests` block a `bench_editor_draw` test. It must:
- Build a **dense worst-case grid**: `let mut grid = Grid::default_routing();` then for every `(r, c)` set `grid.cell_mut(r, c).enabled = true;` and `grid.cell_mut(r, c).sends = 0xFF;` (all 8 directions lit).
- Create a `Pixmap` at the window size: `tiny_skia::Pixmap::new(crate::editor::WINDOW_WIDTH, crate::editor::WINDOW_HEIGHT).unwrap()`.
- Construct a `WavefrontDisplay`, a `MultosisParams`, a `widgets::TextRenderer`, a `SeqStatusDisplay` (use the real constructors found in Step 1).
- Use `scale = 1.0`.
- Run ~10 warm-up render iterations, then ~200 timed iterations. Each iteration: `widgets::fill_pixmap_opaque(&mut pixmap, widgets::color_bg())`, then time `draw_grid(&mut pixmap, &grid, 1.0, None)`, `draw_wavefront(&mut pixmap, &wf, 1.0)`, and `toolbar::draw_toolbar(&mut pixmap, &mut tr, &params, &seq, 1.0)` separately, plus the whole-frame total.
- Collect per-sub-function and total microsecond samples and print min / avg / p95 for each, with clear labels (`"draw_grid"`, `"draw_wavefront"`, `"draw_toolbar"`, `"frame total"`). A small inline stats-print helper modelled on `print_timing_stats` is fine; or compute and `println!` inline.
- The test asserts nothing about timing (timings are machine-dependent) — it asserts only that it ran (e.g. that the sample vector is non-empty). Its value is the printed numbers.

NOTE — write the benchmark with `cargo nextest`'s captured-output behaviour in mind: the prints are visible with `--no-capture` or in a failing run. It is acceptable for the benchmark to deliberately surface its numbers; the simplest reliable approach is a normal test whose output is read with `cargo nextest run -p multosis bench_editor_draw --no-capture` (or `cargo test`). Confirm `bench_wavetable_draw`'s approach and match it.

- [ ] **Step 3: Run the benchmark and record the baseline**

Run (release, for representative numbers — `target-cpu` affects tiny-skia codegen):
`cargo nextest run -p multosis --release bench_editor_draw --no-capture`

Expected: the test passes and prints the timing breakdown. **Record the printed numbers** in the task report — the `draw_grid` / `draw_wavefront` / `draw_toolbar` / total min/avg/p95. This is the baseline the rest of the pass is measured against.

Also run `cargo nextest run -p multosis` — PASS, 143 tests (142 + `bench_editor_draw`).
Run `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "test(multosis): add bench_editor_draw editor-render benchmark"
```

The task report MUST quote the recorded baseline timings.

---

### Task 2: `GridCache` — render the grid once, blit thereafter

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`

The editor repaints all 512 cells on every event. `GridCache` keeps the rendered grid in a `Pixmap`; `draw()` blits that cache and re-renders into it only the cells whose data changed since the last frame (or all of them when `scale` changed). A hover or a region/window drag changes no cells → the per-event grid work becomes a blit; a paint stroke changes one or two cells → only those re-render.

The loop-region outline, edge nubs, move grip (cursor-dependent), the wavefront (animates) and the toolbar are **not** cached — they are cheap and/or change every frame; they keep being drawn fresh, on top of the blitted cache.

- [ ] **Step 1: Write the failing test**

Add to `grid_view.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn grid_cache_matches_an_uncached_cell_render() {
        let mut grid = Grid::default_routing();
        for r in 0..ROWS {
            for c in 0..COLS {
                grid.cell_mut(r, c).enabled = (r + c) % 2 == 0;
                grid.cell_mut(r, c).sends = ((r * 7 + c) & 0xFF) as u8;
            }
        }
        let w = crate::editor::WINDOW_WIDTH;
        let h = crate::editor::WINDOW_HEIGHT;
        // Reference: a fresh pixmap with every cell drawn directly.
        let mut reference = Pixmap::new(w, h).unwrap();
        widgets::fill_pixmap_opaque(&mut reference, widgets::color_bg());
        draw_grid_cells(&mut reference, &grid, 1.0);
        // Cache: first update is a full build.
        let mut cache = GridCache::new(w, h);
        cache.update(&grid, 1.0);
        assert_eq!(cache.pixmap().data(), reference.data(), "cold cache differs");
        // Mutate two cells; an incremental update must still match a full render.
        grid.cell_mut(3, 5).enabled = !grid.cell(3, 5).enabled;
        grid.cell_mut(9, 20).sends ^= 0b0101_0101;
        cache.update(&grid, 1.0);
        let mut reference2 = Pixmap::new(w, h).unwrap();
        widgets::fill_pixmap_opaque(&mut reference2, widgets::color_bg());
        draw_grid_cells(&mut reference2, &grid, 1.0);
        assert_eq!(
            cache.pixmap().data(),
            reference2.data(),
            "incremental cache differs"
        );
    }

    #[test]
    fn grid_cache_rebuilds_on_scale_change() {
        let grid = Grid::default_routing();
        let (w, h) = (crate::editor::WINDOW_WIDTH, crate::editor::WINDOW_HEIGHT);
        let mut cache = GridCache::new(w, h);
        cache.update(&grid, 1.0);
        cache.update(&grid, 1.5);
        let mut reference = Pixmap::new(w, h).unwrap();
        widgets::fill_pixmap_opaque(&mut reference, widgets::color_bg());
        draw_grid_cells(&mut reference, &grid, 1.5);
        assert_eq!(cache.pixmap().data(), reference.data(), "scale rebuild differs");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(grid_cache)'`
Expected: build failure — `cannot find type GridCache` / function `draw_grid_cells`.

- [ ] **Step 3: Extract `draw_grid_cells`**

In `grid_view.rs`, add a public function that renders just the cells (the cacheable part) — the cell loop currently inside `draw_grid`:

```rust
/// Draw every grid cell into `pixmap` (the cacheable part of the grid — no
/// loop-region overlay, no wavefront). `draw_grid` and `GridCache` share it.
pub fn draw_grid_cells(pixmap: &mut Pixmap, grid: &Grid, scale: f32) {
    for r in 0..ROWS {
        for c in 0..COLS {
            draw_cell(pixmap, r, c, grid.cell(r, c), scale);
        }
    }
}
```

Then change `draw_grid`'s own cell loop (the `for r ... for c ... draw_cell` block at its top) to call `draw_grid_cells(pixmap, grid, scale);` instead — leaving the loop-region outline / nubs / move-grip code in `draw_grid` unchanged.

- [ ] **Step 4: Add `GridCache`**

Add to `grid_view.rs`:

```rust
/// A cached render of the grid's cells. The editor repaints only when the
/// grid or `scale` actually changes; an unchanged frame is a blit, and a
/// few-cell change re-renders just those cells.
pub struct GridCache {
    pixmap: Pixmap,
    /// The grid state currently rendered into `pixmap`.
    grid: Grid,
    /// The scale `pixmap` was rendered at.
    scale: f32,
    /// False until the first `update` has populated `pixmap`.
    built: bool,
}

impl GridCache {
    /// A cache sized for a `w`×`h` editor window.
    pub fn new(w: u32, h: u32) -> Self {
        Self {
            pixmap: Pixmap::new(w, h).expect("editor window size is valid"),
            grid: Grid::default_routing(),
            scale: 0.0,
            built: false,
        }
    }

    /// Bring the cache up to date for `(grid, scale)`. A full rebuild on the
    /// first call or a scale change; otherwise re-renders only the cells that
    /// differ from the cached grid.
    pub fn update(&mut self, grid: &Grid, scale: f32) {
        if !self.built || self.scale != scale {
            widgets::fill_pixmap_opaque(&mut self.pixmap, widgets::color_bg());
            draw_grid_cells(&mut self.pixmap, grid, scale);
        } else {
            for r in 0..ROWS {
                for c in 0..COLS {
                    if grid.cell(r, c) != self.grid.cell(r, c) {
                        draw_cell(&mut self.pixmap, r, c, grid.cell(r, c), scale);
                    }
                }
            }
        }
        self.grid = *grid;
        self.scale = scale;
        self.built = true;
    }

    /// The cached grid render — blit this into the editor pixmap each frame.
    pub fn pixmap(&self) -> &Pixmap {
        &self.pixmap
    }
}
```

NOTE: `grid.cell(r, c) != self.grid.cell(r, c)` compares `&Cell` — `Cell` derives `PartialEq` (it is a field of `Grid`, which derives `PartialEq`). If the comparison does not compile, dereference both sides. Re-rendering a single cell via `draw_cell` fully repaints that cell's rectangle (the cell background covers it), so an incremental update is exact — confirmed by the `grid_cache_matches_an_uncached_cell_render` test.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(grid_cache)'`
Expected: PASS — 2 tests.

- [ ] **Step 6: Wire `GridCache` into the editor**

In `multosis/src/editor.rs`:
- Add a field to `MultosisWindow`: `grid_cache: grid_view::GridCache`.
- In `MultosisWindow::new`, initialise it: `grid_cache: grid_view::GridCache::new(WINDOW_WIDTH, WINDOW_HEIGHT),` (use the window-size constants the file already has in scope; the cache pixmap is window-sized so a blit covers the whole frame).
- Restructure `draw()`. Add a small public function to `grid_view.rs` — `draw_region_overlay(pixmap, grid, scale, cursor)` — containing exactly the loop-region outline / edge-nub / move-grip code currently at the *end* of `draw_grid` (move that code out of `draw_grid` into `draw_region_overlay`). `draw_grid` is then just `draw_grid_cells` + `draw_region_overlay`; once the editor no longer calls it, remove `draw_grid` and update any caller (the `bench_editor_draw` benchmark — see Step 7) to call the two functions directly.
- `draw()` becomes, in order:
  1. `self.grid_cache.update(&grid, self.scale_factor);`
  2. blit the cache over the surface with a **raw `copy_from_slice`** — the cache pixmap and the surface pixmap are the same dimensions, so this is a plain memcpy and the fastest possible blit (the technique `wavetable-filter`'s 3D-background cache uses): `self.surface.pixmap.data_mut().copy_from_slice(self.grid_cache.pixmap().data());`. This replaces both the old `fill_pixmap_opaque` clear *and* the `draw_grid` call — the cache already contains the cleared background plus the cells, and the copy overwrites the entire surface.
  3. `grid_view::draw_region_overlay(&mut self.surface.pixmap, &grid, self.scale_factor, Some(self.mouse_pos));`
  4. `grid_view::draw_wavefront(...)` and `toolbar::draw_toolbar(...)` as before.

  So the editor's `draw()` is: `grid_cache.update` → `copy_from_slice` blit → `draw_region_overlay` → `draw_wavefront` → `draw_toolbar`. The standalone `fill_pixmap_opaque` clear is gone (the blit covers the whole surface). Confirm `Pixmap::data()` / `data_mut()` return the raw RGBA byte slice and the two pixmaps are the same length before the `copy_from_slice`; if dimensions could ever differ, fall back to `draw_pixmap` with `BlendMode::Source`.

- [ ] **Step 7: Update `bench_editor_draw`**

Update the `bench_editor_draw` benchmark from Task 1 so it measures the new path. It should now time, per iteration:
- a **static-grid** loop: `grid_cache.update(&grid, 1.0)` with the *same* grid every iteration (the cache-hit case — should be cheap after the first), plus the blit;
- a **few-cells-changed** loop: mutate 2 cells each iteration before `grid_cache.update` (the drag-paint case);
- the overlay + wavefront + toolbar as before.
Keep printing min/avg/p95 with clear labels (`"grid_cache.update (static)"`, `"grid_cache.update (2 cells)"`, `"blit"`, `"overlay"`, `"draw_wavefront"`, `"draw_toolbar"`, `"frame total"`).

- [ ] **Step 8: Verify and record the result**

Run: `cargo build -p multosis` — compiles, no warnings.
Run: `cargo nextest run -p multosis` — PASS (143 + 2 `grid_cache` tests = 145).
Run: `cargo clippy -p multosis -- -D warnings` — clean.
Run: `cargo nextest run -p multosis --release bench_editor_draw --no-capture` — record the new timings. The static-grid `grid_cache.update` and the blit must be far cheaper than the Task 1 baseline `draw_grid`; the 2-cells case should be a small fraction of a full render.

The task report MUST quote the before (Task 1) and after numbers.

- [ ] **Step 9: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs
git commit -m "perf(multosis): cache the grid render, repaint only changed cells"
```

---

### Task 3: Verification

**Files:** none — checks, a re-measurement, and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS, 145 tests.
Run: `cargo nextest run --workspace` — PASS, 922 tests.
Run: `cargo clippy -p multosis -- -D warnings` — no warnings.
Run: `cargo fmt -p multosis -- --check` — clean (if it reports a diff, run `cargo fmt -p multosis` and commit it in Step 4).

- [ ] **Step 2: Re-measure against the success criterion**

Run: `cargo nextest run -p multosis --release bench_editor_draw --no-capture`

Compare to the Task 1 baseline. Expected: an unchanged-grid frame (the hover / region-drag / window-idle case) is now a cheap `grid_cache.update` cache-hit plus a blit — well under the spec's ~4 ms budget. A 2-cells-changed frame (the drag-paint case) re-renders only those cells and is also well under budget.

Report the comparison. **If the drag-paint (few-cells) case is still over the ~4 ms budget**, that means a single `draw_cell` is itself expensive (the anti-aliased `fill_triangle` calls) — record the numbers and flag it: a follow-up would make `fill_triangle` cheaper (the spec §3 candidate). Do not implement that here unless the data shows it is needed.

- [ ] **Step 3: Release build and bundle**

Run: `cargo build --bin multosis --release` — the standalone binary builds.
Run: `cargo nih-plug bundle multosis --release` — VST3 + CLAP bundle produced, no errors.

- [ ] **Step 4: Manual smoke test**

Run `cargo run --bin multosis`. Confirm:
- The editor looks identical to before — cells, arrowheads, the loop-region outline / nubs / move grip, the wavefront, the toolbar all render correctly.
- Mouse interaction (hovering, dragging the region edges/corners/grip, resizing the window) feels smooth — no lag.
- Drag-paint still paints correctly and feels responsive; the painted cells update live.
- The wavefront still animates while the transport runs.

Report the smoke-test observations and the before/after benchmark numbers.

- [ ] **Step 5: Commit (only if Step 1 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for the UI performance pass"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- The editor no longer repaints all 512 cells on every mouse event — `GridCache` blits an unchanged grid and re-renders only changed cells.
- `bench_editor_draw` shows an unchanged-grid frame well under the ~4 ms budget; the benchmark stays in the suite as a regression guard.
- `cargo nextest run -p multosis` is green (145 tests); `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles; the editor looks identical and interaction feels smooth.

## Spec coverage check (self-review)

- §1 Methodology (measure → fix → re-measure) — Task 1 measures the baseline, Task 2 fixes and re-measures, Task 3 verifies against §4.
- §2 The draw benchmark — `bench_editor_draw`, a permanent `#[test]` modelled on `bench_wavetable_draw`, dense worst-case grid, per-sub-function timings (Task 1; extended in Task 2 Step 7).
- §3 Candidate optimizations — the pass implements candidate #2 (whole-grid redraw → `GridCache`), which directly targets the observed symptom (per-event full repaint) and is correct independent of any micro-hotspot. Candidate #1 (cheaper triangle fills) is *not* implemented here: the cache makes a full cell render rare; Task 3 Step 2 measures whether the drag-paint case still needs it and flags a follow-up only if the data demands it (YAGNI, per the spec).
- §4 Success criterion — Task 3 Step 2 compares the re-measurement to the ~4 ms budget.
- §5 Unchanged / out of scope — no behaviour/visual change (the cache renders the identical cells; the `grid_cache_matches_an_uncached_cell_render` test pins this byte-for-byte); routing/propagation/audio untouched; no redraw throttle; no new dependency (the benchmark is a plain `#[test]`).
- §6 Testing — the benchmark (Task 1), the cache-correctness tests (Task 2), the re-measurement + smoke test (Task 3).

## Note on measurement-driven scope

Task 1 records the baseline; Task 2's fix (`GridCache`) is justified by the *structure* of the symptom — the editor redoing a fixed 512-cell repaint on every mouse event regardless of what changed — not by guessing which draw primitive is slow, so it is safe to plan ahead of the numbers. Task 3 Step 2 is the genuine measurement gate: if caching alone does not bring the drag-paint case under budget, the numbers (now in hand) point precisely at `fill_triangle`, and that becomes a small, well-targeted follow-up rather than a guess made now.
