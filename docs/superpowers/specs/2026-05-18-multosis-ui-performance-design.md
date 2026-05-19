# Multosis Editor UI Performance Pass — Design

**Date:** 2026-05-18
**Status:** Approved (brainstorming)
**Branch:** `multosis`

## Overview

The Multosis editor feels sluggish during mouse interaction (drag-paint, resizing, hovering). The editor is event-driven: every baseview event runs `MultosisWindow::on_frame` → `draw()`, which unconditionally repaints all 512 grid cells, the wavefront, and the toolbar. A stream of `CursorMoved` events therefore triggers a stream of full redraws; if a single `draw()` is too expensive, the editor cannot keep up with the cursor and lags.

This is a measurement-driven debugging task: instrument the render path, identify the dominant cost, fix it, and re-measure to prove the win. No optimization is applied on assumption alone.

## Background — the render path

- `multosis/src/editor.rs`: `on_frame` (called per baseview event) → `draw()`. `draw()` calls `widgets::fill_pixmap_opaque` (clear), then `grid_view::draw_grid`, `grid_view::draw_wavefront`, `toolbar::draw_toolbar`. There is no redraw throttle and no change detection.
- `grid_view::draw_grid` iterates all 16×32 = 512 cells, calling `draw_cell` on each, then draws the loop-region outline / handle nubs / move grip.
- `grid_view::draw_cell` draws the cell background via `widgets::draw_rect` (the opaque `BlendMode::Source` fast path — already cheap), then one filled triangle per lit send direction via `fill_triangle`, then the start-marker outline.
- `fill_triangle` builds a tiny-skia `PathBuilder` path and calls `pixmap.fill_path` with `anti_alias = true` — the full anti-aliased raster pipeline, once per arrowhead. A dense grid (many enabled cells, many sends) is ~1,000–4,000 such fills per frame; the worst case (512 cells × 8 directions) is 4,096.
- `toolbar::draw_toolbar` performs 5–6 `format!` heap allocations per frame for control labels and the status readout.
- The workspace already proves the relevant fast-path techniques: `tiny-skia-widgets`' opaque `draw_rect` (`BlendMode::Source`), pope-scope's direct-pixel-write `fill_column_opaque` ("~52% GUI CPU reduction"), and wavetable-filter's cached-pixmap `copy_from_slice` blit. `wavetable-filter` also has the benchmark pattern this pass will reuse: `bench_wavetable_draw`, a plain `#[test]` using `std::time::Instant` over N iterations with a min/avg/p95 print-out.

## §1 Methodology

1. **Instrument** — add a permanent draw benchmark (see §2) and record the baseline: the worst-case (dense-grid) `draw()` frame time, broken down per sub-function (`draw_grid`, `draw_wavefront`, `draw_toolbar`).
2. **Identify** — read the breakdown; the dominant cost is the target. The static survey's prime suspect is the anti-aliased triangle fills in `draw_grid`; the measurement confirms or redirects this.
3. **Fix** — apply the optimization for the measured hotspot (§3).
4. **Re-measure** — re-run the benchmark after each fix; the improvement must be shown, not assumed. Repeat from step 2 while `draw()` is still over budget (§4).

This ordering is load-bearing: the implementation plan front-loads the measurement task, and each fix task re-runs the benchmark to prove its win. If the measurement contradicts the survey's prediction, the remaining fix tasks are revised to target what the data shows.

## §2 The draw benchmark

A new permanent test in `multosis` — `bench_editor_draw` — modelled on `wavetable-filter`'s `bench_wavetable_draw`:

- A plain `#[test]` (not criterion), using `std::time::Instant`, with a warm-up and N timed iterations.
- It builds a **dense worst-case grid** (a large share of cells enabled, several sends per cell) and a `Pixmap` at the editor's window size, then times the render.
- It times the whole `draw`-equivalent render and, separately, `draw_grid` / `draw_wavefront` / `draw_toolbar`, so the per-sub-function split is visible.
- It prints min / avg / p95 per frame (a small timing-stats helper, mirroring `wavetable-filter`'s `print_timing_stats`).
- It stays in the test suite as a **regression guard**.

The benchmark must call the real render functions. Since `draw()` is a private method on `MultosisWindow` (which owns a baseview surface), the benchmark renders into a standalone `Pixmap` by calling the same public free functions `draw()` calls — `draw_grid`, `draw_wavefront`, `draw_toolbar` — plus `fill_pixmap_opaque`. (Implementation detail for the plan: the benchmark needs a `WavefrontDisplay`, a `MultosisParams`/`Grid`, a `TextRenderer`, and a `SeqStatusDisplay`, all constructible without a window.)

For representative numbers the benchmark is run in release (`cargo nextest run --release` or via `cargo xtask native`), since `target-cpu` affects tiny-skia codegen; it still runs (correctly, if slower) in the default test profile.

## §3 Candidate optimizations

Applied in measured-priority order; the benchmark decides which actually matter.

1. **Anti-aliased triangle fills (prime suspect).** Replace `fill_triangle`'s AA `fill_path` with a cheaper arrowhead rasterization — e.g. a non-anti-aliased / opaque scanline fill of the triangle, or precomputed arrowhead coverage. The arrowheads are tiny and opaque; the AA path pipeline is the expensive part. This is the optimization the connection-indicator spec explicitly deferred to this pass.
2. **Whole-grid redraw.** All 512 cells repaint every frame. Options: cache the rendered static grid as a `Pixmap` and blit it (`copy_from_slice`, the wavetable-filter technique), repainting only changed cells; or skip the grid redraw entirely when neither the grid nor the cursor-dependent overlay changed since the last frame (change detection on a generation counter).
3. **Toolbar per-frame allocations.** Replace the 5–6 `format!` calls with reused `String` buffers (`write!` into a cleared buffer) or avoid the allocation where the label is static.
4. **`cell_rect` recomputation.** If the measurement shows it matters, cache the 512 cell rectangles for the current `scale`.

Whichever optimizations the measurement does not justify are dropped (YAGNI) — the pass stops when §4 is met.

## §4 Success criterion

The worst-case (dense-grid) `draw()` time is brought comfortably within a 60 Hz frame budget with headroom to spare — a target of roughly ≤ 4 ms per frame (about a quarter of the 16.6 ms budget), so a rapid mouse-event stream does not back up and the editor stays responsive. The exact baseline and target are set by Task 1's measurement; the benchmark reports the before/after and remains in the suite. The pass is done when the benchmark shows the worst-case render under that budget and the manual smoke test confirms interaction feels smooth.

## §5 What is unchanged / out of scope

- No change to the editor's behaviour, layout, or visuals — arrowheads, cells, the wavefront and toolbar must look the same; only how they are rasterized changes.
- Routing, propagation, the audio engine — untouched (this is GUI-thread rendering only).
- A redraw throttle / fixed-rate frame timer is *not* in scope unless the measurement shows per-`draw()` optimization cannot reach §4 — in which case it is revisited then, not pre-emptively.
- No new dependency (e.g. criterion) — the benchmark is a plain `#[test]`, matching the workspace.

## §6 Testing

- The `bench_editor_draw` benchmark is itself the primary measurement instrument and a permanent regression guard.
- Each fix is covered by re-running the benchmark (the improvement is shown) and by the existing editor tests staying green (the optimizations must not change behaviour).
- A manual smoke test confirms the editor looks identical and interaction feels smooth.
