# Wavetable Filter — Port to tiny-skia

**Date:** 2026-04-19
**Branch:** `wavetable-filter-tiny-skia`
**Status:** design approved, implementation plan pending

## Goal

Port the Wavetable Filter plugin's editor from `nih_plug_vizia` (OpenGL) to the established softbuffer + tiny-skia + baseview + fontdue stack used by every other editor in the pack (gain-brain, satch, tinylimit, pope-scope, warp-zone). The DSP is unchanged; only the GUI layer is replaced.

## Motivation

- **Consistency.** All five other softbuffer plugins share `tiny-skia-widgets` (dials, sliders, buttons, text-edit state). The Wavetable Filter is the last vizia holdout and the only one carrying a GPU driver dependency.
- **Resource cost.** CPU rendering via softbuffer eliminates ~25 MB of Mesa/LLVM per instance.
- **Feature parity pull.** Standard softbuffer plugins now support right-click-to-type on continuous dials and host-modulation display on dial arcs. Porting brings Wavetable Filter in line.

## Decisions (from brainstorming)

1. **`ui_scale: IntParam` is removed entirely.** No user has saved presets yet; matches every other softbuffer plugin, none of which expose a scale param.
2. **Right-click-to-type** on all five continuous dials (Frame, Frequency, Resonance, Drive, Mix) via `tiny_skia_widgets::TextEditState<HitAction>`.
3. **2D/3D click-toggle** on the wavetable view is preserved. Click anywhere in the view's bounds toggles between a 2D face-on frame and a 3D overhead stack; default is 3D.
4. **Input-spectrum shadow** on the filter response view is preserved. Audio thread continues to publish `(sample_rate, Vec<f32>)` through `shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>`; GUI reads via `try_lock`.
5. **Window sizing.** Default 900×640, minimum ≈ 700×500, scale-factor clamp `[0.5, 4.0]`. `WINDOW_WIDTH = 900` is the 1.0× reference for `scale_factor = physical_width / WINDOW_WIDTH`.
6. **Mode selector** uses `tiny_skia_widgets::draw_stepped_selector` (Raw / Phaseless).
7. **Top strip is a single row:** `[Browse] [wavetable name]   …   [Mode: Raw | Phaseless]`. No title line, no scaling buttons.
8. **Modulation on dial arcs** via `Some(param.modulated_normalized_value())` passed into `draw_dial_ex`, same as the recent warp-zone change.
9. **Rendering approach:** tiny-skia `stroke_path` / `fill_path` with `anti_alias = true` for both viz areas — **not** the direct-pixel-write pipeline pope-scope uses for its waveform fast path. Waveform quality matters more than GUI-CPU floor here.

## Architecture

```
wavetable-filter/src/
├── lib.rs                       # DSP + params (ui_scale removed; editor creation swapped)
├── wavetable.rs                 # unchanged
├── editor.rs                    # NEW: module root, WavetableFilterWindow, layout
├── editor/
│   ├── wavetable_view.rs        # REWRITTEN: draw_wavetable_view + FrameCache
│   └── filter_response_view.rs  # REWRITTEN: draw_filter_response + FftCache
├── fonts/
│   └── DejaVuSans.ttf           # NEW: embedded for fontdue (copy from sibling plugin)
└── tests/                       # unchanged (DSP fixtures)
```

The audio-thread contract in `lib.rs` does not change: all GUI/audio interop already flows through `Arc<Mutex<…>>` / `Arc<AtomicU32>` handles that are agnostic to the GUI toolkit. The port is isolated to the editor module and a handful of dependency / deletion edits in `lib.rs`.

### `WavetableFilterWindow` (editor.rs)

```rust
struct WavetableFilterWindow {
    gui_context: Arc<dyn GuiContext>,
    params: Arc<WavetableFilterParams>,

    // GUI infrastructure (shared pattern across softbuffer plugins)
    surface: SurfaceState,
    text_renderer: TextRenderer,
    drag: DragState<HitAction>,
    text_edit: TextEditState<HitAction>,

    // Resize / scale plumbing
    physical_width: u32,
    physical_height: u32,
    scale_factor: f32,
    shared_scale: Arc<AtomicF32>,
    pending_resize: Arc<AtomicU64>,

    // Plugin-specific audio → GUI
    wavetable_path: Arc<Mutex<String>>,
    should_reload: Arc<AtomicBool>,
    pending_reload: Arc<Mutex<Option<PendingReload>>>,
    shared_wavetable: Arc<Mutex<Wavetable>>,
    wavetable_version: Arc<AtomicU32>,
    shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>,

    // View-local state
    show_2d: bool,
    frame_cache: FrameCache,
    fft_cache: FftCache,
}

enum ParamId { Frame, Frequency, Resonance, Drive, Mix }

enum ButtonAction {
    Browse,
    WavetableToggle2D3D,   // click anywhere in the wavetable view bounds
    Mode(u8),              // stepped selector segments
}

enum HitAction {
    Dial(ParamId),
    Button(ButtonAction),
}
```

### Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ [Browse]  wavetable-name.wt                Mode: [ Raw | Phaseless ] │   ~32 px strip
├──────────────────────────────────────────────────────────────────────┤
│ ┌──── Wavetable view ────────────┐  ┌──── Filter response ─────────┐ │
│ │ (click toggles 2D / 3D)        │  │ grid + input shadow + curve  │ │   viz area
│ │                                │  │ + cutoff marker + labels     │ │
│ └────────────────────────────────┘  └──────────────────────────────┘ │
│        [Frame dial]                   [Freq] [Res]                   │   dial rows
│                                       [Drive] [Mix]                  │
└──────────────────────────────────────────────────────────────────────┘
```

All sizes and dial radii scale with `scale_factor` (`physical_width / WINDOW_WIDTH`).

### `editor/wavetable_view.rs`

Cache struct owned by `WavetableFilterWindow`; draw logic is a pure-ish free function (takes `&FrameCache` + geometry + `show_2d` flag).

```rust
pub(crate) struct FrameCache {
    cached_frames: Vec<Vec<f32>>,
    cached_version: u32,
    cached_frame_count: usize,
    cached_frame_size: usize,
    global_min: f32,
    global_max: f32,
}

pub(crate) fn refresh_frame_cache(
    cache: &mut FrameCache,
    shared_wt: &Mutex<Wavetable>,
    version: &AtomicU32,
);

pub(crate) fn draw_wavetable_view(
    pixmap: &mut Pixmap,
    cache: &FrameCache,
    x: f32, y: f32, w: f32, h: f32,
    current_frame_pos: f32,
    show_2d: bool,
);
```

Rendering ports 1:1 from the current vizia implementation:
- **2D mode:** interpolated frame between `frame_lo` / `frame_hi` rendered as a translucent fill + anti-aliased stroke, plus a faint zero line.
- **3D mode:** all non-active frames drawn back-to-front with depth-based alpha/hue fade, then the active frame overlaid in bright orange with a thicker stroke.

Both modes use tiny-skia `Path` + `Paint { anti_alias: true, .. }`.

### `editor/filter_response_view.rs`

Same pattern — `FftCache` owned by the window, free function for draw.

```rust
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

pub(crate) fn refresh_fft_cache(cache: &mut FftCache, frame_pos: f32, cutoff_hz: f32, resonance: f32, shared_wt: &Mutex<Wavetable>);
pub(crate) fn refresh_input_spectrum(cache: &mut FftCache, shared_in: &Mutex<(f32, Vec<f32>)>);

pub(crate) fn draw_filter_response(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    cache: &FftCache,
    x: f32, y: f32, w: f32, h: f32,
    cutoff_hz: f32,
);
```

Caching strategy preserved from vizia version:
- Skip FFT rebuild unless `frame_pos` drifted by `> 0.01` OR cutoff/resonance changed by `> PARAM_EPSILON` OR `cached_mags` is empty.
- `freq_table` rebuilt only when display width changes.
- `cached_response_ys` invalidated on param change or width change.

Rendering:
- Background + border.
- Grid: horizontal dB lines (−12, −24, −36, −48) + brighter 0 dB reference + vertical decade lines (100, 1k, 10k).
- Input spectrum shadow: `Paint::color(rgba(255, 200, 100, 25))` filled polygon closed along the bottom edge.
- Response curve: `Paint::color(rgba(100, 200, 255, 40))` fill + `rgb(100, 200, 255)` 2 px stroke.
- Cutoff marker: `rgba(255, 100, 100, 200)` vertical 2 px line.
- Labels: `TextRenderer::draw_text` for frequency (50 / 200 / 1k / 5k / 20k along bottom, centered) and dB (0 / −24 / −48 on left axis, right-aligned by measured width).

## Dependencies

**Add to `wavetable-filter/Cargo.toml`:**
- `baseview`
- `softbuffer`
- `tiny-skia`
- `fontdue`
- `keyboard-types`
- `tiny-skia-widgets = { path = "../tiny-skia-widgets" }`
- `rfd` (file dialog — already present)

**Remove:**
- `nih_plug_vizia`
- `nih-plug-widgets` (the vizia-side `ParamDial`)

## Data flow

Unchanged from the current implementation:

- **`shared_wavetable: Arc<Mutex<Wavetable>>`** — audio thread installs new wavetable from `pending_reload`; GUI reads via `try_lock()` when `wavetable_version` advances.
- **`wavetable_version: Arc<AtomicU32>`** — bumped by GUI on successful load; both view caches gate their refresh on this value.
- **`shared_input_spectrum: Arc<Mutex<(f32, Vec<f32>)>>`** — audio thread publishes sample_rate + magnitudes each hop; GUI snapshots via `try_lock()` per frame.
- **`pending_reload: Arc<Mutex<Option<PendingReload>>>`** — GUI-thread wavetable load pre-allocates FFT plan + scratch buffers so the audio thread's `install` path stays allocation-free.
- **`should_reload: Arc<AtomicBool>`** — flag the audio thread polls to pick up `pending_reload`.

## Error handling

- **File dialog cancelled.** No-op.
- **Wavetable parse error.** `nih_log!("Error: {e}")`. The existing `status_message: String` field is never rendered in the current layout — it is dropped, not ported.
- **Lock contention** on any shared Arc<Mutex<…>> → render from stale cache.
- **Zero-frame wavetable** → skip drawing the affected view.
- **Host-initiated resize** handled via the standard `pending_resize: Arc<AtomicU64>` pattern (high 32 bits width, low 32 bits height) consumed on the next frame.

## Testing

- No new editor-level tests — matches the rest of the pack (warp-zone, satch, tinylimit have zero editor tests; pope-scope tests only non-rendering helpers).
- All 30 existing DSP tests in `wavetable-filter/src/lib.rs` and `src/wavetable.rs` must keep passing.
- Widget-level rendering is already covered by the 29 tests in `tiny-skia-widgets/`.
- **Manual verification checklist:**
  1. Bundle builds cleanly (`cargo nih-plug bundle wavetable-filter --release`).
  2. `cargo clippy --workspace -- -D warnings` is clean.
  3. Standalone opens at 900×640; Browse button loads `tests/fixtures/phaseless-bass.wt`; waveform + frequency response both render.
  4. Click on wavetable view toggles 2D ⇄ 3D.
  5. Right-click any dial opens the edit field with unit stripped; Enter commits via `string_to_normalized_value`; Escape cancels; click-outside auto-commits.
  6. Free resize from ~700×500 up to 3200×2400; layout stays intelligible.
  7. Host modulation (Bitwig) shows an orange modulation arc on dial arcs (delta > 0.001).
  8. Input audio drives the amber input-spectrum shadow behind the response curve.

## Out of scope

- DSP changes of any kind.
- Fork / upstream changes to `nih-plug`.
- New features beyond parity with the current editor minus the cut widgets.
- Preset-format migration (no presets saved yet).
