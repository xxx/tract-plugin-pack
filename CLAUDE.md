# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tract Plugin Pack is a Cargo workspace containing multiple audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

### Plugins

**Wavetable Filter** — uses wavetable frames as FIR filter kernels. Two modes: Raw (direct convolution, zero latency) and Phaseless (STFT magnitude-only filtering, no pre-ringing). GUI uses softbuffer + tiny-skia (CPU rendering). 3D wavetable background is cached into a viewport-sized pixmap and blitted with a raw row-wise `copy_from_slice` (not `draw_pixmap`) so per-frame cost stays flat as the wavetable grows. Convolution has a silence fast-path: when the filter state is all-zero and no non-trivial samples have been pushed since the last reset, the 2048-tap SIMD MAC loop is skipped entirely.

**GS Meter** — lightweight loudness meter with gain utility for clip-to-zero workflows. dB mode: peak, true peak (ITU-R BS.1770-4), RMS integrated/momentary, crest factor. LUFS mode: EBU R128 integrated/short-term/momentary loudness, LRA, true peak. Per-mode gain and reference with gain-match buttons. GUI uses softbuffer + tiny-skia (CPU rendering, no GPU). Designed for 100+ instances per project.

**Gain Brain** — lightweight gain utility with cross-instance group linking via in-process static atomics. 16 groups, Absolute/Relative link modes, Invert toggle for mirrored gain movement. GUI uses softbuffer + tiny-skia (CPU rendering). ~0.62 MB RSS per instance. Inspired by BlueCat's Gain Suite.

**Tinylimit** — low-latency wideband peak limiter for track-level use. Feed-forward with lookahead, dual-stage transient/dynamics envelope, soft knee (Giannoulis 2012), optional ISP (ITU-1770 true peak). 7 built-in character presets. GUI uses softbuffer + tiny-skia (CPU rendering). 50 instances @ 6.2% CPU, 50 MB RSS (~1.0 MB, 0.12% CPU per instance). Inspired by DMG Audio TrackLimit.

**Satch** — detail-preserving spectral saturator. FFT-based spectral analysis preserves quiet frequency components through clipping. Independent gain, threshold, knee, detail, and mix controls. CPU rendering via softbuffer + tiny-skia.

**Six Pack** — six-band parallel "distort the difference" multiband saturator. 1 low-shelf + 4 peaks + 1 high-shelf, six saturation algorithms (Tube, Tape, Diode, Digital, Class B, Wavefold). Per-band Stereo/Mid/Side routing applied to the diff *before* saturation (linear SVF on L/R per channel; route diff to M/S; saturate; recombine to L/R). Linear-phase polyphase oversampling (Off / 4× / 8× / 16×). Global de-emphasis subtracts the linear EQ boost so only saturation harmonics remain audible. Spectrum analyzer overlay (audio-thread FFT, 1024-sample throttle, atomic bins). softbuffer + tiny-skia CPU rendering.

**Pope Scope** — multichannel real-time oscilloscope with beat sync. Static global store shares audio across up to 16 instances. Three display modes (Vertical/Overlay/Sum), three draw styles (Line/Filled/Both), beat-aligned grid, dB-scaled amplitude mapping. Ring buffer with hierarchical mipmap (L0 raw, L1 per-64, L2 per-256). CPU rendering via softbuffer + tiny-skia. Cursor tooltip on hover shows time (or bar position in beat-sync) and per-track dB readings; in Vertical mode the tooltip and cursor line restrict to the hovered lane. Waveform drawing bypasses tiny-skia's raster pipeline — direct pixel-write column fills with half-split envelope smoothing cut GUI CPU cost by ~52% vs the original path-based rasterizer.

**Warp Zone** — psychedelic spectral shifter/stretcher using a phase vocoder. Shift (-24 to +24 semitones) moves pitch, Stretch (0.5x to 2.0x) warps harmonic spacing for inharmonic textures. Freeze captures the current FFT frame as a sustained drone (transport-aware). Feedback feeds output back into input for compounding spectral effects. Low/High frequency range limits for selective processing. Scrolling spectral waterfall display with psychedelic color palette. 4096-point FFT, 1024 hop, ~85ms latency. CPU rendering via softbuffer + tiny-skia.

**Imagine** — multiband stereo imager modeled on iZotope Ozone Imager. 4 fixed bands with Lipshitz/Vanderkooy compensated Linkwitz-Riley IIR or linear-phase FIR crossovers (switchable Quality). Per-band Ozone-style Width law (S_gain = (width+100)/100, M unchanged: 0=mono, 1=unity, 2=double-side at +100), two Stereoize modes (Mode I = Haas mid-into-side delay, Mode II = Schroeder/Gerzon all-pass decorrelator). Global Recover Sides folds a Hilbert-rotated residue of removed-side energy back into mid for perceptual width retention when narrowing. Spectrum + coherence display via single complex M+jS FFT. Half-disc polar (default) / 45°-rotated goniometer / Lissajous vectorscope on left, spectrum + 4-band strip + coherence on right. Pink/cyan duo-tone palette.

## Workspace Structure

```
tract-plugin-pack/
├── wavetable-filter/       # Wavetable-based filter plugin (softbuffer GUI)
├── gs-meter/               # Loudness meter + gain utility (softbuffer GUI)
├── gain-brain/             # Gain utility with group linking (softbuffer GUI)
├── tinylimit/              # Wideband peak limiter (softbuffer GUI)
├── satch/                  # Spectral saturator with detail preservation (softbuffer GUI)
├── six-pack/               # Six-band parallel multiband saturator (softbuffer GUI)
├── pope-scope/             # Multichannel oscilloscope with beat sync (softbuffer GUI)
├── warp-zone/              # Spectral shifter/stretcher with waterfall display (softbuffer GUI)
├── imagine/                # Multiband stereo imager modeled on Ozone Imager (softbuffer GUI)
├── nih-plug-widgets/       # Legacy vizia widgets (kept for reference; workspace-excluded so its old transitive deps don't enter the lock file)
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
cargo nih-plug bundle six-pack --release
cargo nih-plug bundle pope-scope --release
cargo nih-plug bundle warp-zone --release
cargo nih-plug bundle imagine --release

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release
cargo build --bin gain-brain --release
cargo build --bin tinylimit --release
cargo build --bin satch --release
cargo build --bin six-pack --release
cargo build --bin pope-scope --release
cargo build --bin warp-zone --release
cargo build --bin imagine --release

# Debug standalone (for GUI testing without DAW)
cargo build --bin gs-meter
cargo build --bin gain-brain
cargo build --bin tinylimit
cargo build --bin satch
cargo build --bin six-pack
cargo build --bin pope-scope
cargo build --bin warp-zone
cargo build --bin imagine
```

## Testing & Linting

```bash
cargo nextest run --workspace                     # all tests (554) -- parallel test runner
cargo clippy --workspace -- -D warnings           # lint (CI uses -D warnings)
cargo fmt --check
```

Install the runner with `cargo install cargo-nextest --locked` if missing. Config lives in `.config/nextest.toml`; CI uses the `ci` profile (retries=1, full failure enumeration). No doctests in the workspace, so `cargo test --doc` isn't needed.

Tests are inline `#[cfg(test)]` modules:
- `wavetable-filter/src/lib.rs` and `wavetable-filter/src/wavetable.rs` — 30 DSP tests
- `gs-meter/src/meter.rs` — 62 meter tests (RMS, peak, true peak, SIMD, stereo)
- `gain-brain/src/groups.rs` and `gain-brain/src/lib.rs` — 42 group sync tests
- `tinylimit/src/limiter.rs` — 33 limiter tests (gain computer, envelope, lookahead, integration)
- `satch/src/lib.rs` and `satch/src/spectral.rs` — 46 spectral saturator tests
- `pope-scope/` — 113 tests (ring buffer, snapshot, time mapping, renderer, store, theme, cursor tooltip, peak_at_column parity)
- `six-pack/` — 54 tests across `svf.rs`, `saturation.rs`, `bands.rs`, `spectrum.rs`, `oversampling.rs`, `lib.rs::plugin_tests` (bypass equivalence, harmonic structure, mix curve, de-emph cancellation, sample-rate sweep), and `editor/{curve_view,band_labels}.rs` (log-freq mapping, peak magnitude, label rendering)
- `warp-zone/src/spectral.rs` and `warp-zone/src/lib.rs` — 17 tests (phase vocoder, bin remapping, shift/stretch accuracy, spectral display, band downsampling)
- `imagine/` — 80 tests across `midside.rs`, `crossover.rs` (IIR + FIR crossfade, redesign-during-crossfade, band sum flatness), `hilbert.rs`, `decorrelator.rs`, `bands.rs` (constant-power width, stereoize, S_removed gating), `spectrum.rs`, `vectorscope.rs` (SPSC concurrent writer/reader), `theme.rs`, and `lib.rs::plugin_tests` (no-op default passes signal, recover-sides bypass)
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

### Tinylimit

| File | Role |
|------|------|
| `tinylimit/src/lib.rs` | Plugin struct, params, process(), metering atomics |
| `tinylimit/src/limiter.rs` | Core DSP: gain computer, dual-stage envelope, lookahead backward pass |
| `tinylimit/src/true_peak.rs` | ITU polyphase FIR true peak detector (copied from gs-meter) |
| `tinylimit/src/editor.rs` | Softbuffer + baseview editor with meters, dials, presets |
| `tinylimit/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### Satch

| File | Role |
|------|------|
| `satch/src/lib.rs` | Plugin struct, params, process(), clip detection |
| `satch/src/spectral.rs` | FFT spectral analysis, per-bin magnitude saturation, detail preservation |
| `satch/src/editor.rs` | Softbuffer + baseview editor with dials and meters |

### Six Pack

| File | Role |
|------|------|
| `six-pack/src/lib.rs` | Plugin struct, params, process() with OS/spectrum integration |
| `six-pack/src/svf.rs` | TPT SVF in mix-form; analytic unity at 0 dB (load-bearing for diff-trick) |
| `six-pack/src/saturation.rs` | Six waveshaper functions + Algorithm enum + dispatch |
| `six-pack/src/bands.rs` | Per-band SVF pair (L/R) + M/S routing applied to diff before saturation |
| `six-pack/src/oversampling.rs` | Cascaded half-band linear-phase polyphase 4×/8×/16× |
| `six-pack/src/spectrum.rs` | 2048-pt FFT analyzer (1024 hop, throttled, atomic 128 log-spaced bins) |
| `six-pack/src/editor.rs` | softbuffer + baseview editor lifecycle, hit testing, drag, resize |
| `six-pack/src/editor/curve_view.rs` | Composite EQ curve + spectrum overlay + 6 draggable dots |
| `six-pack/src/editor/band_labels.rs` | Per-band 5-row label grid; right-click text entry |
| `six-pack/src/editor/bottom_strip.rs` | Input/Output/Mix dials + Quality/Drive/De-Emphasis selectors |

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

### Imagine

| File | Role |
|------|------|
| `imagine/src/lib.rs` | Plugin orchestration: 22 params, lifecycle, process loop. Encode → crossover → bands → Recover → decode. Dry-delay aligns M/S sums with HilbertFir's group delay on the recover path. Quality is non-automatable. |
| `imagine/src/midside.rs` | M/S encode/decode (scalar + f32x16 SIMD). |
| `imagine/src/crossover.rs` | 4-band split: `CrossoverIir` (Linkwitz-Riley + Lipshitz/Vanderkooy compensation, magnitude-flat sum within ±0.05 dB), `CrossoverFir` (windowed-sinc with double-buffered taps + sample-wise crossfade on coefficient swap). FIR redesign is gated on >0.5 Hz frequency change. |
| `imagine/src/hilbert.rs` | 90° phase rotator. FIR-only (Type-IV anti-symmetric, length 65 = ~32 samples latency). Used by Recover Sides. |
| `imagine/src/decorrelator.rs` | Schroeder/Gerzon 6-stage all-pass cascade with prime-spaced delays. Used by Stereoize Mode II — genuinely lowers cross-correlation (xcorr<0.3 on broadband noise) vs the original Hilbert-90 design which left correlation near +0.8. |
| `imagine/src/bands.rs` | Per-band processor: Ozone-style Width gain (M unchanged, S scaled 0..2 across width [-100..+100]), Stereoize Mode I (Haas) / Mode II (decorrelator), gated S_removed accumulator (zero for width≥0). |
| `imagine/src/spectrum.rs` | Complex M+jS FFT trick yields \|M\| and \|S\| spectra in one transform. Magnitude-squared coherence γ²(k) computed audio-side via smoothed auto/cross spectra; published as 1−γ² per log-spaced bin. |
| `imagine/src/vectorscope.rs` | SPSC ring buffer of (L, R) samples. Per-sample `AtomicU32` storage (no `unsafe`), Acquire/Release ordering on write_pos. 32k-pair capacity. |
| `imagine/src/theme.rs` | Pink/cyan duo-tone palette (function accessors). |
| `imagine/src/editor.rs` | Softbuffer + baseview lifecycle; mouse/keyboard wiring; layout B coordination (vectorscope on left, spectrum + band strip + coherence on right, global strip at bottom). |
| `imagine/src/editor/spectrum_view.rs` | Crossover spectrum + 3 draggable splits + coherence bar. Log-frequency mapping helpers. |
| `imagine/src/editor/vectorscope_view.rs` | Polar (L vs R, 45°-rotated) + Lissajous render with mode toggle. Direct-pixel writes. |
| `imagine/src/editor/band_strip.rs` | Per-band: vertical Width slider + Stereoize knob + Mode I/II toggle + Solo. |
| `imagine/src/editor/global_strip.rs` | Recover Sides bar + Link Bands toggle + Quality 2-segment selector. |

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
| `nih-plug-widgets/*` | Legacy vizia ParamDial + CSS theme. Unreferenced by any plugin since wavetable-filter's softbuffer port; retained on disk for possible future reuse but excluded from the workspace build (the root `Cargo.toml`'s `exclude = [..., "nih-plug-widgets"]`) so its xcb 0.9.0 dependency stays out of the lock file |

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
- **Six Pack distorts the difference, not the signal.** Each band runs the dry input through a unity-when-flat SVF and computes `diff = svf_out − dry`. The diff *is* the EQ boost (and only the EQ boost) — at 0 dB the SVF is analytically unity so the diff is exactly zero and that band contributes silence. Saturation runs on the diff, so the band-gain knob effectively becomes the per-band drive. This requires the SVF to be exactly unity at 0 dB on every sample (no asymptotic approach, no transient bleed), which is why the SVF implementation is load-bearing for the entire architecture.
- **Six Pack M/S routing happens before saturation.** The SVF runs on L/R (one pair per channel) so the filter response is identical regardless of routing. The diff is then routed: Stereo passes through, Mid uses `(L+R)/2` on both legs, Side uses `(L−R)/2` and `(R−L)/2`. The router output goes into the per-band saturator, and the result is recombined back to L/R additively before the next band. This keeps the EQ shape independent of routing while letting saturation harmonics live exclusively in the chosen channel space.
- **Six Pack uses TPT SVF in `dry + (peak_gain − 1) · k · bandpass` mix form** for the four peak bands. The `k = 1/Q` factor cancels the `1/Q`-scaled peak height of the bandpass branch so the resulting peak magnitude at the center frequency is `peak_gain` independent of Q. Low-shelf and high-shelf use analogous mix forms with the lowpass / highpass branches. All three are analytically unity at 0 dB (the gain coefficient is exactly zero) — verified by `svf::tests::{peak,low_shelf,high_shelf}_unity_at_0db` and the `bands::tests::zero_db_band_produces_zero_diff` integration test.
- **Six Pack de-emphasis cancels the linear EQ boost from the wet path.** The wet signal is `dry + Σ saturate(diff_i)`. The de-emphasis stage subtracts `Σ diff_i` (the linear EQ sum) so the wet output collapses to `dry + Σ (saturate(diff_i) − diff_i)` — i.e. only saturation harmonics remain audible. At the trivial-saturation limit (drive → 0, where `saturate(x) → x`) and mix ≤ 50% the output equals dry exactly, which is the "Spectre-style" property exercised by `plugin_tests::deemph_cancellation_per_channel_mode`.
- **Imagine's Width law is the Ozone-style "scale Side, leave Mid alone"** (`S_gain = (width+100)/100`, range 0..2; `M_gain = 1` always). Width=0 is unity, -100 is mono (S muted), +100 doubles the side. The earlier constant-power law (M²+S²=2 invariant) was theoretically clean but practically wrong: it removed the mid at +100, which dramatically reduced volume on most music (whose energy is mid-dominated). Trade-off: at +100 with strongly stereo content the output may exceed 0 dBFS — users handle this with downstream gain, standard mastering practice.
- **Imagine's Stereoize Mode II is a real Schroeder/Gerzon decorrelator**, not a phase rotator. The original spec used Hilbert-90 but xcorr stayed near +0.8 — adding a phase-shifted copy of mid into side doesn't actually decorrelate. The 6-stage all-pass cascade with mutually-prime delays {41, 53, 67, 79, 97, 113} (sample-rate-scaled) drops xcorr below 0.3 on broadband noise.
- **Imagine's Recover Sides is gated per-band by Width sign**. Only bands with width<0 contribute to S_removed_total. The Hilbert FIR rotates the aggregate residue; recover_amount mixes it into mid. Recover Sides is a *perceptual* control (phase-decorrelated residue retains spatial impression when narrowing) — the audio path itself doesn't try to preserve total energy, since the new Width law doesn't either.
- **Imagine's IIR crossover uses Lipshitz/Vanderkooy delay-matched cascade** so the 4-band sum is true allpass-equivalent (magnitude-flat to ±0.05 dB across 20 Hz–20 kHz). Each band passes through compensating allpasses for splits it didn't traverse, so `Σ bands = AP3 ∘ AP2 ∘ AP1 (input)`.
- **Imagine's FIR crossover uses double-buffered tap arrays + sample-wise crossfade** on coefficient swap to eliminate clicks during crossover-frequency automation. The redesign is gated on >0.5 Hz change so static-parameter workloads don't pay for a continuous crossfade.
- **Imagine's Hilbert is FIR-only** (length 65, ~32 samples latency at 48 kHz). The plan originally specified an IIR all-pass cascade for zero-latency Recover Sides, but a single all-pass cascade can't produce 90° at low frequencies and the Niemitalo analytic-pair design produces an `(real, imag)` pair where `imag` is rotated relative to `real`, not relative to the input. The FIR is mathematically exact and ~0.7 ms latency is below human perception threshold.
- **Imagine's vectorscope ring buffer uses per-sample `AtomicU32` storage** (f32 stored as bit-pattern) to avoid `unsafe` while remaining lock-free SPSC. Acquire/Release ordering on `write_pos` synchronizes slot writes with the GUI consumer's reads. Per-sample (L, R) pair can tear under contention but the vectorscope decimates thousands of points/frame so a single torn pair is sub-pixel. Same general pattern as Warp Zone's spectral display and Pope Scope's ring buffer atomics, applied here to per-sample (L, R) pairs.
- **nih-plug dependency** currently points to `xxx/nih-plug` branch `finish-vst3-pr`. Fork adds: Editor::set_size() for host-initiated resize, Plugin::update_track_info() + TrackInfo struct for CLAP track-info, BYPASS_BUFFER_COPY const, nightly SIMD compatibility, VST3 license fix.

### Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous chunks of equal size (256/512/1024/2048 samples)
- `.wt` — Surge-compatible wavetable format; frame metadata in header

Sample wavetable: `wavetable-filter/tests/fixtures/phaseless-bass.wt`
