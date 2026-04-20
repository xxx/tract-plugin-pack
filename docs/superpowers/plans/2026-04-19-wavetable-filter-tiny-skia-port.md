# Wavetable Filter — tiny-skia Port — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the Wavetable Filter's editor from `nih_plug_vizia` (OpenGL) to the softbuffer + tiny-skia + baseview + fontdue stack used by the rest of the plugin pack. DSP is untouched.

**Architecture:** Replace the `editor/` module with a `WavetableFilterWindow` mirroring the warp-zone / satch pattern. The audio → GUI interop in `lib.rs` already flows through `Arc<Mutex<…>>` / `Arc<AtomicU32>` handles and stays unchanged aside from dropping `ui_scale` and swapping `ViziaState` for `widgets::EditorState`.

**Tech Stack:** Rust, baseview, softbuffer, tiny-skia, fontdue, nih-plug (fork branch `finish-vst3-pr`), `tiny-skia-widgets` (workspace crate), `keyboard-types`, `rfd`.

**Spec:** [docs/superpowers/specs/2026-04-19-wavetable-filter-tiny-skia-port.md](../specs/2026-04-19-wavetable-filter-tiny-skia-port.md)

**Reference template:** `warp-zone/src/editor.rs` is the closest-shape reference. When in doubt, match its conventions (ParamId enum, HitAction enum, draw() / on_event() shape, SoftbufferSurface usage, EditorHandle, EditorState persist). The one difference: wavetable-filter has additional shared state (`shared_wavetable`, `wavetable_version`, `shared_input_spectrum`, `should_reload`, `pending_reload`) that flow through the `create()` signature.

---

## Task 1: Cargo.toml — swap dependencies

**Files:**
- Modify: `wavetable-filter/Cargo.toml`

- [ ] **Step 1: Replace `[dependencies]` block**

Current deps (from warp-zone reference and the spec's dependency list). Open `wavetable-filter/Cargo.toml` and replace the `[dependencies]` section with:

```toml
[dependencies]
nih_plug = { git = "https://github.com/xxx/nih-plug.git", branch = "finish-vst3-pr", features = ["simd", "standalone", "assert_process_allocs"] }
baseview = { git = "https://github.com/RustAudio/baseview.git", rev = "9a0b42c09d712777b2edb4c5e0cb6baf21e988f0", features = ["opengl"] }
softbuffer = { version = "0.4", default-features = false, features = ["kms", "x11"] }
raw-window-handle = "0.5"
raw-window-handle-06 = { package = "raw-window-handle", version = "0.6" }
tiny-skia = "0.12"
tiny-skia-widgets = { path = "../tiny-skia-widgets" }
keyboard-types = "0.6"
crossbeam = "0.8"
hound = "3.5"
atomic_float = "1.0"
rfd = "0.17.2"
realfft = "3.3"
rustfft = "6.2"
```

Removed: `nih-plug-widgets = { path = "../nih-plug-widgets" }`, `nih_plug_vizia = { git = ... }`. Added: `baseview`, `softbuffer`, `raw-window-handle`, `raw-window-handle-06`, `tiny-skia`, `tiny-skia-widgets`, `keyboard-types`, `crossbeam`.

- [ ] **Step 2: Verify the manifest parses**

Run: `cargo tree -p wavetable-filter 2>&1 | head -20`
Expected: no parse error from Cargo; the plugin will not compile yet because `lib.rs` still references vizia — that's fixed in Task 2. Do NOT run `cargo check` yet.

- [ ] **Step 3: Commit**

```bash
git add wavetable-filter/Cargo.toml
git commit -m "wavetable-filter: swap vizia deps for softbuffer + tiny-skia"
```

---

## Task 2: lib.rs — drop `ui_scale` and vizia wiring

**Files:**
- Modify: `wavetable-filter/src/lib.rs`

The goal of this task is to get `lib.rs` to compile against a stub `editor` module that exports only what `lib.rs` needs (`EditorState`, `default_editor_state`, `WINDOW_WIDTH`, `WINDOW_HEIGHT`, `create`). Later tasks fill in the real implementations.

- [ ] **Step 1: Remove the `ui_scale` field from `WavetableFilterParams`**

In `wavetable-filter/src/lib.rs`, find the `#[derive(Params)] struct WavetableFilterParams` block (around line 133). Remove these two lines:

```rust
    #[id = "ui_scale"]
    pub ui_scale: IntParam,
```

- [ ] **Step 2: Remove the `ui_scale` initializer**

In the `impl WavetableFilterParams { pub fn new(...) }` around line 740. Remove the line:

```rust
            ui_scale: IntParam::new("UI Scale", 100, IntRange::Linear { min: 100, max: 300 })
                // ...(any chained builder calls on ui_scale)
                .non_automatable(),
```

The full original block is a `.non_automatable()`-chained builder. Delete the entire `ui_scale: IntParam::new(...)...,` entry (it will span multiple lines).

- [ ] **Step 3: Swap `editor_state` field type**

Find (around line 34):

```rust
    editor_state: Arc<nih_plug_vizia::ViziaState>,
```

Replace with:

```rust
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,
```

Make sure this field lives in `WavetableFilterParams` (alongside `wavetable_path`), matching the warp-zone pattern. If it's currently on the plugin struct rather than the params struct, move it to params. Search for `editor_state` usages and update them.

- [ ] **Step 4: Simplify `initialize()`**

Find the `fn initialize(...)` block (around line 853). Delete the vizia scale-derivation chunk that currently reads `ui_scale` and rebuilds the `ViziaState` (approximately lines 862-871 in the current file):

```rust
        // Sync editor scale from the persisted ui_scale parameter
        let scale_pct = self.params.ui_scale.value() as f64;
        let scale = (scale_pct / 100.0).clamp(1.0, 3.0);
        // Write into ViziaState so next editor open uses this scale
        let new_state = nih_plug_vizia::ViziaState::new_with_default_scale_factor(
            || (editor::WINDOW_WIDTH, editor::WINDOW_HEIGHT),
            scale,
        );
        let new_inner = Arc::try_unwrap(new_state).unwrap();
        nih_plug::params::persist::PersistentField::set(&self.editor_state, new_inner);
```

Keep everything else in `initialize()` (wavetable loading, initial kernel synthesis, etc.) intact.

- [ ] **Step 5: Rewrite `editor()`**

Find `fn editor(...)` (around line 840). Replace the entire method body with:

```rust
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.should_reload.clone(),
            self.pending_reload.clone(),
            self.shared_wavetable.clone(),
            self.wavetable_version.clone(),
            self.shared_input_spectrum.clone(),
        )
    }
```

`wavetable_path` is reached from `self.params.wavetable_path` by the editor — no need to pass it separately. `editor_state` lives on `self.params.editor_state` now.

- [ ] **Step 6: Replace the vizia `editor` module declaration**

Find `mod editor;` or `pub mod editor;` near the top of `lib.rs`. Leave it as `pub mod editor;` (module is declared in `src/editor.rs`, which we rewrite in Task 4). In the meantime, we also need to stub the old `src/editor.rs` + `src/editor/` so the crate compiles at this point.

Delete the body of `wavetable-filter/src/editor.rs` and temporarily replace it with this stub (we will rewrite it entirely in Task 4):

```rust
//! Stub — real implementation follows in Task 4.

use nih_plug::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Mutex;
use tiny_skia_widgets as widgets;

use crate::{PendingReload, WavetableFilterParams};

pub const WINDOW_WIDTH: u32 = 900;
pub const WINDOW_HEIGHT: u32 = 640;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

#[allow(clippy::too_many_arguments, unused_variables)]
pub fn create(
    _params: Arc<WavetableFilterParams>,
    _should_reload: Arc<AtomicBool>,
    _pending_reload: Arc<Mutex<Option<PendingReload>>>,
    _shared_wavetable: Arc<Mutex<crate::wavetable::Wavetable>>,
    _wavetable_version: Arc<AtomicU32>,
    _shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    None
}
```

Delete (via `git rm`) the now-unneeded submodule files:

```bash
git rm wavetable-filter/src/editor/wavetable_view.rs wavetable-filter/src/editor/filter_response_view.rs
rmdir wavetable-filter/src/editor
```

- [ ] **Step 7: Remove leftover vizia imports in `lib.rs`**

`cargo check -p wavetable-filter` and fix the remaining errors. Typical leftovers to remove:

```rust
use nih_plug_vizia::ViziaState;   // delete
```

And any `nih_plug_vizia::…` references elsewhere in `lib.rs`.

- [ ] **Step 8: Update `default_state()` callers**

If `lib.rs` previously called `editor::default_state()` (vizia name), it should now call `editor::default_editor_state()`. Update the initializer in `WavetableFilterParams::new` around line 144 or wherever `editor_state` is constructed:

```rust
            editor_state: editor::default_editor_state(),
```

- [ ] **Step 9: Verify `cargo check -p wavetable-filter` passes**

Run: `cargo check -p wavetable-filter 2>&1 | tail -30`
Expected: `Finished dev profile` with no errors. Warnings about unused vars in the stub `create()` are fine.

- [ ] **Step 10: Run existing DSP tests**

Run: `cargo test -p wavetable-filter --lib 2>&1 | tail -10`
Expected: all 30 existing DSP tests pass (wavetable tests + filter tests). The editor is stubbed but the DSP is untouched.

- [ ] **Step 11: Commit**

```bash
git add wavetable-filter/src/lib.rs wavetable-filter/src/editor.rs
git commit -m "wavetable-filter: drop ui_scale param and vizia wiring, stub editor"
```

---

## Task 3: Copy the embedded font

**Files:**
- Create: `wavetable-filter/src/fonts/DejaVuSans.ttf` (binary, copied from a sibling plugin)

- [ ] **Step 1: Copy the font file**

```bash
mkdir -p wavetable-filter/src/fonts
cp warp-zone/src/fonts/DejaVuSans.ttf wavetable-filter/src/fonts/DejaVuSans.ttf
```

- [ ] **Step 2: Verify**

Run: `ls -la wavetable-filter/src/fonts/`
Expected: `DejaVuSans.ttf` present, same size as in warp-zone (~757KB).

- [ ] **Step 3: Commit**

```bash
git add wavetable-filter/src/fonts/DejaVuSans.ttf
git commit -m "wavetable-filter: add embedded DejaVuSans font"
```

---

## Task 4: Editor skeleton — blank window

**Files:**
- Rewrite: `wavetable-filter/src/editor.rs`

This task replaces the stub with a full `WavetableFilterWindow` that opens, renders a solid background, and handles host-initiated resize. Views and layout come in later tasks.

- [ ] **Step 1: Rewrite `editor.rs`**

Replace the entire contents of `wavetable-filter/src/editor.rs` with:

```rust
//! Softbuffer-based editor for Wavetable Filter. CPU rendering via tiny-skia.
//!
//! Layout (900x640, freely resizable):
//! - Top strip (~32px): Browse button + wavetable name + mode stepped selector
//! - Main area: wavetable view (left) + filter response view (right)
//! - Dials below each view: Frame | Frequency, Resonance, Drive, Mix

pub mod filter_response_view;
pub mod wavetable_view;

use baseview::{WindowOpenOptions, WindowScalePolicy};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tiny_skia_widgets as widgets;

use crate::wavetable::Wavetable;
use crate::{PendingReload, WavetableFilterParams};

pub const WINDOW_WIDTH: u32 = 900;
pub const WINDOW_HEIGHT: u32 = 640;
const MIN_WIDTH: u32 = 700;
const MIN_HEIGHT: u32 = 500;

pub use widgets::EditorState;

pub fn default_editor_state() -> Arc<EditorState> {
    EditorState::from_size(WINDOW_WIDTH, WINDOW_HEIGHT)
}

// ── Hit actions ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ParamId {
    Frame,
    Frequency,
    Resonance,
    Drive,
    Mix,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ButtonAction {
    Browse,
    WavetableToggle2D3D,
    /// 0 = Raw, 1 = Phaseless. Matches the EnumParam::variants() order.
    Mode(u8),
}

// ── Window handler ──────────────────────────────────────────────────────

struct WavetableFilterWindow {
    gui_context: Arc<dyn GuiContext>,
    surface: widgets::SoftbufferSurface,
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,

    params: Arc<WavetableFilterParams>,
    text_renderer: widgets::TextRenderer,
    drag: widgets::DragState<HitAction>,
    text_edit: widgets::TextEditState<HitAction>,

    // Audio → GUI plumbing
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,

    // View-local state
    show_2d: bool,
    frame_cache: wavetable_view::FrameCache,
    fft_cache: filter_response_view::FftCache,
}

impl WavetableFilterWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        window: &mut baseview::Window<'_>,
        gui_context: Arc<dyn GuiContext>,
        params: Arc<WavetableFilterParams>,
        should_reload: Arc<AtomicBool>,
        pending_reload: Arc<Mutex<Option<PendingReload>>>,
        shared_wavetable: Arc<Mutex<Wavetable>>,
        wavetable_version: Arc<AtomicU32>,
        shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
        shared_scale: Arc<AtomicCell<f32>>,
        pending_resize: Arc<AtomicU64>,
        scale_factor: f32,
    ) -> Self {
        let pw = (WINDOW_WIDTH as f32 * scale_factor).round() as u32;
        let ph = (WINDOW_HEIGHT as f32 * scale_factor).round() as u32;

        let surface = widgets::SoftbufferSurface::new(window, pw, ph);

        let font_data = include_bytes!("fonts/DejaVuSans.ttf");
        let text_renderer = widgets::TextRenderer::new(font_data);

        Self {
            gui_context,
            surface,
            physical_width: pw,
            physical_height: ph,
            scale_factor,
            shared_scale,
            pending_resize,
            params,
            text_renderer,
            drag: widgets::DragState::new(),
            text_edit: widgets::TextEditState::new(),
            should_reload,
            pending_reload,
            shared_wavetable,
            wavetable_version,
            shared_input_spectrum,
            show_2d: false,
            frame_cache: wavetable_view::FrameCache::new(),
            fft_cache: filter_response_view::FftCache::new(),
        }
    }

    fn draw(&mut self) {
        // Full-frame clear; layout comes in Task 5.
        self.surface.pixmap.fill(widgets::color_bg());
    }

    fn resize_buffers(&mut self) {
        let pw = self.physical_width.max(1);
        let ph = self.physical_height.max(1);
        self.surface.resize(pw, ph);
        self.params.editor_state.store_size(pw, ph);
    }
}

impl baseview::WindowHandler for WavetableFilterWindow {
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
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width.max(MIN_WIDTH);
                self.physical_height = info.physical_size().height.max(MIN_HEIGHT);
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }
            _ => {}
        }
        baseview::EventStatus::Captured
    }
}

// ── Editor trait implementation ─────────────────────────────────────────

pub(crate) struct WavetableFilterEditor {
    params: Arc<WavetableFilterParams>,
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
    scaling_factor: Arc<AtomicCell<f32>>,
    pending_resize: Arc<AtomicU64>,
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    params: Arc<WavetableFilterParams>,
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    Some(Box::new(WavetableFilterEditor {
        params,
        should_reload,
        pending_reload,
        shared_wavetable,
        wavetable_version,
        shared_input_spectrum,
        scaling_factor: Arc::new(AtomicCell::new(1.0)),
        pending_resize: Arc::new(AtomicU64::new(0)),
    }))
}

impl Editor for WavetableFilterEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let (persisted_w, persisted_h) = self.params.editor_state.size();
        let sf = (persisted_w as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
        self.scaling_factor.store(sf);

        let gui_context = Arc::clone(&context);
        let params = Arc::clone(&self.params);
        let should_reload = Arc::clone(&self.should_reload);
        let pending_reload = Arc::clone(&self.pending_reload);
        let shared_wavetable = Arc::clone(&self.shared_wavetable);
        let wavetable_version = Arc::clone(&self.wavetable_version);
        let shared_input_spectrum = Arc::clone(&self.shared_input_spectrum);
        let shared_scale = Arc::clone(&self.scaling_factor);
        let pending_resize = Arc::clone(&self.pending_resize);

        let window = baseview::Window::open_parented(
            &widgets::ParentWindowHandleAdapter(parent),
            WindowOpenOptions {
                title: String::from("Wavetable Filter"),
                size: baseview::Size::new(persisted_w as f64, persisted_h as f64),
                scale: WindowScalePolicy::ScaleFactor(1.0),
                gl_config: None,
            },
            move |window| {
                WavetableFilterWindow::new(
                    window,
                    gui_context,
                    params,
                    should_reload,
                    pending_reload,
                    shared_wavetable,
                    wavetable_version,
                    shared_input_spectrum,
                    shared_scale,
                    pending_resize,
                    sf,
                )
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

    fn set_scale_factor(&self, factor: f32) -> bool {
        if self.params.editor_state.is_open() {
            return false;
        }
        self.scaling_factor.store(factor);
        true
    }

    fn set_size(&self, width: u32, height: u32) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let packed = ((width as u64) << 32) | (height as u64);
        self.pending_resize.store(packed, Ordering::Relaxed);
        true
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}
```

- [ ] **Step 2: Create minimal `wavetable_view.rs` and `filter_response_view.rs` stubs**

Create `wavetable-filter/src/editor/wavetable_view.rs`:

```rust
//! Wavetable visualization (2D face-on / 3D overhead). Rewritten in Task 8.

pub(crate) struct FrameCache {
    pub cached_frames: Vec<Vec<f32>>,
    pub cached_version: u32,
    pub cached_frame_count: usize,
    pub cached_frame_size: usize,
    pub global_min: f32,
    pub global_max: f32,
}

impl FrameCache {
    pub fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX,
            cached_frame_count: 0,
            cached_frame_size: 0,
            global_min: 0.0,
            global_max: 0.0,
        }
    }
}
```

Create `wavetable-filter/src/editor/filter_response_view.rs`:

```rust
//! Filter response + input spectrum visualization. Rewritten in Task 9.

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;

pub(crate) struct FftCache {
    pub planner: RealFftPlanner<f32>,
    pub frame_buf: Vec<f32>,
    pub spectrum: Vec<Complex<f32>>,
    pub cached_mags: Vec<f32>,
    pub cached_frame_pos: f32,
    pub cached_cutoff: f32,
    pub cached_resonance: f32,
    pub freq_table: Vec<f32>,
    pub freq_table_size: usize,
    pub cached_response_ys: Vec<f32>,
    pub cached_input_mags: Vec<f32>,
    pub cached_input_sr: f32,
}

impl FftCache {
    pub fn new() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            frame_buf: Vec::new(),
            spectrum: Vec::new(),
            cached_mags: Vec::new(),
            cached_frame_pos: -1.0,
            cached_cutoff: -1.0,
            cached_resonance: -1.0,
            freq_table: Vec::new(),
            freq_table_size: 0,
            cached_response_ys: Vec::new(),
            cached_input_mags: Vec::new(),
            cached_input_sr: 0.0,
        }
    }
}
```

- [ ] **Step 3: Verify compile**

Run: `cargo check -p wavetable-filter 2>&1 | tail -20`
Expected: clean. Warnings about unused fields are fine.

- [ ] **Step 4: Build and launch standalone**

Run: `cargo build --bin wavetable-filter 2>&1 | tail -5`
Expected: `Finished`. Then: `./target/debug/wavetable-filter &` (kill it after verifying the window opens). A blank 900x640 window with a dark grey background should appear. Free-resize works; minimum is 700x500.

- [ ] **Step 5: Commit**

```bash
git add wavetable-filter/src/editor.rs wavetable-filter/src/editor/
git commit -m "wavetable-filter: editor skeleton with blank softbuffer surface"
```

---

## Task 5: Top strip — Browse, path, mode selector

**Files:**
- Modify: `wavetable-filter/src/editor.rs`

Adds the header strip: Browse button, wavetable name label, and a two-segment mode selector (Raw | Phaseless). Also wires up the file dialog. No dial rendering yet.

- [ ] **Step 1: Add constants and helpers**

In `editor.rs`, above the `WavetableFilterWindow` struct, add:

```rust
const TOP_STRIP_H: f32 = 32.0;
const STRIP_PAD: f32 = 8.0;

fn format_wavetable_label(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("(none)")
        .to_string()
}
```

- [ ] **Step 2: Add helper methods on `WavetableFilterWindow`**

Inside `impl WavetableFilterWindow`, above `fn draw`, add:

```rust
    fn float_param(&self, id: ParamId) -> &FloatParam {
        match id {
            ParamId::Frame => &self.params.frame_position,
            ParamId::Frequency => &self.params.frequency,
            ParamId::Resonance => &self.params.resonance,
            ParamId::Drive => &self.params.drive,
            ParamId::Mix => &self.params.mix,
        }
    }

    fn begin_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.begin_set_parameter(self.float_param(id));
    }

    fn set_param_normalized(&self, setter: &ParamSetter, id: ParamId, normalized: f32) {
        setter.set_parameter_normalized(self.float_param(id), normalized);
    }

    fn end_set_param(&self, setter: &ParamSetter, id: ParamId) {
        setter.end_set_parameter(self.float_param(id));
    }

    fn reset_param_to_default(&self, setter: &ParamSetter, id: ParamId) {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, p.default_normalized_value());
        setter.end_set_parameter(p);
    }

    fn format_value(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        p.normalized_value_to_string(p.modulated_normalized_value(), true)
    }

    fn formatted_value_without_unit(&self, id: ParamId) -> String {
        use nih_plug::prelude::Param;
        let p = self.float_param(id);
        p.normalized_value_to_string(p.modulated_normalized_value(), false)
    }

    fn commit_text_edit(&mut self) {
        use nih_plug::prelude::Param;
        let Some((action, text)) = self.text_edit.commit() else {
            return;
        };
        let HitAction::Dial(param_id) = action else {
            return;
        };
        let p = self.float_param(param_id);
        let Some(norm) = p.string_to_normalized_value(&text) else {
            return;
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        self.begin_set_param(&setter, param_id);
        self.set_param_normalized(&setter, param_id, norm);
        self.end_set_param(&setter, param_id);
    }

    fn set_mode(&self, variant: u8) {
        use crate::FilterMode;
        use nih_plug::prelude::Param;
        let target = match variant {
            0 => FilterMode::Raw,
            _ => FilterMode::Minimum,
        };
        let setter = ParamSetter::new(self.gui_context.as_ref());
        let norm = self.params.mode.preview_normalized(target);
        setter.begin_set_parameter(&self.params.mode);
        setter.set_parameter_normalized(&self.params.mode, norm);
        setter.end_set_parameter(&self.params.mode);
    }

    fn open_file_dialog(&mut self) {
        use nih_plug::nih_log;

        let mut dialog = rfd::FileDialog::new().add_filter("Wavetable files", &["wav", "wt"]);
        if let Ok(current) = self.params.wavetable_path.lock() {
            if let Some(dir) = std::path::Path::new(current.as_str()).parent() {
                if dir.exists() {
                    dialog = dialog.set_directory(dir);
                }
            }
        }
        let Some(path) = dialog.pick_file() else {
            return;
        };
        let Some(path_str) = path.to_str() else { return };
        let path_string = path_str.to_string();

        let new_wavetable = match Wavetable::from_file(&path_string) {
            Ok(wt) => wt,
            Err(e) => {
                nih_log!("Wavetable load error: {e}");
                return;
            }
        };

        // Pre-allocate FFT scratch on the GUI thread — audio thread stays allocation-free.
        let new_size = new_wavetable.frame_size;
        let spec_len = new_size / 2 + 1;
        let mut planner = realfft::RealFftPlanner::<f32>::new();
        let frame_fft = planner.plan_fft_forward(new_size);
        let reload = PendingReload {
            wavetable: new_wavetable.clone(),
            frame_fft,
            frame_cache: vec![0.0; new_size],
            frame_buf: vec![0.0; new_size],
            frame_spectrum: vec![rustfft::num_complex::Complex::new(0.0, 0.0); spec_len],
            frame_mags: vec![0.0; spec_len],
        };

        if let Ok(mut pending) = self.pending_reload.lock() {
            *pending = Some(reload);
        }
        if let Ok(mut shared) = self.shared_wavetable.lock() {
            *shared = new_wavetable;
        }
        self.wavetable_version.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut wt) = self.params.wavetable_path.lock() {
            *wt = path_string.clone();
        }
        self.should_reload.store(true, Ordering::Relaxed);
    }
```

- [ ] **Step 3: Implement top strip drawing in `draw()`**

Replace the `fn draw(&mut self)` body with:

```rust
    fn draw(&mut self) {
        let s = self.scale_factor;

        self.drag.clear_regions();
        self.surface.pixmap.fill(widgets::color_bg());

        let w = self.physical_width as f32;

        // ── Top strip: Browse | path | Mode selector ──
        let strip_y = 0.0;
        let strip_h = TOP_STRIP_H * s;
        let pad = STRIP_PAD * s;

        let browse_w = 72.0 * s;
        let browse_h = 22.0 * s;
        let browse_x = pad;
        let browse_y = strip_y + (strip_h - browse_h) * 0.5;

        widgets::draw_button(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            browse_x,
            browse_y,
            browse_w,
            browse_h,
            "Browse",
            false,
            false,
        );
        self.drag.push_region(
            browse_x,
            browse_y,
            browse_w,
            browse_h,
            HitAction::Button(ButtonAction::Browse),
        );

        // Mode selector (right-aligned)
        let mode_w = 160.0 * s;
        let mode_h = 22.0 * s;
        let mode_x = w - pad - mode_w;
        let mode_y = strip_y + (strip_h - mode_h) * 0.5;
        let active_idx = if self.params.mode.value() == crate::FilterMode::Raw {
            0
        } else {
            1
        };
        let segments = ["Raw", "Phaseless"];
        widgets::draw_stepped_selector(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            mode_x,
            mode_y,
            mode_w,
            mode_h,
            &segments,
            active_idx,
        );
        let seg_w = mode_w / segments.len() as f32;
        for i in 0..segments.len() as u8 {
            self.drag.push_region(
                mode_x + seg_w * i as f32,
                mode_y,
                seg_w,
                mode_h,
                HitAction::Button(ButtonAction::Mode(i)),
            );
        }

        // Path label between Browse and Mode selector
        let path_x = browse_x + browse_w + pad;
        let path_right = mode_x - pad;
        let path_w = (path_right - path_x).max(0.0);
        if path_w > 10.0 {
            let path_text = self
                .params
                .wavetable_path
                .lock()
                .map(|p| format_wavetable_label(&p))
                .unwrap_or_else(|_| "(locked)".to_string());
            let text_size = 13.0 * s;
            let text_y = strip_y + (strip_h + text_size) * 0.5 - 3.0 * s;
            self.text_renderer.draw_text(
                &mut self.surface.pixmap,
                path_x,
                text_y,
                &path_text,
                text_size,
                widgets::color_text(),
            );
        }

        // Bottom rule under the strip
        widgets::draw_rect(
            &mut self.surface.pixmap,
            0.0,
            strip_h - 1.0,
            w,
            1.0,
            widgets::color_border(),
        );
    }
```

- [ ] **Step 4: Verify compile**

Run: `cargo check -p wavetable-filter 2>&1 | tail -20`
Expected: clean. If `widgets::draw_stepped_selector` has a different signature (optional `editing_text` etc.), match it — check `tiny-skia-widgets/src/controls.rs`.

- [ ] **Step 5: Commit**

```bash
git add wavetable-filter/src/editor.rs
git commit -m "wavetable-filter: top strip with Browse, path label, mode selector"
```

---

## Task 6: Dial rows with modulation + text-edit

**Files:**
- Modify: `wavetable-filter/src/editor.rs`

Draws the five dials beneath the visualization area (which is still blank at this task). The view area itself is wired in Tasks 8–9, but the dial row layout can be drawn independently and stays in place once the views land.

- [ ] **Step 1: Add dial rendering to `draw()`**

After the top-strip code in `draw()`, continue the function with:

```rust
        let h = self.physical_height as f32;

        // ── Dial geometry ──
        let dial_row_h = 60.0 * s;
        let dial_radius = 22.0 * s;

        // Lower dial row: sits at the bottom of the window
        let dial_row_y = h - dial_row_h;

        // Frame dial takes the left half; Freq/Res/Drive/Mix share the right half
        let left_w = w * 0.5;
        let right_w = w - left_w;

        // Frame dial, centered in left half
        self.draw_dial(
            ParamId::Frame,
            "Frame",
            left_w * 0.5,
            dial_row_y + dial_row_h * 0.5,
            dial_radius,
        );

        // Right-side dials: 4 evenly spaced
        let right_dials: [(ParamId, &str); 4] = [
            (ParamId::Frequency, "Freq"),
            (ParamId::Resonance, "Res"),
            (ParamId::Drive, "Drive"),
            (ParamId::Mix, "Mix"),
        ];
        let spacing = right_w / right_dials.len() as f32;
        for (i, &(pid, label)) in right_dials.iter().enumerate() {
            let cx = left_w + spacing * (i as f32 + 0.5);
            let cy = dial_row_y + dial_row_h * 0.5;
            self.draw_dial(pid, label, cx, cy, dial_radius);
        }
    }

    fn draw_dial(&mut self, param_id: ParamId, label: &str, cx: f32, cy: f32, radius: f32) {
        use nih_plug::prelude::Param;
        let p = self.float_param(param_id);
        let unmod = p.unmodulated_normalized_value();
        let modulated = p.modulated_normalized_value();
        let value_text = self.format_value(param_id);

        let editing_buf: Option<String> = self
            .text_edit
            .active_for(&HitAction::Dial(param_id))
            .map(str::to_owned);
        let caret = self.text_edit.caret_visible();

        // Hit region is the bounding square around the dial plus label/value area.
        let hit_w = radius * 3.2;
        let hit_h = radius * 3.2;
        self.drag.push_region(
            cx - hit_w * 0.5,
            cy - hit_h * 0.5,
            hit_w,
            hit_h,
            HitAction::Dial(param_id),
        );

        widgets::draw_dial_ex(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            cx,
            cy,
            radius,
            label,
            &value_text,
            unmod,
            Some(modulated),
            editing_buf.as_deref(),
            caret,
        );
    }
```

Note: the closing `}` on `fn draw` moved inside the dial block — make sure the braces are balanced. The method signature for `draw_dial` is new.

- [ ] **Step 2: Verify compile**

Run: `cargo check -p wavetable-filter 2>&1 | tail -20`
Expected: clean. Tune the `self.params.frame_position` / `.frequency` / etc. field names if they differ from the code assumption.

- [ ] **Step 3: Commit**

```bash
git add wavetable-filter/src/editor.rs
git commit -m "wavetable-filter: dial row rendering with modulation + text-edit support"
```

---

## Task 7: Mouse / keyboard event handling

**Files:**
- Modify: `wavetable-filter/src/editor.rs`

Wires up dial drag, button clicks, right-click-to-type, and keyboard input. Same structure as warp-zone's `on_event`, with the browse + mode + 2D/3D-toggle button actions instead of Freeze.

- [ ] **Step 1: Replace `on_event` with the full handler**

Replace the stub `on_event` block with:

```rust
    fn on_event(
        &mut self,
        _window: &mut baseview::Window,
        event: baseview::Event,
    ) -> baseview::EventStatus {
        match &event {
            baseview::Event::Window(baseview::WindowEvent::Resized(info)) => {
                self.physical_width = info.physical_size().width.max(MIN_WIDTH);
                self.physical_height = info.physical_size().height.max(MIN_HEIGHT);
                let sf = (self.physical_width as f32 / WINDOW_WIDTH as f32).clamp(0.5, 4.0);
                self.scale_factor = sf;
                self.shared_scale.store(sf);
                self.resize_buffers();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorEntered) => {
                self.drag.on_cursor_entered();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorLeft) => {
                self.drag.on_cursor_left();
            }
            baseview::Event::Mouse(baseview::MouseEvent::CursorMoved {
                position,
                modifiers,
            }) => {
                self.drag.set_mouse(position.x as f32, position.y as f32);
                if let Some(HitAction::Dial(param_id)) = self.drag.active_action().copied() {
                    let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                    let current = self.float_param(param_id).unmodulated_normalized_value();
                    if let Some(norm) = self.drag.update_drag(shift, current) {
                        let setter = ParamSetter::new(self.gui_context.as_ref());
                        self.set_param_normalized(&setter, param_id, norm);
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Left,
                modifiers,
            }) => {
                self.commit_text_edit();

                if let Some(region) = self.drag.hit_test().cloned() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                        self.end_set_param(&setter, id);
                    }
                    let is_double = self.drag.check_double_click(&region.action);
                    match region.action {
                        HitAction::Dial(param_id) => {
                            if is_double {
                                self.reset_param_to_default(&setter, param_id);
                            } else {
                                let norm =
                                    self.float_param(param_id).unmodulated_normalized_value();
                                let shift = modifiers.contains(keyboard_types::Modifiers::SHIFT);
                                self.drag.begin_drag(HitAction::Dial(param_id), norm, shift);
                                self.begin_set_param(&setter, param_id);
                            }
                        }
                        HitAction::Button(ButtonAction::Browse) => {
                            self.open_file_dialog();
                        }
                        HitAction::Button(ButtonAction::WavetableToggle2D3D) => {
                            self.show_2d = !self.show_2d;
                        }
                        HitAction::Button(ButtonAction::Mode(variant)) => {
                            self.set_mode(variant);
                        }
                    }
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonReleased {
                button: baseview::MouseButton::Left,
                ..
            }) => {
                if let Some(HitAction::Dial(id)) = self.drag.end_drag() {
                    let setter = ParamSetter::new(self.gui_context.as_ref());
                    self.end_set_param(&setter, id);
                }
            }
            baseview::Event::Mouse(baseview::MouseEvent::ButtonPressed {
                button: baseview::MouseButton::Right,
                ..
            }) => {
                if self.drag.active_action().is_some() {
                    return baseview::EventStatus::Captured;
                }
                if let Some(region) = self.drag.hit_test().cloned() {
                    self.commit_text_edit();
                    if let HitAction::Dial(param_id) = region.action {
                        let initial = self.formatted_value_without_unit(param_id);
                        self.text_edit.begin(HitAction::Dial(param_id), &initial);
                    }
                }
            }
            baseview::Event::Keyboard(ev) if self.text_edit.is_active() => {
                if ev.state != keyboard_types::KeyState::Down {
                    return baseview::EventStatus::Captured;
                }
                match &ev.key {
                    keyboard_types::Key::Character(s) => {
                        for c in s.chars() {
                            self.text_edit.insert_char(c);
                        }
                    }
                    keyboard_types::Key::Backspace => self.text_edit.backspace(),
                    keyboard_types::Key::Escape => self.text_edit.cancel(),
                    keyboard_types::Key::Enter => {
                        self.commit_text_edit();
                    }
                    _ => return baseview::EventStatus::Ignored,
                }
                return baseview::EventStatus::Captured;
            }
            _ => {}
        }

        baseview::EventStatus::Captured
    }
```

- [ ] **Step 2: Verify compile**

Run: `cargo check -p wavetable-filter 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p wavetable-filter -- -D warnings 2>&1 | tail -15`
Expected: clean.

- [ ] **Step 4: Standalone smoke test**

Run: `cargo build --bin wavetable-filter && ./target/debug/wavetable-filter &`
Expected: Window opens. Browse button opens a file dialog. Mode selector segments are clickable and toggle the underlying `FilterMode` (no visible UI change yet — dial values just stop reacting correctly once `Phaseless` is set because no wavetable is loaded). Drag on a dial moves it (label won't re-render until the next frame, but the parameter should change — verify by opening Bitwig / the standalone's generic UI). Right-click on a dial opens an edit field with the current value. Kill the process.

- [ ] **Step 5: Commit**

```bash
git add wavetable-filter/src/editor.rs
git commit -m "wavetable-filter: mouse/keyboard handling, right-click-to-type"
```

---

## Task 8: Port the wavetable view (2D + 3D)

**Files:**
- Modify: `wavetable-filter/src/editor/wavetable_view.rs`
- Modify: `wavetable-filter/src/editor.rs`

Ports the vizia wavetable-view rendering to tiny-skia anti-aliased paths. Keeps the 2D/3D modes and the click-to-toggle hit region.

- [ ] **Step 1: Fill out `wavetable_view.rs`**

Replace `wavetable-filter/src/editor/wavetable_view.rs` with:

```rust
//! Wavetable visualization — 2D face-on or 3D overhead stack.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::wavetable::Wavetable;

pub(crate) struct FrameCache {
    pub cached_frames: Vec<Vec<f32>>,
    pub cached_version: u32,
    pub cached_frame_count: usize,
    pub cached_frame_size: usize,
    pub global_min: f32,
    pub global_max: f32,
}

impl FrameCache {
    pub fn new() -> Self {
        Self {
            cached_frames: Vec::new(),
            cached_version: u32::MAX,
            cached_frame_count: 0,
            cached_frame_size: 0,
            global_min: 0.0,
            global_max: 0.0,
        }
    }
}

/// Refresh the cached frames if the wavetable version has advanced.
/// Uses `try_lock` to avoid stalling the GUI thread on contention.
pub(crate) fn refresh_frame_cache(
    cache: &mut FrameCache,
    shared_wt: &Mutex<Wavetable>,
    version: &AtomicU32,
) {
    let current_version = version.load(Ordering::Relaxed);
    if current_version == cache.cached_version {
        return;
    }
    let Ok(wt) = shared_wt.try_lock() else {
        return;
    };
    cache.cached_frames = wt.frames.clone();
    cache.cached_frame_count = wt.frame_count;
    cache.cached_frame_size = wt.frame_size;
    cache.cached_version = current_version;

    let mut gmin = f32::INFINITY;
    let mut gmax = f32::NEG_INFINITY;
    for frame in &cache.cached_frames {
        for &sample in frame {
            gmin = gmin.min(sample);
            gmax = gmax.max(sample);
        }
    }
    cache.global_min = gmin;
    cache.global_max = gmax;
}

/// Draw the wavetable visualization into `pixmap` at the given bounds.
/// `current_frame_pos` is the normalized [0,1] frame index; `show_2d` selects face-on (true)
/// or 3D overhead stack (false).
pub(crate) fn draw_wavetable_view(
    pixmap: &mut Pixmap,
    cache: &FrameCache,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    current_frame_pos: f32,
    show_2d: bool,
) {
    // Background
    let mut bg = PathBuilder::new();
    bg.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).expect("valid rect"));
    if let Some(bg_path) = bg.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(20, 22, 28, 255);
        pixmap.fill_path(&bg_path, &paint, FillRule::Winding, Transform::identity(), None);

        let mut border = Paint::default();
        border.set_color_rgba8(60, 60, 70, 255);
        border.anti_alias = true;
        let stroke = Stroke {
            width: 1.0,
            ..Default::default()
        };
        pixmap.stroke_path(&bg_path, &border, &stroke, Transform::identity(), None);
    }

    let frame_count = cache.cached_frame_count;
    let frame_size = cache.cached_frame_size;
    if frame_count == 0 || frame_size == 0 {
        return;
    }

    let padding = 20.0;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let range = (cache.global_max - cache.global_min).max(0.001);
    let current_frame_idx = (current_frame_pos * (frame_count - 1) as f32).round() as usize;

    if show_2d {
        draw_2d_face_on(
            pixmap,
            &cache.cached_frames,
            current_frame_pos,
            x + padding,
            y + padding,
            width,
            height,
            frame_count,
            frame_size,
        );
        draw_zero_line(pixmap, x + padding, y + padding + height * 0.5, width);
        return;
    }

    draw_3d_overhead(
        pixmap,
        &cache.cached_frames,
        current_frame_idx,
        cache.global_min,
        range,
        x,
        y,
        w,
        h,
        padding,
        width,
        height,
        frame_count,
        frame_size,
    );

    // Zero line (grid)
    draw_zero_line(pixmap, x + padding, y + padding + height * 0.5, width);
}

#[allow(clippy::too_many_arguments)]
fn draw_2d_face_on(
    pixmap: &mut Pixmap,
    frames: &[Vec<f32>],
    current_frame_pos: f32,
    x0: f32,
    y0: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) {
    let exact_pos = current_frame_pos * (frame_count - 1) as f32;
    let lo = (exact_pos.floor() as usize).min(frame_count - 1);
    let hi = (lo + 1).min(frame_count - 1);
    let frac = exact_pos - lo as f32;

    let frame_lo = &frames[lo];
    let frame_hi = &frames[hi];

    let num_points = (width as usize).min(frame_size).max(1);

    let mut fmin = f32::INFINITY;
    let mut fmax = f32::NEG_INFINITY;
    for pi in 0..num_points {
        let si = ((pi as f32 / num_points as f32) * frame_size as f32) as usize;
        let si = si.min(frame_size - 1);
        let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
        fmin = fmin.min(s);
        fmax = fmax.max(s);
    }
    let frange = (fmax - fmin).max(0.001);
    let zero_y = y0 + height * 0.5;

    let mut fill_pb = PathBuilder::new();
    let mut stroke_pb = PathBuilder::new();
    fill_pb.move_to(x0, zero_y);

    for pi in 0..num_points {
        let t = pi as f32 / num_points as f32;
        let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
        let s = frame_lo[si] * (1.0 - frac) + frame_hi[si] * frac;
        let normalized = (s - fmin) / frange;
        let x = x0 + t * width;
        let y = y0 + height - normalized * height;

        fill_pb.line_to(x, y);
        if pi == 0 {
            stroke_pb.move_to(x, y);
        } else {
            stroke_pb.line_to(x, y);
        }
    }
    fill_pb.line_to(x0 + width, zero_y);
    fill_pb.close();

    if let Some(fill_path) = fill_pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(79, 195, 247, 30);
        paint.anti_alias = true;
        pixmap.fill_path(&fill_path, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(stroke_path) = stroke_pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(79, 195, 247, 220);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: 1.5,
            line_cap: LineCap::Round,
            ..Default::default()
        };
        pixmap.stroke_path(&stroke_path, &paint, &stroke, Transform::identity(), None);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_3d_overhead(
    pixmap: &mut Pixmap,
    frames: &[Vec<f32>],
    current_frame_idx: usize,
    global_min: f32,
    range: f32,
    bounds_x: f32,
    bounds_y: f32,
    _bounds_w: f32,
    bounds_h: f32,
    padding: f32,
    width: f32,
    height: f32,
    frame_count: usize,
    frame_size: usize,
) {
    // Non-active frames, back-to-front
    for frame_idx in (0..frame_count).rev() {
        if frame_idx == current_frame_idx {
            continue;
        }
        let frame = &frames[frame_idx];
        let depth = frame_idx as f32 / frame_count.max(1) as f32;
        let perspective_x = depth * 80.0;
        let perspective_y = -depth * 80.0;
        let alpha = 0.3 + (1.0 - depth) * 0.4;

        let draw_w = (width * 0.7) as usize;
        let pts = draw_w.min(frame_size).max(1);

        let mut pb = PathBuilder::new();
        for pi in 0..pts {
            let t = pi as f32 / pts as f32;
            let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
            let normalized = (frame[si] - global_min) / range;
            let x = bounds_x + padding + t * (width * 0.7) + perspective_x;
            let y =
                bounds_y + bounds_h - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
            if pi == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let r = (50.0 + (1.0 - depth) * 100.0) as u8;
            let g = (100.0 + (1.0 - depth) * 100.0) as u8;
            let a = (alpha * 255.0) as u8;
            let mut paint = Paint::default();
            paint.set_color_rgba8(r, g, 255, a);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 1.2,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Active frame on top
    if current_frame_idx < frame_count {
        let frame = &frames[current_frame_idx];
        let depth = current_frame_idx as f32 / frame_count.max(1) as f32;
        let perspective_x = depth * 80.0;
        let perspective_y = -depth * 80.0;

        let draw_w = (width * 0.7) as usize;
        let pts = draw_w.min(frame_size).max(1);
        let mut pb = PathBuilder::new();
        for pi in 0..pts {
            let t = pi as f32 / pts as f32;
            let si = ((t * frame_size as f32) as usize).min(frame_size - 1);
            let normalized = (frame[si] - global_min) / range;
            let x = bounds_x + padding + t * (width * 0.7) + perspective_x;
            let y =
                bounds_y + bounds_h - padding * 2.0 - (normalized * height * 0.4) + perspective_y;
            if pi == 0 {
                pb.move_to(x, y);
            } else {
                pb.line_to(x, y);
            }
        }
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 200, 100, 255);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 2.5,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        }
    }
}

fn draw_zero_line(pixmap: &mut Pixmap, x: f32, y: f32, w: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y);
    pb.line_to(x + w, y);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(80, 80, 90, 100);
        paint.anti_alias = true;
        let stroke = Stroke {
            width: 0.5,
            ..Default::default()
        };
        pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}
```

- [ ] **Step 2: Wire the wavetable view into `draw()`**

In `editor.rs`, in `fn draw`, after the top-strip bottom-rule code and before the dial code, add the visualization area and wavetable view block. The complete middle section should look like:

```rust
        // ── Visualization area ──
        let viz_top = strip_h;
        let viz_bot = h - dial_row_h;
        let viz_h = (viz_bot - viz_top).max(1.0);
        let viz_pad = 10.0 * s;

        let col_w = (w - viz_pad * 3.0) * 0.5;
        let left_x = viz_pad;
        let right_x = viz_pad * 2.0 + col_w;

        // Refresh wavetable frame cache (try_lock, non-blocking)
        wavetable_view::refresh_frame_cache(
            &mut self.frame_cache,
            &self.shared_wavetable,
            &self.wavetable_version,
        );

        use nih_plug::prelude::Param;
        let current_frame_pos = self.params.frame_position.modulated_normalized_value();

        wavetable_view::draw_wavetable_view(
            &mut self.surface.pixmap,
            &self.frame_cache,
            left_x,
            viz_top + viz_pad,
            col_w,
            viz_h - viz_pad * 2.0,
            current_frame_pos,
            self.show_2d,
        );

        // Click region for 2D/3D toggle
        self.drag.push_region(
            left_x,
            viz_top + viz_pad,
            col_w,
            viz_h - viz_pad * 2.0,
            HitAction::Button(ButtonAction::WavetableToggle2D3D),
        );

        // Filter response view placeholder (Task 9 fills this in)
        widgets::draw_rect_outline(
            &mut self.surface.pixmap,
            right_x,
            viz_top + viz_pad,
            col_w,
            viz_h - viz_pad * 2.0,
            widgets::color_border(),
            1.0,
        );
```

Make sure the dial code that follows is at the bottom (Frame dial centered in left half, etc., from Task 6). The `h` variable should already be declared near the top of `draw` — if it isn't, move its declaration above the visualization block.

**Hit-region ordering matters:** the 2D/3D toggle region covers the entire wavetable view, and the Frame dial's hit region overlaps the bottom portion. Dial regions are pushed later, so they win (hit tests walk from last-pushed to first). Confirm by adjusting region ordering if necessary.

- [ ] **Step 3: Compile + clippy**

Run: `cargo check -p wavetable-filter 2>&1 | tail -15`
Run: `cargo clippy -p wavetable-filter -- -D warnings 2>&1 | tail -15`
Expected: both clean.

- [ ] **Step 4: Standalone smoke test**

Run: `cargo build --bin wavetable-filter && ./target/debug/wavetable-filter &`
Expected: Browse → pick `wavetable-filter/tests/fixtures/phaseless-bass.wt`. The wavetable visualization appears in the left column (3D overhead). Click on it → toggles to 2D face-on showing the current interpolated frame. Dragging the Frame dial updates both modes. Kill the process.

- [ ] **Step 5: Commit**

```bash
git add wavetable-filter/src/editor.rs wavetable-filter/src/editor/wavetable_view.rs
git commit -m "wavetable-filter: port wavetable view (2D/3D) to tiny-skia"
```

---

## Task 9: Port the filter response view

**Files:**
- Modify: `wavetable-filter/src/editor/filter_response_view.rs`
- Modify: `wavetable-filter/src/editor.rs`

Ports the response curve, grid, input-spectrum shadow, cutoff marker, and axis labels.

- [ ] **Step 1: Fill out `filter_response_view.rs`**

Replace `wavetable-filter/src/editor/filter_response_view.rs` with:

```rust
//! Filter response curve + input spectrum shadow, using tiny-skia paths.

use realfft::RealFftPlanner;
use rustfft::num_complex::Complex;
use std::sync::Mutex;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};
use tiny_skia_widgets::TextRenderer;

use crate::wavetable::Wavetable;

const FREQ_MIN: f32 = 20.0;
const FREQ_MAX: f32 = 20000.0;
const DB_CEIL: f32 = 0.0;
const DB_FLOOR: f32 = -48.0;
const DB_RANGE: f32 = DB_CEIL - DB_FLOOR;

const PARAM_EPSILON: f32 = 0.001;

pub(crate) struct FftCache {
    planner: RealFftPlanner<f32>,
    frame_buf: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    cached_mags: Vec<f32>,
    cached_frame_pos: f32,
    cached_cutoff: f32,
    cached_resonance: f32,
    freq_table: Vec<f32>,
    freq_table_size: usize,
    cached_response_ys: Vec<f32>,
    cached_input_mags: Vec<f32>,
    cached_input_sr: f32,
}

impl FftCache {
    pub fn new() -> Self {
        Self {
            planner: RealFftPlanner::new(),
            frame_buf: Vec::new(),
            spectrum: Vec::new(),
            cached_mags: Vec::new(),
            cached_frame_pos: -1.0,
            cached_cutoff: -1.0,
            cached_resonance: -1.0,
            freq_table: Vec::new(),
            freq_table_size: 0,
            cached_response_ys: Vec::new(),
            cached_input_mags: Vec::new(),
            cached_input_sr: 0.0,
        }
    }
}

pub(crate) fn refresh_fft_cache(
    cache: &mut FftCache,
    frame_pos: f32,
    cutoff_hz: f32,
    resonance: f32,
    shared_wt: &Mutex<Wavetable>,
) {
    let needs_update = (cache.cached_frame_pos - frame_pos).abs() > 0.01
        || (cache.cached_cutoff - cutoff_hz).abs() > PARAM_EPSILON
        || (cache.cached_resonance - resonance).abs() > PARAM_EPSILON
        || cache.cached_mags.is_empty();
    if !needs_update {
        return;
    }

    let frame_n = match shared_wt.try_lock() {
        Ok(wt) => {
            if wt.frame_count == 0 || wt.frame_size == 0 {
                return;
            }
            let n = wt.frame_size;
            cache.frame_buf.resize(n, 0.0);
            wt.interpolate_frame_into(frame_pos, &mut cache.frame_buf);
            n
        }
        Err(_) => return,
    };

    let fft = cache.planner.plan_fft_forward(frame_n);
    cache.spectrum.resize(frame_n / 2 + 1, Complex::new(0.0, 0.0));
    for c in cache.spectrum.iter_mut() {
        *c = Complex::new(0.0, 0.0);
    }

    let FftCache {
        frame_buf,
        spectrum,
        cached_mags,
        ..
    } = cache;

    if fft.process(frame_buf, spectrum).is_err() {
        return;
    }
    cached_mags.clear();
    cached_mags.extend(spectrum.iter().map(|c| c.norm()));
    let peak = cached_mags
        .iter()
        .cloned()
        .fold(0.0f32, f32::max)
        .max(1e-10);
    for m in cached_mags.iter_mut() {
        *m /= peak;
    }

    cache.cached_frame_pos = frame_pos;
    cache.cached_cutoff = cutoff_hz;
    cache.cached_resonance = resonance;
    cache.cached_response_ys.clear();
}

pub(crate) fn refresh_input_spectrum(
    cache: &mut FftCache,
    shared_in: &Mutex<(f32, Vec<f32>)>,
) {
    let Ok(data) = shared_in.try_lock() else {
        return;
    };
    let (sr, ref mags) = *data;
    if sr <= 0.0 || mags.is_empty() {
        return;
    }
    cache.cached_input_mags.resize(mags.len(), 0.0);
    cache.cached_input_mags.copy_from_slice(mags);
    cache.cached_input_sr = sr;
}

fn ensure_freq_table(cache: &mut FftCache, num_points: usize) {
    if cache.freq_table_size == num_points {
        return;
    }
    cache.freq_table.resize(num_points + 1, 0.0);
    let log_min = FREQ_MIN.ln();
    let log_range = FREQ_MAX.ln() - log_min;
    for i in 0..=num_points {
        let x_norm = i as f32 / num_points as f32;
        cache.freq_table[i] = (log_min + x_norm * log_range).exp();
    }
    cache.freq_table_size = num_points;
    cache.cached_response_ys.clear();
}

fn ensure_response_ys(cache: &mut FftCache, cutoff_hz: f32, resonance: f32, height: f32, y0: f32) {
    let n = cache.freq_table_size;
    if cache.cached_response_ys.len() == n + 1 || cache.cached_mags.is_empty() {
        return;
    }
    let comb_exp = resonance * 8.0;
    let FftCache {
        cached_mags,
        freq_table,
        cached_response_ys,
        ..
    } = cache;

    let max_src = (cached_mags.len() - 1) as f32;
    cached_response_ys.resize(n + 1, 0.0);
    for i in 0..=n {
        let freq = freq_table[i];
        let src = freq * 24.0 / cutoff_hz;
        let mag = if src >= max_src {
            0.0
        } else if src <= 0.0 {
            cached_mags[0]
        } else {
            let lo = src.floor() as usize;
            let frac = src - lo as f32;
            let interp = cached_mags[lo] * (1.0 - frac) + cached_mags[lo + 1] * frac;
            if comb_exp > 0.01 {
                let dist = frac.min(1.0 - frac);
                let comb = (std::f32::consts::PI * dist).cos().powf(comb_exp);
                interp * comb
            } else {
                interp
            }
        };
        let db = 20.0 * mag.max(1e-6).log10();
        let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
        cached_response_ys[i] = y0 + height - y_norm * height;
    }
}

fn freq_to_x(freq_hz: f32) -> f32 {
    ((freq_hz.max(FREQ_MIN).ln() - FREQ_MIN.ln()) / (FREQ_MAX.ln() - FREQ_MIN.ln())).clamp(0.0, 1.0)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_filter_response(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cache: &mut FftCache,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cutoff_hz: f32,
    resonance: f32,
) {
    // Background
    let mut bg = PathBuilder::new();
    bg.push_rect(tiny_skia::Rect::from_xywh(x, y, w, h).expect("valid rect"));
    if let Some(bg_path) = bg.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(20, 22, 28, 255);
        pixmap.fill_path(&bg_path, &paint, FillRule::Winding, Transform::identity(), None);
        let mut border = Paint::default();
        border.set_color_rgba8(60, 60, 70, 255);
        border.anti_alias = true;
        pixmap.stroke_path(
            &bg_path,
            &border,
            &Stroke { width: 1.0, ..Default::default() },
            Transform::identity(),
            None,
        );
    }

    let padding = 20.0;
    let width = w - padding * 2.0;
    let height = h - padding * 2.0;
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let x0 = x + padding;
    let y0 = y + padding;

    let num_points = (width.max(1.0) as usize).min(256);
    ensure_freq_table(cache, num_points);
    ensure_response_ys(cache, cutoff_hz, resonance, height, y0);

    // Grid: horizontal dB lines
    for db in [-12.0_f32, -24.0, -36.0, -48.0] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (80, 80, 90, 100), 0.5);
    }
    // 0 dB reference, slightly brighter
    {
        let y_norm = (0.0_f32 - DB_FLOOR) / DB_RANGE;
        let gy = y0 + height - y_norm * height;
        stroke_line(pixmap, x0, gy, x0 + width, gy, (120, 120, 140, 180), 0.5);
    }
    // Vertical decade lines
    for freq in [100.0_f32, 1000.0, 10000.0] {
        let gx = x0 + freq_to_x(freq) * width;
        stroke_line(pixmap, gx, y0, gx, y0 + height, (80, 80, 90, 100), 0.5);
    }

    // Input spectrum shadow
    if cache.cached_input_sr > 0.0 && !cache.cached_input_mags.is_empty() {
        let bin_hz = cache.cached_input_sr / (2.0 * (cache.cached_input_mags.len() - 1) as f32);
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y0 + height);
        for i in 0..=num_points {
            let freq = cache.freq_table[i];
            let bin = freq / bin_hz;
            let mag = if bin >= (cache.cached_input_mags.len() - 1) as f32 {
                0.0
            } else if bin <= 0.0 {
                cache.cached_input_mags[0]
            } else {
                let lo = bin.floor() as usize;
                let frac = bin - lo as f32;
                cache.cached_input_mags[lo] * (1.0 - frac) + cache.cached_input_mags[lo + 1] * frac
            };
            let db = 20.0 * mag.max(1e-6).log10();
            let y_norm = ((db - DB_FLOOR) / DB_RANGE).clamp(0.0, 1.0);
            let xx = x0 + (i as f32 / num_points as f32) * width;
            pb.line_to(xx, y0 + height - y_norm * height);
        }
        pb.line_to(x0 + width, y0 + height);
        pb.close();
        if let Some(path) = pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(255, 200, 100, 25);
            paint.anti_alias = true;
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // Response curve
    if cache.cached_response_ys.len() == num_points + 1 {
        let mut fill_pb = PathBuilder::new();
        let mut stroke_pb = PathBuilder::new();
        for (i, &yy) in cache.cached_response_ys.iter().enumerate() {
            let xx = x0 + (i as f32 / num_points as f32) * width;
            if i == 0 {
                fill_pb.move_to(xx, y0 + height);
                fill_pb.line_to(xx, yy);
                stroke_pb.move_to(xx, yy);
            } else {
                fill_pb.line_to(xx, yy);
                stroke_pb.line_to(xx, yy);
            }
        }
        fill_pb.line_to(x0 + width, y0 + height);
        fill_pb.close();
        if let Some(fill_path) = fill_pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(100, 200, 255, 40);
            paint.anti_alias = true;
            pixmap.fill_path(&fill_path, &paint, FillRule::Winding, Transform::identity(), None);
        }
        if let Some(stroke_path) = stroke_pb.finish() {
            let mut paint = Paint::default();
            paint.set_color_rgba8(100, 200, 255, 255);
            paint.anti_alias = true;
            let stroke = Stroke {
                width: 2.0,
                line_cap: LineCap::Round,
                ..Default::default()
            };
            pixmap.stroke_path(&stroke_path, &paint, &stroke, Transform::identity(), None);
        }
    }

    // Cutoff marker
    let cutoff_x = x0 + freq_to_x(cutoff_hz) * width;
    stroke_line(pixmap, cutoff_x, y0, cutoff_x, y0 + height, (255, 100, 100, 200), 2.0);

    // Frequency labels — centered under each tick
    let text_size = 10.0;
    let labels_y = y + h - 5.0;
    for (freq, label) in [
        (50.0_f32, "50"),
        (200.0, "200"),
        (1000.0, "1k"),
        (5000.0, "5k"),
        (20000.0, "20k"),
    ] {
        let tw = text_renderer.text_width(label, text_size);
        let tx = x0 + freq_to_x(freq) * width - tw * 0.5;
        text_renderer.draw_text(pixmap, tx, labels_y, label, text_size, tiny_skia::Color::from_rgba8(150, 150, 150, 255));
    }

    // dB labels — right-aligned to x0
    for (db, label) in [(0.0_f32, "0"), (-24.0, "-24"), (-48.0, "-48")] {
        let y_norm = (db - DB_FLOOR) / DB_RANGE;
        let yy = y0 + height - y_norm * height;
        let tw = text_renderer.text_width(label, text_size);
        text_renderer.draw_text(
            pixmap,
            x0 - 3.0 - tw,
            yy + text_size * 0.4,
            label,
            text_size,
            tiny_skia::Color::from_rgba8(150, 150, 150, 255),
        );
    }
}

fn stroke_line(pixmap: &mut Pixmap, x0: f32, y0: f32, x1: f32, y1: f32, color: (u8, u8, u8, u8), width: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x0, y0);
    pb.line_to(x1, y1);
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color_rgba8(color.0, color.1, color.2, color.3);
        paint.anti_alias = true;
        pixmap.stroke_path(
            &path,
            &paint,
            &Stroke { width, ..Default::default() },
            Transform::identity(),
            None,
        );
    }
}
```

- [ ] **Step 2: Wire the response view into `draw()`**

In `editor.rs`, replace the "Filter response view placeholder" block (the `draw_rect_outline` call from Task 8 Step 2) with:

```rust
        let cutoff_hz = self.params.frequency.modulated_plain_value();
        let resonance = self.params.resonance.modulated_plain_value();

        filter_response_view::refresh_fft_cache(
            &mut self.fft_cache,
            current_frame_pos,
            cutoff_hz,
            resonance,
            &self.shared_wavetable,
        );
        filter_response_view::refresh_input_spectrum(
            &mut self.fft_cache,
            &self.shared_input_spectrum,
        );
        filter_response_view::draw_filter_response(
            &mut self.surface.pixmap,
            &mut self.text_renderer,
            &mut self.fft_cache,
            right_x,
            viz_top + viz_pad,
            col_w,
            viz_h - viz_pad * 2.0,
            cutoff_hz,
            resonance,
        );
```

- [ ] **Step 3: Compile + clippy**

Run: `cargo check -p wavetable-filter 2>&1 | tail -15`
Run: `cargo clippy -p wavetable-filter -- -D warnings 2>&1 | tail -15`
Expected: both clean.

- [ ] **Step 4: Standalone smoke test**

Run: `cargo build --bin wavetable-filter && ./target/debug/wavetable-filter &`

Expected: Load `tests/fixtures/phaseless-bass.wt` via Browse. The filter response curve draws in cyan with a translucent fill. Grid lines visible. Axis labels visible (50/200/1k/5k/20k along bottom; 0/-24/-48 on left). Dragging the Frequency dial moves the cutoff marker (red vertical line) and reshapes the curve. Dragging Resonance deepens the notch pattern. Feed audio into the standalone (or skip this step) and verify the amber input-spectrum shadow appears behind the curve. Kill the process.

- [ ] **Step 5: Commit**

```bash
git add wavetable-filter/src/editor.rs wavetable-filter/src/editor/filter_response_view.rs
git commit -m "wavetable-filter: port filter response view to tiny-skia"
```

---

## Task 10: Final verification + bundle

**Files:** none modified — verification only.

- [ ] **Step 1: Workspace clippy**

Run: `cargo clippy --workspace -- -D warnings 2>&1 | tail -20`
Expected: clean across all crates.

- [ ] **Step 2: Workspace tests**

Run: `cargo test --workspace 2>&1 | tail -15`
Expected: all 372 (pre-existing) tests pass. The wavetable-filter DSP tests should still be at 30.

- [ ] **Step 3: Release bundle**

Run: `cargo nih-plug bundle wavetable-filter --release 2>&1 | tail -5`
Expected: VST3 + CLAP bundles appear in `target/bundled/`.

- [ ] **Step 4: Manual DAW verification checklist**

Load `target/bundled/wavetable-filter.clap` (or `.vst3`) in Bitwig and verify:

- [ ] Editor opens at the persisted size (default 900×640).
- [ ] Browse loads a `.wt` file and the wavetable view redraws.
- [ ] Click on the wavetable view toggles 2D ⇄ 3D.
- [ ] Mode selector segments switch Raw ⇄ Phaseless.
- [ ] Dragging each dial updates the parameter; Shift-drag is finer; double-click resets to default.
- [ ] Right-click on each dial opens an edit field seeded with the current value (unit stripped). Enter commits, Escape cancels, click-outside auto-commits.
- [ ] Host modulation (e.g. Bitwig mod LFO on Frequency) draws an orange arc on the dial.
- [ ] Free resize from ~700×500 up to a very large size — layout stays intact.
- [ ] Input audio drives the amber spectrum shadow behind the response curve.
- [ ] `ui_scale` no longer appears in the host's parameter list.

- [ ] **Step 5: No-commit final gate**

If any checklist item fails, add follow-up tasks before considering the port complete. If all pass, the port is done. Do NOT commit or merge unless explicitly asked.

---

## Out of scope reminder

- No DSP changes.
- No new unit tests for editor rendering (matches existing softbuffer plugins in the pack).
- No changes to the nih-plug fork.
- No preset migration (no saved presets).

## Known-risk watchpoints

- **Missing `FilterMode` visibility.** `editor.rs` references `crate::FilterMode` (for mapping `Mode(u8)` → enum variant). If the current `FilterMode` enum is private, keep it private and expose `pub(crate)` or add a small `pub(crate) fn mode_is_raw(&self) -> bool` helper on params. Adjust Task 5 Step 2 accordingly.
- **`interpolate_frame_into` must exist.** `refresh_fft_cache` calls `wt.interpolate_frame_into(frame_pos, &mut cache.frame_buf)` — the method already exists in `src/wavetable.rs` and is used by the current vizia view.
- **`PendingReload` fields.** Task 5's `open_file_dialog` constructs a `PendingReload`. Match the field names to the struct definition in `lib.rs` — if fields drift, this is the first place an error surfaces.
- **`EditorState::from_size` vs `new`.** Check the current `tiny-skia-widgets/src/editor_base.rs` API — if it uses `EditorState::new(w, h)` instead of `from_size`, swap. Match what warp-zone uses.
- **`draw_stepped_selector` signature.** If the current signature takes an `editing_text: Option<&str>` trailing argument (following the dial/slider convention), pass `None`. The task shows the simplest form — adjust per actual signature.
