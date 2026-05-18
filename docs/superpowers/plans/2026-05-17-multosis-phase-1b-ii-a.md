# Multosis Phase 1 — Milestone 1b-ii-a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Multosis plugin a CPU-rendered editor window that draws the 16×32 octant grid and animates the live wavefront — display only, no interaction yet.

**Architecture:** A new `WavefrontDisplay` (an `[AtomicU32; 16]` mirror) carries the lit-set from the audio thread to the GUI. A softbuffer + tiny-skia + baseview editor (`editor.rs`, mirroring `gain-brain/src/editor.rs`) opens a window; `editor/grid_view.rs` renders the grid (cells, send pips, start markers, loop region) and overlays the wavefront each frame. Interaction — clicking cells, dragging loop-region handles, toolbar controls — is Milestone 1b-ii-b.

**Tech Stack:** Rust (nightly), nih-plug, baseview + softbuffer + tiny-skia + `tiny-skia-widgets`, `cargo nextest`.

**Reference:** `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7 (grid editor UI), §3.1 (the audio→GUI atomic wavefront mirror).

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** Every commit message must end with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` commands below omit it for brevity — add it to each.

**Pre-existing state:** The `multosis` crate is a working headless nih-plug plugin (Milestone 1b-i): `grid`/`region`/`randomize`/`propagation`/`clock`/`handoff`/`effects`/`engine` modules plus `lib.rs` (`MultosisParams`, `Multosis`, `impl Plugin`/`ClapPlugin`/`Vst3Plugin`), 75 tests green. `editor()` is not overridden (the plugin is currently GUI-less). `Grid` is `Copy`; `propagation::Wavefront` has `is_lit(row, col)`; `grid` exposes `ROWS = 16`, `COLS = 32`, `Grid::index`. `engine::AudioEngine` has `wavefront(&self) -> &Wavefront`.

---

### Task 1: GUI dependencies and embedded font

**Files:**
- Modify: `multosis/Cargo.toml`
- Create: `multosis/src/fonts/DejaVuSans.ttf` (copied)

- [ ] **Step 1: Add the GUI dependencies**

In `multosis/Cargo.toml`, replace the `[dependencies]` section with:

```toml
[dependencies]
nih_plug = { git = "https://github.com/xxx/nih-plug.git", branch = "finish-vst3-pr", features = ["standalone", "assert_process_allocs"] }
serde = { version = "1.0", features = ["derive"] }
baseview = { git = "https://github.com/RustAudio/baseview.git", tag = "v0.1.1", features = ["opengl"] }
softbuffer = { version = "0.4", default-features = false, features = ["kms", "x11"] }
raw-window-handle = "0.5"
raw-window-handle-06 = { package = "raw-window-handle", version = "0.6" }
tiny-skia = "0.12"
tiny-skia-widgets = { path = "../tiny-skia-widgets" }
```

Leave `[package]`, `[lib]`, `[[bin]]`, `[dev-dependencies]`, and `[package.metadata.bundler]` unchanged.

- [ ] **Step 2: Copy the embedded font**

Run:
```bash
mkdir -p multosis/src/fonts
cp gain-brain/src/fonts/DejaVuSans.ttf multosis/src/fonts/DejaVuSans.ttf
```

- [ ] **Step 3: Verify the crate still builds**

Run: `cargo build -p multosis`
Expected: the library and bin compile cleanly. (The new deps add baseview/softbuffer/tiny-skia to the build; the first build is slow — normal.)

- [ ] **Step 4: Verify tests still pass**

Run: `cargo nextest run -p multosis`
Expected: PASS — 75 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/Cargo.toml multosis/src/fonts/DejaVuSans.ttf Cargo.lock
git commit -m "feat(multosis): add GUI dependencies and embedded font"
```

---

### Task 2: `WavefrontDisplay` — the audio→GUI wavefront mirror

**Files:**
- Create: `multosis/src/wavefront_display.rs`
- Modify: `multosis/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/wavefront_display.rs`:

```rust
//! Lock-free audio→GUI mirror of the propagation wavefront.
//!
//! The audio thread publishes the lit-set once per process block; the editor
//! reads it each frame to draw the live wavefront. One `AtomicU32` per row
//! (bit `col` = cell lit) — 16 stores per publish, `Relaxed` ordering; a torn
//! read is sub-frame and visually irrelevant.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §3.1.

use crate::grid::{COLS, ROWS};
use crate::propagation::Wavefront;
use std::sync::atomic::{AtomicU32, Ordering};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_display_is_all_dark() {
        let d = WavefrontDisplay::new();
        for r in 0..ROWS {
            for c in 0..COLS {
                assert!(!d.is_lit(r, c));
            }
        }
    }

    #[test]
    fn publish_then_read_round_trips_the_wavefront() {
        let d = WavefrontDisplay::new();
        let mut wf = Wavefront::empty();
        wf.set(0, 0, true);
        wf.set(5, 17, true);
        wf.set(15, 31, true);
        d.publish(&wf);
        assert!(d.is_lit(0, 0));
        assert!(d.is_lit(5, 17));
        assert!(d.is_lit(15, 31));
        assert!(!d.is_lit(5, 18));
        assert!(!d.is_lit(8, 8));
    }

    #[test]
    fn publish_overwrites_the_previous_wavefront() {
        let d = WavefrontDisplay::new();
        let mut a = Wavefront::empty();
        a.set(3, 3, true);
        d.publish(&a);
        assert!(d.is_lit(3, 3));
        let b = Wavefront::empty();
        d.publish(&b);
        assert!(!d.is_lit(3, 3));
    }
}
```

Add `pub mod wavefront_display;` to `multosis/src/lib.rs` (with the other `pub mod` lines).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib wavefront_display`
Expected: build failure — `cannot find type WavefrontDisplay`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/wavefront_display.rs`, after the `use` lines (before the `#[cfg(test)]` module):

```rust
/// The audio→GUI wavefront mirror: one `AtomicU32` per grid row, bit `col`
/// set when cell `(row, col)` is lit.
pub struct WavefrontDisplay {
    rows: [AtomicU32; ROWS],
}

impl WavefrontDisplay {
    /// A display with every cell dark.
    pub fn new() -> Self {
        Self {
            rows: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    /// Audio thread: publish the current wavefront. One `Relaxed` store per row.
    pub fn publish(&self, wf: &Wavefront) {
        for r in 0..ROWS {
            let mut word = 0u32;
            for c in 0..COLS {
                if wf.is_lit(r, c) {
                    word |= 1 << c;
                }
            }
            self.rows[r].store(word, Ordering::Relaxed);
        }
    }

    /// GUI thread: is cell `(row, col)` lit in the last published wavefront?
    pub fn is_lit(&self, row: usize, col: usize) -> bool {
        (self.rows[row].load(Ordering::Relaxed) >> col) & 1 != 0
    }
}

impl Default for WavefrontDisplay {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib wavefront_display`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/wavefront_display.rs multosis/src/lib.rs
git commit -m "feat(multosis): add WavefrontDisplay audio-to-GUI mirror"
```

---

### Task 3: Wire `WavefrontDisplay` and `editor_state` into the plugin

**Files:**
- Modify: `multosis/src/lib.rs`

Plugin glue — verified by compilation. This task adds the `editor_state` persisted field, the `wavefront_display` plugin field, and publishes the wavefront each block. The `editor()` method is added in Task 4 (it needs `editor.rs`).

- [ ] **Step 1: Add the `editor_state` param field**

In `multosis/src/lib.rs`, in the `MultosisParams` struct, add as the FIRST field (before `grid`):

```rust
    /// Persisted editor window size.
    #[persist = "editor-state"]
    pub editor_state: std::sync::Arc<tiny_skia_widgets::EditorState>,
```

In `impl Default for MultosisParams`, add as the first field of the returned struct:

```rust
            editor_state: tiny_skia_widgets::EditorState::from_size(1056, 576),
```

(1056×576 is the editor's logical size — defined as `WINDOW_WIDTH`/`WINDOW_HEIGHT` in Task 4; the literal is used here to avoid a forward reference.)

- [ ] **Step 2: Add the `wavefront_display` plugin field**

In the `Multosis` struct, add a field:

```rust
    /// Audio→GUI wavefront mirror, shared with the editor.
    wavefront_display: Arc<crate::wavefront_display::WavefrontDisplay>,
```

In `impl Default for Multosis`, add to the returned struct:

```rust
            wavefront_display: Arc::new(crate::wavefront_display::WavefrontDisplay::new()),
```

- [ ] **Step 3: Publish the wavefront each process block**

In `impl Plugin for Multosis`, in `process()`, immediately AFTER the `self.engine.process(...)` call and BEFORE the output-gain loop, add:

```rust
        // Publish the wavefront for the editor to draw.
        self.wavefront_display.publish(self.engine.wavefront());
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. A `dead_code` warning for `wavefront_display` being written but not yet read (the `editor()` method that shares it arrives in Task 4) is EXPECTED — do not suppress it. No errors.

Run: `cargo nextest run -p multosis`
Expected: PASS — 75 tests.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/lib.rs
git commit -m "feat(multosis): wire WavefrontDisplay and editor-state into the plugin"
```

---

### Task 4: Editor scaffold and the `editor()` method

**Files:**
- Create: `multosis/src/editor.rs`
- Modify: `multosis/src/lib.rs`

Editor boilerplate — verified by compilation and a window opening. Adapted from `gain-brain/src/editor.rs`.

- [ ] **Step 1: Create the editor module**

Create `multosis/src/editor.rs`:

```rust
//! Softbuffer + tiny-skia CPU editor for Multosis.
//!
//! Milestone 1b-ii-a: opens the window and renders the grid + live wavefront.
//! Interaction (cell editing, loop-region drag, toolbar) is Milestone 1b-ii-b.

use baseview::{WindowOpenOptions, WindowScalePolicy};
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::wavefront_display::WavefrontDisplay;
use crate::MultosisParams;
use tiny_skia_widgets as widgets;

pub mod grid_view;

/// Logical editor size; physical size = logical × scale.
pub const WINDOW_WIDTH: u32 = 1056;
pub const WINDOW_HEIGHT: u32 = 576;

pub use widgets::EditorState;

/// The baseview window handler — owns the surface and draws each frame.
struct MultosisWindow {
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    /// Packed `(w << 32) | h` pending host-initiated resize, read next frame.
    pending_resize: Arc<AtomicU64>,
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    text_renderer: widgets::TextRenderer,
}

impl MultosisWindow {
    fn new(
        window: &mut baseview::Window<'_>,
        params: Arc<MultosisParams>,
        wavefront_display: Arc<WavefrontDisplay>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;
        let surface = widgets::SoftbufferSurface::new(window, pw, ph);
        let text_renderer =
            widgets::TextRenderer::new(include_bytes!("fonts/DejaVuSans.ttf"));
        Self {
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            pending_resize,
            params,
            wavefront_display,
            text_renderer,
        }
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }

    fn draw(&mut self) {
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg());
        // Grid, wavefront, and status drawing are added in later tasks.
    }
}

impl baseview::WindowHandler for MultosisWindow {
    fn on_frame(&mut self, window: &mut baseview::Window) {
        let packed = self.pending_resize.swap(0, Ordering::Relaxed);
        if packed != 0 {
            let new_w = (packed >> 32) as u32;
            let new_h = (packed & 0xFFFF_FFFF) as u32;
            if new_w > 0
                && new_h > 0
                && (new_w != self.physical_width || new_h != self.physical_height)
            {
                window.resize(baseview::Size::new(new_w as f64, new_h as f64));
            }
        }
        self.draw();
        self.surface.present();
    }

    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        if let baseview::Event::Window(baseview::WindowEvent::Resized(info)) = &event {
            self.physical_width = info.physical_size().width;
            self.physical_height = info.physical_size().height;
            self.scale_factor =
                (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
            self.resize_buffers();
        }
        baseview::EventStatus::Captured
    }
}

/// The nih-plug `Editor` — spawns the window.
struct MultosisEditor {
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
    pending_resize: Arc<AtomicU64>,
}

/// Build the editor. Returns `None` is never used — the editor always exists.
pub fn create(
    params: Arc<MultosisParams>,
    wavefront_display: Arc<WavefrontDisplay>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(MultosisEditor {
        params,
        wavefront_display,
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for MultosisEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        _context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);

        let params = Arc::clone(&self.params);
        let wavefront_display = Arc::clone(&self.wavefront_display);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Multosis"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                MultosisWindow::new(window, params, wavefront_display, pending_resize, sf)
            },
        );

        self.params.editor_state.set_open(true);
        Box::new(widgets::EditorHandle::new(
            self.params.editor_state.clone(),
            window,
        ))
    }

    fn size(&self) -> (u32, u32) {
        self.params.editor_state.size()
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        self.pending_resize
            .store(((width as u64) << 32) | (height as u64), Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}
```

- [ ] **Step 2: Wire the editor module and the `editor()` method into `lib.rs`**

In `multosis/src/lib.rs`, add `pub mod editor;` to the `pub mod` declarations.

In `impl Plugin for Multosis`, add the `editor()` method (after `params()`):

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.wavefront_display.clone())
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. `dead_code` warnings for the `MultosisWindow` fields `wavefront_display` and `text_renderer` (consumed by the rendering in Tasks 6–7) are EXPECTED — do not suppress them. (`params` is already read, by `resize_buffers`.) No errors. The plugin-side `wavefront_display` `dead_code` warning from Task 3 is now resolved — it is read by `editor()`.

Run: `cargo nextest run -p multosis`
Expected: PASS — 75 tests.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor.rs multosis/src/lib.rs
git commit -m "feat(multosis): add editor scaffold and the editor() method"
```

---

### Task 5: Grid geometry

**Files:**
- Create: `multosis/src/editor/grid_view.rs`

- [ ] **Step 1: Write the failing test**

Create `multosis/src/editor/grid_view.rs`:

```rust
//! Renders the 16×32 routing grid and the live wavefront into the editor
//! pixmap. Geometry is in logical units; every draw multiplies by `scale`.
//!
//! See `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §7.

use crate::grid::{Direction, Grid, COLS, ROWS};
use crate::wavefront_display::WavefrontDisplay;
use tiny_skia::Pixmap;
use tiny_skia_widgets as widgets;

/// Logical height of the top status strip.
pub const STATUS_H: f32 = 48.0;
/// Logical edge length of one square grid cell.
pub const CELL: f32 = 33.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_size_matches_the_grid() {
        // The editor window in editor.rs must exactly fit the grid.
        assert_eq!(crate::editor::WINDOW_WIDTH, (COLS as f32 * CELL) as u32);
        assert_eq!(
            crate::editor::WINDOW_HEIGHT,
            (STATUS_H + ROWS as f32 * CELL) as u32
        );
    }

    #[test]
    fn cell_rect_top_left_and_bottom_right() {
        let (x, y, w, h) = cell_rect(0, 0, 1.0);
        assert_eq!((x, y, w, h), (0.0, STATUS_H, CELL, CELL));
        let (x, y, _, _) = cell_rect(ROWS - 1, COLS - 1, 1.0);
        assert_eq!(x, (COLS - 1) as f32 * CELL);
        assert_eq!(y, STATUS_H + (ROWS - 1) as f32 * CELL);
    }

    #[test]
    fn cell_rect_scales() {
        let (x, y, w, h) = cell_rect(1, 2, 2.0);
        assert_eq!((x, y, w, h), (2.0 * CELL * 2.0, (STATUS_H + CELL) * 2.0, CELL * 2.0, CELL * 2.0));
    }

    #[test]
    fn cell_at_maps_a_point_back_to_a_cell() {
        // A point inside cell (3, 7) resolves to (3, 7).
        let (x, y, w, h) = cell_rect(3, 7, 1.5);
        let mid = (x + w / 2.0, y + h / 2.0);
        assert_eq!(cell_at(mid.0, mid.1, 1.5), Some((3, 7)));
        // A point in the status strip is not a cell.
        assert_eq!(cell_at(10.0, 5.0, 1.0), None);
        // A point past the grid is not a cell.
        assert_eq!(cell_at(100_000.0, 100_000.0, 1.0), None);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib grid_view`
Expected: build failure — `cannot find function cell_rect` (and `grid_view` is not yet a module — see Step 3).

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/editor/grid_view.rs`, after the `CELL` constant (before the `#[cfg(test)]` module):

```rust
/// The physical-pixel rectangle `(x, y, w, h)` of cell `(row, col)` at `scale`.
pub fn cell_rect(row: usize, col: usize, scale: f32) -> (f32, f32, f32, f32) {
    let x = col as f32 * CELL * scale;
    let y = (STATUS_H + row as f32 * CELL) * scale;
    let side = CELL * scale;
    (x, y, side, side)
}

/// The cell containing physical-pixel point `(px, py)` at `scale`, or `None`
/// if the point is in the status strip or outside the grid.
pub fn cell_at(px: f32, py: f32, scale: f32) -> Option<(usize, usize)> {
    if scale <= 0.0 || px < 0.0 {
        return None;
    }
    let logical_y = py / scale - STATUS_H;
    if logical_y < 0.0 {
        return None;
    }
    let col = (px / scale / CELL) as usize;
    let row = (logical_y / CELL) as usize;
    if row < ROWS && col < COLS {
        Some((row, col))
    } else {
        None
    }
}
```

The module is referenced as `pub mod grid_view;` inside `editor.rs` (already added in Task 4). No `lib.rs` change is needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib grid_view`
Expected: PASS — 4 tests.

Run: `cargo build -p multosis`
Expected: compiles (the `dead_code` warnings for the `MultosisWindow` fields and the unused `Direction`/`Grid`/`WavefrontDisplay`/`Pixmap`/`widgets` imports in `grid_view.rs` are EXPECTED — Tasks 6–7 consume them; do not suppress or remove them).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/editor/grid_view.rs
git commit -m "feat(multosis): add grid editor geometry"
```

---

### Task 6: Render the grid

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`

Rendering — verified by compilation and (in Task 8) a visual smoke test. No unit test for pixel output; the geometry it relies on is already tested.

- [ ] **Step 1: Add the grid-rendering functions**

Add to `multosis/src/editor/grid_view.rs`, after the `cell_at` function (before the `#[cfg(test)]` module):

```rust
/// Cell background when the cell is enabled.
fn color_cell_enabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x33, 0x37, 0x42, 0xFF)
}
/// Cell background when the cell is disabled.
fn color_cell_disabled() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x20, 0x22, 0x29, 0xFF)
}
/// A lit send-direction pip.
fn color_send() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x6f, 0x8a, 0xb8, 0xFF)
}
/// A start-cell marker.
fn color_start() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x5f, 0xd0, 0x9a, 0xFF)
}
/// The loop-region outline.
fn color_loop() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0x4f, 0xc3, 0xf7, 0xFF)
}

/// Draw one cell's background, send pips, and start marker.
fn draw_cell(pixmap: &mut Pixmap, row: usize, col: usize, cell: &crate::grid::Cell, scale: f32) {
    let (x, y, w, h) = cell_rect(row, col, scale);
    let gap = 1.0 * scale;
    // Background (inset by the gap so cells read as a grid).
    let bg = if cell.enabled {
        color_cell_enabled()
    } else {
        color_cell_disabled()
    };
    widgets::draw_rect(pixmap, x + gap, y + gap, w - 2.0 * gap, h - 2.0 * gap, bg);

    // Send pips: a small square pulled toward each sent direction.
    let cx = x + w / 2.0;
    let cy = y + h / 2.0;
    let pip = w * 0.16;
    for dir in Direction::ALL {
        if !cell.sends_to(dir) {
            continue;
        }
        let (dr, dc) = dir.delta();
        let px = cx + dc as f32 * w * 0.34 - pip / 2.0;
        let py = cy + dr as f32 * h * 0.34 - pip / 2.0;
        widgets::draw_rect(pixmap, px, py, pip, pip, color_send());
    }

    // Start marker: a thin inset outline.
    if cell.is_start {
        widgets::draw_rect_outline(
            pixmap,
            x + gap,
            y + gap,
            w - 2.0 * gap,
            h - 2.0 * gap,
            color_start(),
            1.5 * scale,
        );
    }
}

/// Draw the whole grid — every cell, then the loop-region outline.
pub fn draw_grid(pixmap: &mut Pixmap, grid: &Grid, scale: f32) {
    for r in 0..ROWS {
        for c in 0..COLS {
            draw_cell(pixmap, r, c, grid.cell(r, c), scale);
        }
    }
    // Loop-region outline: a rectangle spanning the region's cells.
    let lr = grid.loop_region;
    let (x0, y0, _, _) = cell_rect(lr.row0, lr.col0, scale);
    let (x1, y1, w1, h1) = cell_rect(lr.row1, lr.col1, scale);
    widgets::draw_rect_outline(
        pixmap,
        x0,
        y0,
        (x1 + w1) - x0,
        (y1 + h1) - y0,
        color_loop(),
        2.0 * scale,
    );
}
```

- [ ] **Step 2: Call `draw_grid` from the window's `draw`**

In `multosis/src/editor.rs`, replace the body of `MultosisWindow::draw` with:

```rust
    fn draw(&mut self) {
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg());
        let grid = self
            .params
            .grid
            .lock()
            .map(|g| *g)
            .unwrap_or_default();
        grid_view::draw_grid(&mut self.surface.pixmap, &grid, self.scale_factor);
        // The wavefront overlay and status strip are added in Task 7.
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p multosis`
Expected: compiles. The remaining expected `dead_code` warnings are for the `MultosisWindow` fields `wavefront_display` and `text_renderer` (consumed in Task 7). No errors.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs
git commit -m "feat(multosis): render the routing grid in the editor"
```

---

### Task 7: Render the wavefront overlay and status strip

**Files:**
- Modify: `multosis/src/editor/grid_view.rs`
- Modify: `multosis/src/editor.rs`

- [ ] **Step 1: Add the wavefront and status rendering**

Add to `multosis/src/editor/grid_view.rs`, after the `draw_grid` function (before the `#[cfg(test)]` module):

```rust
/// A lit wavefront cell.
fn color_wavefront() -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(0xd8, 0x89, 0x3a, 0xFF)
}

/// Overlay the live wavefront — every lit cell gets an orange core square.
pub fn draw_wavefront(pixmap: &mut Pixmap, wf: &WavefrontDisplay, scale: f32) {
    for r in 0..ROWS {
        for c in 0..COLS {
            if !wf.is_lit(r, c) {
                continue;
            }
            let (x, y, w, h) = cell_rect(r, c, scale);
            let inset = w * 0.22;
            widgets::draw_rect(
                pixmap,
                x + inset,
                y + inset,
                w - 2.0 * inset,
                h - 2.0 * inset,
                color_wavefront(),
            );
        }
    }
}

/// Draw the top status strip — the plugin title.
pub fn draw_status(pixmap: &mut Pixmap, tr: &mut widgets::TextRenderer, scale: f32) {
    let strip_h = STATUS_H * scale;
    widgets::draw_rect(
        pixmap,
        0.0,
        0.0,
        pixmap.width() as f32,
        strip_h,
        widgets::color_control_bg(),
    );
    let size = 20.0 * scale;
    tr.draw_text(
        pixmap,
        12.0 * scale,
        strip_h / 2.0 + size * 0.36,
        "MULTOSIS",
        size,
        widgets::color_text(),
    );
}
```

- [ ] **Step 2: Call them from the window's `draw`**

In `multosis/src/editor.rs`, replace the body of `MultosisWindow::draw` with:

```rust
    fn draw(&mut self) {
        widgets::fill_pixmap_opaque(&mut self.surface.pixmap, widgets::color_bg());
        let grid = self
            .params
            .grid
            .lock()
            .map(|g| *g)
            .unwrap_or_default();
        grid_view::draw_grid(&mut self.surface.pixmap, &grid, self.scale_factor);
        grid_view::draw_wavefront(
            &mut self.surface.pixmap,
            &self.wavefront_display,
            self.scale_factor,
        );
        grid_view::draw_status(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            self.scale_factor,
        );
    }
```

- [ ] **Step 3: Verify it compiles warning-free**

Run: `cargo build -p multosis`
Expected: compiles with NO warnings — every `MultosisWindow` field and every `grid_view.rs` import is now consumed.

Run: `cargo nextest run -p multosis`
Expected: PASS — 82 tests (the 75 from Milestone 1b-i, plus Task 2's 3 and Task 5's 4).

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/grid_view.rs multosis/src/editor.rs
git commit -m "feat(multosis): render the wavefront overlay and status strip"
```

---

### Task 8: Milestone 1b-ii-a verification

**Files:** none — checks and a manual smoke test only.

- [ ] **Step 1: Full test suite**

Run: `cargo nextest run -p multosis`
Expected: PASS — all tests green (82: the 75 from Milestone 1b-i, plus `WavefrontDisplay` ×3 and `grid_view` geometry ×4).

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

Run the standalone binary (`cargo run --bin multosis`). Confirm:
- A 1056×576 window opens with a dark background and a "MULTOSIS" status strip across the top.
- The 16×32 grid is drawn: each cell shows its East-only default send pip, the left column shows green start-cell outlines, and a blue loop-region outline frames the whole grid.
- With audio playing and the host transport running, an orange wavefront sweeps left-to-right across the grid, one column per `speed` step.
- Resizing the window rescales the grid cleanly.

Report the smoke-test observations. (This step is a human/visual check — it cannot be unit-tested.)

- [ ] **Step 5: Commit (only if Step 2 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for milestone 1b-ii-a"
```

If Step 2 produced no edits, skip this commit.

---

## Milestone 1b-ii-a — definition of done

- The plugin opens a CPU-rendered editor window showing the 16×32 octant grid (cell enable state, send pips, start markers, loop-region outline) and the live orange wavefront animating as the sequence plays.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles (VST3 + CLAP + standalone).
- The editor is display-only — no cell editing, no drag, no toolbar controls. That is Milestone 1b-ii-b.

## Spec coverage check (self-review)

- §3.1 audio→GUI wavefront mirror — `WavefrontDisplay` (`[AtomicU32; 16]`), Task 2; published each block, Task 3.
- §7 grid editor — the resizable softbuffer/tiny-skia editor (Task 4), the 16×32 octant-cell grid with send pips, enable state, start markers, loop-region outline (Tasks 5–6), the live wavefront in orange (Task 7), the top status strip (Task 7).
- Out of scope (Milestone 1b-ii-b): octant click hit-testing to toggle sends/enable/start (the `cell_at` helper is built now as the seam), loop-region drag handles, and the toolbar controls + the six grid operations.
