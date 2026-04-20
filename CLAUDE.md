# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tract Plugin Pack is a Cargo workspace containing multiple audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

### Plugins

**Wavetable Filter** — uses wavetable frames as FIR filter kernels. Two modes: Raw (direct convolution, zero latency) and Phaseless (STFT magnitude-only filtering, no pre-ringing). GUI uses softbuffer + tiny-skia (CPU rendering). 3D wavetable background is cached into a viewport-sized pixmap and blitted with a raw row-wise `copy_from_slice` (not `draw_pixmap`) so per-frame cost stays flat as the wavetable grows. Convolution has a silence fast-path: when the filter state is all-zero and no non-trivial samples have been pushed since the last reset, the 2048-tap SIMD MAC loop is skipped entirely.

**GS Meter** — lightweight loudness meter with gain utility for clip-to-zero workflows. dB mode: peak, true peak (ITU-R BS.1770-4), RMS integrated/momentary, crest factor. LUFS mode: EBU R128 integrated/short-term/momentary loudness, LRA, true peak. Per-mode gain and reference with gain-match buttons. GUI uses softbuffer + tiny-skia (CPU rendering, no GPU). Designed for 100+ instances per project.

**Gain Brain** — lightweight gain utility with cross-instance group linking via in-process static atomics. 16 groups, Absolute/Relative link modes, Invert toggle for mirrored gain movement. GUI uses softbuffer + tiny-skia (CPU rendering). ~0.62 MB RSS per instance. Inspired by BlueCat's Gain Suite.

**tinylimit** — low-latency wideband peak limiter for track-level use. Feed-forward with lookahead, dual-stage transient/dynamics envelope, soft knee (Giannoulis 2012), optional ISP (ITU-1770 true peak). 7 built-in character presets. GUI uses softbuffer + tiny-skia (CPU rendering). 50 instances @ 6.2% CPU, 50 MB RSS (~1.0 MB, 0.12% CPU per instance). Inspired by DMG Audio TrackLimit.

**satch** — detail-preserving spectral saturator. FFT-based spectral analysis preserves quiet frequency components through clipping. Independent gain, threshold, knee, detail, and mix controls. CPU rendering via softbuffer + tiny-skia.

**Pope Scope** — multichannel real-time oscilloscope with beat sync. Static global store shares audio across up to 16 instances. Three display modes (Vertical/Overlay/Sum), three draw styles (Line/Filled/Both), beat-aligned grid, dB-scaled amplitude mapping. Ring buffer with hierarchical mipmap (L0 raw, L1 per-64, L2 per-256). CPU rendering via softbuffer + tiny-skia. Cursor tooltip on hover shows time (or bar position in beat-sync) and per-track dB readings; in Vertical mode the tooltip and cursor line restrict to the hovered lane. Waveform drawing bypasses tiny-skia's raster pipeline — direct pixel-write column fills with half-split envelope smoothing cut GUI CPU cost by ~52% vs the original path-based rasterizer.

**Warp Zone** — psychedelic spectral shifter/stretcher using a phase vocoder. Shift (-24 to +24 semitones) moves pitch, Stretch (0.5x to 2.0x) warps harmonic spacing for inharmonic textures. Freeze captures the current FFT frame as a sustained drone (transport-aware). Feedback feeds output back into input for compounding spectral effects. Low/High frequency range limits for selective processing. Scrolling spectral waterfall display with psychedelic color palette. 4096-point FFT, 1024 hop, ~85ms latency. CPU rendering via softbuffer + tiny-skia.

## Workspace Structure

```
tract-plugin-pack/
├── wavetable-filter/       # Wavetable-based filter plugin (softbuffer GUI)
├── gs-meter/               # Loudness meter + gain utility (softbuffer GUI)
├── gain-brain/             # Gain utility with group linking (softbuffer GUI)
├── tinylimit/              # Wideband peak limiter (softbuffer GUI)
├── satch/                  # Spectral saturator with detail preservation (softbuffer GUI)
├── pope-scope/             # Multichannel oscilloscope with beat sync (softbuffer GUI)
├── warp-zone/              # Spectral shifter/stretcher with waterfall display (softbuffer GUI)
├── nih-plug-widgets/       # Legacy vizia widgets (unused since wavetable-filter's tiny-skia port; kept for reference)
├── tiny-skia-widgets/      # Shared CPU-rendered widgets (dial, slider, button)
├── docs/                   # Plugin manuals (markdown + PDF)
└── xtask/                  # Build tooling
```

## Build Commands

Requires **nightly Rust** (enforced via `rust-toolchain.toml`) for portable SIMD (`std::simd::f32x16`).

```bash
# Plugin bundles (VST3 + CLAP)
cargo nih-plug bundle wavetable-filter --release
cargo nih-plug bundle gs-meter --release
cargo nih-plug bundle gain-brain --release
cargo nih-plug bundle tinylimit --release
cargo nih-plug bundle satch --release
cargo nih-plug bundle pope-scope --release
cargo nih-plug bundle warp-zone --release

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release
cargo build --bin gain-brain --release
cargo build --bin tinylimit --release
cargo build --bin satch --release
cargo build --bin pope-scope --release
cargo build --bin warp-zone --release

# Debug standalone (for GUI testing without DAW)
cargo build --bin gs-meter
cargo build --bin gain-brain
cargo build --bin tinylimit
cargo build --bin satch
cargo build --bin pope-scope
cargo build --bin warp-zone
```

## Testing & Linting

```bash
cargo test --workspace                            # all tests (407)
cargo clippy --workspace -- -D warnings           # lint (CI uses -D warnings)
cargo fmt --check
```

Tests are inline `#[cfg(test)]` modules:
- `wavetable-filter/src/lib.rs` and `wavetable-filter/src/wavetable.rs` — 30 DSP tests
- `gs-meter/src/meter.rs` — 62 meter tests (RMS, peak, true peak, SIMD, stereo)
- `gain-brain/src/groups.rs` and `gain-brain/src/lib.rs` — 42 group sync tests
- `tinylimit/src/limiter.rs` — 33 limiter tests (gain computer, envelope, lookahead, integration)
- `satch/src/lib.rs` and `satch/src/spectral.rs` — 46 spectral saturator tests
- `pope-scope/` — 113 tests (ring buffer, snapshot, time mapping, renderer, store, theme, cursor tooltip, peak_at_column parity)
- `warp-zone/src/spectral.rs` and `warp-zone/src/lib.rs` — 17 tests (phase vocoder, bin remapping, shift/stretch accuracy, spectral display, band downsampling)
- `tiny-skia-widgets/` — 29 widget rendering tests (dial, slider, button, text, drag state mouse-in-window)
- Test fixtures: `wavetable-filter/tests/fixtures/`

## Development Practices

- **Prefer TDD**: Write tests before or alongside implementation. New DSP functions and data structures should have tests covering normal operation, edge cases, and error paths.
- **Never commit unless asked**: Do not create git commits unless the user explicitly requests it. This is a hard rule with zero exceptions.
- **No allocations on the audio thread**: `process()` must never allocate. Use pre-allocated buffers, `try_lock()` for shared data, and avoid `Vec::new()`, `clone()` of collections, or `String` operations in the hot path.
- **No unsafe code**: Do not use `unsafe` blocks. Find safe alternatives or restructure the code to avoid needing unsafe. Exceptions: FFI windowing glue (raw-window-handle trait impls, `Send` for window handles) where the underlying API requires it.
- **Don't guess at fixes**: Write tests to verify, add debug logging to diagnose, dispatch agents to review. Never claim a fix works without evidence.
- **Use the LSP tool**: Prefer the LSP tool over grep for code navigation. Fall back to grep only when LSP is unavailable.

## Architecture

### Wavetable Filter

| File | Role |
|------|------|
| `wavetable-filter/src/lib.rs` | Plugin DSP: convolution, STFT, kernel synthesis, parameter smoothing. `FilterState::is_silent` silence fast-path skips the per-sample SIMD MAC loop when history is all zero |
| `wavetable-filter/src/wavetable.rs` | Wavetable I/O (`.wav`/`.wt`), frame interpolation |
| `wavetable-filter/src/editor.rs` | Softbuffer + baseview editor: top strip (Browse + wavetable name + Raw/Phaseless stepped selector), two visualization columns, five dials (Frame / Frequency / Resonance / Gain / Mix) with modulation arcs and right-click text entry, free resize |
| `wavetable-filter/src/editor/wavetable_view.rs` | 2D face-on + 3D overhead wavetable visualization. 3D strands are strided (max 48) and the full bg is cached into a viewport-sized pixmap + blitted per frame via a raw row-wise `copy_from_slice` (bypasses tiny-skia's raster pipeline) |
| `wavetable-filter/src/editor/filter_response_view.rs` | Frequency response + input spectrum shadow. Response-curve Y coords cached with height/y0 keys to invalidate on vertical resize; shadow draw skipped when no bin exceeds the -48 dB floor (avoids tiny-skia's zero-height-polygon warning) |
| `wavetable-filter/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### GS Meter

| File | Role |
|------|------|
| `gs-meter/src/lib.rs` | Plugin integration, process() loop, parameter definitions |
| `gs-meter/src/meter.rs` | Core metering DSP: RMS, peak, true peak (ITU BS.1770-4), crest factor, SIMD |
| `gs-meter/src/editor.rs` | Softbuffer + baseview editor, hit testing, mouse interaction, free resize |
| `gs-meter/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### Gain Brain

| File | Role |
|------|------|
| `gain-brain/src/lib.rs` | Plugin struct, params, process(), group sync logic (cumulative delta + absolute mode) |
| `gain-brain/src/groups.rs` | In-process static atomics: 16 group slots with cumulative_delta, absolute_gain, epoch, generation |
| `gain-brain/src/editor.rs` | Softbuffer + baseview editor with rotary dial |
| `gain-brain/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### tinylimit

| File | Role |
|------|------|
| `tinylimit/src/lib.rs` | Plugin struct, params, process(), metering atomics |
| `tinylimit/src/limiter.rs` | Core DSP: gain computer, dual-stage envelope, lookahead backward pass |
| `tinylimit/src/true_peak.rs` | ITU polyphase FIR true peak detector (copied from gs-meter) |
| `tinylimit/src/editor.rs` | Softbuffer + baseview editor with meters, dials, presets |
| `tinylimit/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### satch

| File | Role |
|------|------|
| `satch/src/lib.rs` | Plugin struct, params, process(), clip detection |
| `satch/src/spectral.rs` | FFT spectral analysis, per-bin magnitude saturation, detail preservation |
| `satch/src/editor.rs` | Softbuffer + baseview editor with dials and meters |

### Pope Scope

| File | Role |
|------|------|
| `pope-scope/src/lib.rs` | Plugin struct, params, process() with ring buffer push, time mapping, pending_push buffer |
| `pope-scope/src/ring_buffer.rs` | RingBuffer with atomic write_pos, two-pass push (memcpy + SIMD f32x16 mipmap), 3-level hierarchy |
| `pope-scope/src/store.rs` | Static global store: 16 slots with CAS ownership, RwLock<Option<Vec<RingBuffer>>> |
| `pope-scope/src/snapshot.rs` | Snapshot building (free + beat sync), bar latch, hold buffer, stale detection. `WaveSnapshot::peak_at_column` aligns cursor-tooltip readouts with the renderer's decimated column envelopes; `sample_at_normalized_x` for raw index lookup |
| `pope-scope/src/time_mapping.rs` | PPQ/sample/rb_pos atomic mapping, beat-aligned window, discontinuity detection |
| `pope-scope/src/renderer.rs` | Per-column opaque waveform fills with half-split envelope smoothing (direct pixel writes, bypasses tiny-skia raster pipeline), amplitude grid, beat grid with quarter-beat subdivisions, cursor tooltip rendering (time/bar label + per-track color-coded dB readings) |
| `pope-scope/src/editor.rs` | Softbuffer + baseview editor, 60 FPS, hit regions, control strips, peak hold, cursor tooltip dispatch, CachedViewParams for freeze-aware grid/labels, vertical-lane cursor restriction |
| `pope-scope/src/controls.rs` | Track control strip: solo/mute/color buttons, name truncation |
| `pope-scope/src/theme.rs` | Amber phosphor color palette, 16-color channel palette, hue shifting, `blend_u32` pre-mix helper for opaque-path fills |

### Warp Zone

| File | Role |
|------|------|
| `warp-zone/src/lib.rs` | Plugin struct, params, process(), SpectralDisplay shared buffer, band downsampling |
| `warp-zone/src/spectral.rs` | Phase vocoder: STFT analysis, bin remapping with linear interpolation, phase accumulation, freeze, frequency range |
| `warp-zone/src/editor.rs` | Softbuffer + baseview editor with dials, freeze button, spectral waterfall display |

### Shared

| File | Role |
|------|------|
| `tiny-skia-widgets/src/primitives.rs` | Color palette (incl. `color_edit_bg` for right-click text-entry highlight), `draw_rect` (auto-fast-path on opaque colors via `BlendMode::Source`), `draw_rect_outline`, `draw_rect_opaque` (explicit Source blend), `fill_pixmap_opaque` (slice-fill BG clear), `fill_column_opaque` (1px-wide direct pixel-write strip used by pope-scope waveform fast path) |
| `tiny-skia-widgets/src/text.rs` | TextRenderer with fontdue glyph cache |
| `tiny-skia-widgets/src/controls.rs` | draw_button, draw_slider, draw_stepped_selector + outline variants. Sliders render a unit-stripped edit field with 1px caret when their `editing_text: Option<&str>` argument is `Some` |
| `tiny-skia-widgets/src/param_dial.rs` | Arc-based rotary dial widget (draw_dial; draw_dial_ex renders the edit field + caret when `editing_text: Option<&str>` is `Some`) |
| `tiny-skia-widgets/src/editor_base.rs` | Shared EditorState (size persistence), SurfaceState (pixmap + softbuffer) |
| `tiny-skia-widgets/src/drag.rs` | DragState with hit regions, drag/shift-granular handling, `mouse_in_window()` tracking via CursorEntered/CursorLeft events |
| `tiny-skia-widgets/src/text_edit.rs` | TextEditState<A> — right-click-to-type state machine shared by every softbuffer editor. Filtered numeric buffer (`0-9 . - + e E`), 16-char cap, 1000 ms caret blink |
| `nih-plug-widgets/*` | Legacy vizia ParamDial + CSS theme. Unreferenced by any plugin since wavetable-filter's softbuffer port; retained for possible future reuse |

### Key Design Decisions

- **GS Meter uses CPU rendering** (softbuffer + tiny-skia + fontdue) instead of vizia/OpenGL. This eliminates 25 MB of GPU driver overhead (Mesa/LLVM) per instance. At 300 instances (Bitwig, 48kHz/1024): 15% CPU, 560 MB RSS (~1.8 MB per instance).
- **All softbuffer plugins are freely resizable.** Scale factor is derived from `physical_width / WINDOW_WIDTH` on resize. Window size is persisted via `EditorState`. Host-initiated resize uses a packed `AtomicU64` (`pending_resize`) consumed on the next frame.
- **Right-click text entry on continuous dials/sliders** is shared across gain-brain, satch, tinylimit, pope-scope, warp-zone, and wavetable-filter via `tiny_skia_widgets::TextEditState<A>`. Right-click opens a highlighted edit field seeded with `Param::normalized_value_to_string(_, false)` (unit stripped). `Enter` commits through `Param::string_to_normalized_value` + `begin/set/end_set_parameter`; `Escape` cancels; click-outside or drag-start auto-commits; right-click during a drag is ignored. Key-up events are swallowed while editing so host DAW shortcuts don't fire on release. Stepped selectors, buttons, and toggles remain non-editable — right-click on them is a no-op.
- **True peak uses exact ITU-R BS.1770-4 coefficients** (48-tap, 4-phase polyphase FIR). Double-buffered history for contiguous SIMD dot products. Sample-rate-aware: 4x OS at <96kHz, 2x at 96-192kHz, bypass at >=192kHz.
- **Stereo RMS uses sum-of-power** (matches dpMeter5 SUM mode): `sqrt(ms_L + ms_R)`.
- **Crest factor uses dpMeter5's convention** (peak_stereo vs rms_stereo), not the mathematically correct max(crest_L, crest_R). Documented for future "correct mode" toggle.
- **RMS momentary uses O(1) running sum** (f64 precision, incremental add/subtract) instead of O(N) ring scan per buffer.
- **Gain Brain uses in-process static atomics** for cross-instance group linking. 16 group slots with cumulative_delta (fetch_add), absolute_gain, epoch, generation counters. Lock-free, zero overhead.
- **Gain Brain inversion** is applied on both reads and writes. The slot stores the writer's coordinate-space value. Invert toggles trigger a local rebaseline (re-read cumulative without applying delta) rather than bumping a shared epoch. Relative readers track last_seen_cumulative for self-echo suppression.
- **tinylimit uses feed-forward lookahead** with a backward-pass gain reduction ramp (DanielRudrich approach). Signal flow: gain computer → lookahead backward pass → dual-stage envelope → apply to delayed audio → safety clip. Hard knee fast path skips log/exp for sub-threshold samples. `exp()` instead of `powf()` for gain application (2x faster). Threshold/ceiling lerped per block (2 `exp` calls) instead of per-sample `powf`.
- **Warp Zone uses a phase vocoder** for spectral shifting/stretching. 4096-point FFT, 1024 hop (75% overlap, Hann window). Bin remapping uses linear interpolation between adjacent target bins with max-magnitude-wins collision resolution. Phase accumulation formula: `expected_target_increment + source_phase_deviation` (deviation NOT scaled by frequency ratio). Identity short-circuit (shift=0, stretch=1.0) copies bins directly without phase accumulation.
- **Warp Zone freeze** stops writing to the input ring buffer; the STFT keeps re-analyzing the frozen content. Transport-aware: output is silenced when DAW transport is stopped and freeze is active.
- **Warp Zone feedback** feeds clamped (±4.0) wet output back into the input on the next sample. Compounds spectral shifts for Shepard tone effects.
- **Warp Zone spectral display** uses lock-free AtomicU32 storage (128 bins × 256 columns). Audio thread writes f32 magnitudes as bit patterns; GUI reads them. Magnitudes normalized by 2/fft_size before storage.
- **Warp Zone frequency range** (Low/High params) controls which bins are remapped. Bins outside the range pass through with their original phase, maintaining phase tracking consistency.
- **Pope Scope cursor tooltip** ports the original JUCE oscilloscope's hover popup. On mouseover in the waveform area, a vertical cursor line is drawn at the mouse x and a tooltip shows the time (Free mode, via `format_time_ms`) or bar position (BeatSync mode, `"Bar X.XX"`) at the cursor, plus one row per visible track with a color swatch and dB reading. Readings are computed via `WaveSnapshot::peak_at_column(col, num_cols, mix_to_mono)`, which uses integer `div_ceil` arithmetic that exactly mirrors `decimate_to_columns`' floor-based mapping so the tooltip dB always matches the rendered envelope peak at that column. In the sparse-samples path (`samples.len() <= num_cols`) the lookup linearly interpolates between adjacent samples to match the renderer's line-segment output. In Vertical display mode the tooltip and cursor line are restricted to the hovered lane (computed from `mouse_y` and `track_h`). A `DragState::mouse_in_window` flag tracks baseview `CursorEntered`/`CursorLeft` events so the tooltip doesn't latch a phantom hover at the initial (0,0) position or stick at the last in-window coordinate after the cursor leaves the window.
- **Pope Scope freeze consistency** captures `sync_mode`, `timebase_ms`, and `sync_unit_bars` into `CachedViewParams` every frame the snapshot is rebuilt. When `freeze` is on, the grid rendering and tooltip time label both read from the cached params instead of live parameter values, so toggling sync mode or editing timebase while frozen can't desync the visible grid from the frozen waveform. All three display-mode grid call sites (Vertical/Overlay/Sum) thread the same `eff_sync_mode` / `eff_timebase_ms` / `eff_sync_bars` tuple.
- **Pope Scope waveform rendering is a direct pixel-write pipeline**. The original path-based renderer (`stroke_path` / `fill_path`) spent ~42% of GUI CPU in tiny-skia's anti-aliased raster pipeline (`walk_edges`, `blit_anti_h`, `source_over_rgba_tail`, `lerp_u8`) whenever audio was playing. The current implementation calls `tiny_skia_widgets::fill_column_opaque` once per pixel column in the dense-samples branch. That helper writes directly into `Pixmap::pixels_mut()` via `chunks_exact_mut(width)` + indexed assignment, with the color pre-flattened to `PremultipliedColorU8` once per draw. No tiny-skia `Paint`, no `RasterPipelineBlitter`, no per-pixel blend. The filled variant pre-mixes its requested opacity with `theme::BG` via `theme::blend_u32`, so the rect is drawn fully opaque and `fill_column_opaque` can bypass source-over blending entirely. `tiny_skia_widgets::draw_rect` auto-detects opaque colors and switches to `BlendMode::Source` internally, giving existing opaque callers (cursor line, tooltip background, control bar rects, borders) the same fast path without code changes. Profile trajectory on a 2-track / cursor-motion / audio-playing scenario in Bitwig (10s @ 999 Hz): 8653 samples (paths) → 5585 (opaque rects) → 4367 (direct pixel write) → 4154 (half-split envelope) — a 52% reduction in total GUI CPU.
- **Pope Scope half-split envelope smoothing**: each column's effective top/bot is `min(own_top, (prev_top + own_top)/2, (own_top + next_top)/2)` (and the symmetric `max` for bot), instead of the simpler full-bridge approach. This is equivalent to rasterizing the envelope polyline such that each segment between adjacent columns contributes half its vertical span to each endpoint column — dy=5 steps become two dy=2.5 steps. Cheaper than the full bridge (fewer pixels extended per column), smoother contour, and symmetric (preserves isolated peaks and valleys that the earlier asymmetric bridge-to-prev would flatten).
- **nih-plug dependency** currently points to `xxx/nih-plug` branch `finish-vst3-pr`. Fork adds: Editor::set_size() for host-initiated resize, Plugin::update_track_info() + TrackInfo struct for CLAP track-info, BYPASS_BUFFER_COPY const, nightly SIMD compatibility, VST3 license fix.

### Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous chunks of equal size (256/512/1024/2048 samples)
- `.wt` — Surge-compatible wavetable format; frame metadata in header

Sample wavetable: `wavetable-filter/tests/fixtures/phaseless-bass.wt`
