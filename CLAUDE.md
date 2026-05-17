# CLAUDE.md

Guidance for Claude Code working in this repository.

## Project Overview

Tract Plugin Pack — a Cargo workspace of audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust. Every plugin's GUI is CPU-rendered (softbuffer + tiny-skia + fontdue, no GPU) and freely resizable.

### Plugins

- **wavetable-filter** — wavetable frames as FIR kernels. Raw (direct convolution, zero latency) / Phaseless (STFT magnitude-only, no pre-ringing) modes.
- **miff** — convolution filter whose FIR kernel is hand-drawn with an MSEG editor (sibling of wavetable-filter, but no wavetable file). Raw / Phaseless modes.
- **gs-meter** — loudness meter + gain utility. dB mode (peak, true peak, RMS, crest) and LUFS mode (EBU R128, LRA). Designed for 100+ instances.
- **gain-brain** — gain utility with cross-instance group linking (16 groups, Absolute/Relative, Invert). Inspired by BlueCat Gain Suite.
- **tinylimit** — low-latency wideband peak limiter. Feed-forward + lookahead, dual-stage envelope, soft knee, optional ISP, 7 presets. Inspired by DMG TrackLimit.
- **satch** — detail-preserving spectral saturator. FFT per-bin magnitude saturation preserves quiet components through clipping.
- **six-pack** — six-band parallel "distort the difference" multiband saturator. 6 algorithms, per-band M/S routing, linear-phase oversampling, de-emphasis.
- **pope-scope** — multichannel oscilloscope with beat sync. Shared global store across 16 instances, 3 display modes, hierarchical mipmap ring buffer.
- **warp-zone** — spectral shifter/stretcher (phase vocoder). Shift, Stretch, Freeze, Feedback, frequency range. 4096-pt FFT, 1024 hop, ~85 ms latency.
- **imagine** — multiband stereo imager modeled on iZotope Ozone Imager. 4 bands, Ozone-style Width law, per-band Stereoize, Recover Sides, 4 vectorscope modes.

## Workspace Structure

Each plugin is a crate (`<plugin>/`). Plus: `tiny-skia-widgets/` (shared CPU-rendered widgets), `tract-dsp/` (shared GUI-free DSP primitives), `docs/` (manuals md+PDF), `xtask/` (build tooling), `nih-plug-widgets/` (legacy vizia widgets, workspace-excluded so its old transitive deps stay out of the lock file).

## Build / Test / Lint

Requires **nightly Rust** (enforced by `rust-toolchain.toml`) for portable SIMD (`std::simd::f32x16`).

```bash
cargo nih-plug bundle <plugin> --release   # VST3 + CLAP bundle
cargo build --bin <plugin> --release       # standalone
cargo build --bin <plugin>                 # debug standalone (GUI testing without DAW)

cargo nextest run --workspace              # all tests (parallel runner)
cargo clippy --workspace -- -D warnings    # lint (CI uses -D warnings)
cargo fmt --check
```

For local release/profile/bundle builds, prefix with `cargo xtask native` (auto-detects host CPU → `-C target-cpu=haswell`). Install the test runner via `cargo install cargo-nextest --locked`; config in `.config/nextest.toml`, CI uses the `ci` profile (retries=1). No doctests. Tests are inline `#[cfg(test)]` modules. Fixtures in `wavetable-filter/tests/fixtures/`.

## Development Practices

- **Prefer TDD** — tests before/alongside implementation, covering normal/edge/error paths.
- **Never commit unless asked** — hard rule, zero exceptions.
- **No allocations on the audio thread** — `process()` uses pre-allocated buffers and `try_lock()`; no `Vec::new()`, collection `clone()`, or `String` ops in the hot path.
- **No unsafe code** — except FFI windowing glue (raw-window-handle impls, `Send` for window handles) where the API requires it.
- **Don't guess at fixes** — verify with tests, diagnose with logging, dispatch agents to review. No claiming a fix works without evidence.
- **Use the LSP tool** over grep for code navigation; fall back to grep only when LSP is unavailable.

## Architecture

Common shape per plugin: `lib.rs` (plugin struct, params, `process()`), `editor.rs` (softbuffer + baseview editor), plus DSP modules. Notable per-plugin files:

**wavetable-filter** — `wavetable.rs` (.wav/.wt I/O, frame interpolation); `editor/wavetable_view.rs` (2D + 3D viz, strided strands, cached bg pixmap blitted via raw `copy_from_slice`); `editor/filter_response_view.rs` (response curve + input spectrum shadow). `FilterState::is_silent` skips the SIMD MAC loop when history is all-zero.

**miff** — `kernel.rs` (curve→FIR bake: single-walk `bake_taps`, peak-magnitude normalization, `KernelHandoff` Mutex+try_lock GUI→audio); `convolution.rs` (`RawChannel` SIMD MAC + silence fast-path, `PhaselessChannel` fixed 4096-pt STFT); `editor/response_view.rs`.

**gs-meter** — `meter.rs` (RMS, peak, true peak ITU BS.1770-4, crest, SIMD).

**gain-brain** — `groups.rs` (16-slot static atomics: cumulative_delta, absolute_gain, epoch, generation).

**tinylimit** — `limiter.rs` (gain computer, dual-stage envelope, lookahead backward pass); `true_peak.rs` (ITU polyphase FIR, copied from gs-meter).

**satch** — `spectral.rs` (FFT per-bin magnitude saturation, detail preservation).

**six-pack** — `svf.rs` (TPT SVF mix-form, analytic unity at 0 dB); `saturation.rs` (6 waveshapers); `bands.rs` (per-band SVF pair + M/S routing); `oversampling.rs` (polyphase 4/8/16×); `spectrum.rs`; `editor/{curve_view,band_labels,bottom_strip}.rs`.

**pope-scope** — `ring_buffer.rs` (atomic write_pos, two-pass push + SIMD mipmap, 3-level hierarchy); `store.rs` (16-slot static global, CAS ownership); `snapshot.rs` (free + beat sync, `peak_at_column`); `time_mapping.rs`; `renderer.rs` (direct pixel-write column fills, grids, cursor tooltip); `controls.rs`; `theme.rs` (amber phosphor, `blend_u32`).

**warp-zone** — `spectral.rs` (phase vocoder: STFT, bin remapping, phase accumulation, freeze, frequency range).

**imagine** — `midside.rs` (M/S encode/decode); `crossover.rs` (`CrossoverIir` Linkwitz-Riley + Lipshitz/Vanderkooy comp, `CrossoverFir` double-buffered + crossfade); `hilbert.rs` (FIR 90° rotator, len 65); `decorrelator.rs` (Schroeder/Gerzon 6-stage all-pass); `bands.rs` (Ozone Width, Stereoize Mode I/II, S_removed accumulator); `spectrum.rs` (M+jS FFT trick, coherence); `vectorscope.rs` (SPSC AtomicU32 ring); `polar_rays.rs` (SPSC emit ring); `theme.rs` (Cassiopeia A gold/teal); `editor/{spectrum_view,vectorscope_view,band_strip,global_strip}.rs`.

**tiny-skia-widgets** (shared) — `primitives.rs` (color palette, `draw_rect` opaque fast-path, `fill_pixmap_opaque`, `fill_column_opaque`); `text.rs` (fontdue glyph cache); `controls.rs` (button/slider/stepped-selector); `param_dial.rs` (rotary dial); `editor_base.rs` (EditorState size persistence, SurfaceState); `drag.rs` (DragState hit regions, `mouse_in_window`); `text_edit.rs` (`TextEditState<A>` right-click-to-type machine).

**tract-dsp** (shared, GUI-free DSP) — `true_peak.rs` (ITU-R BS.1770-4 detector, used by gs-meter/tinylimit); `spsc.rs` (lock-free SPSC ring); `db.rs`; `window.rs` (`hann_periodic` D=N / `hann_symmetric` D=N−1); `boxcar.rs` (`RunningSumWindow`, f64 accumulator); `fir.rs` (`FirRing` — double-buffered SIMD MAC, used by miff/wavetable-filter); `stft.rs` (`StftConvolver` — magnitude-multiply overlap-add; `stft` feature); `stft_analysis.rs` (`StftAnalyzer` — STFT analysis front-end: input ring + periodic-Hann + COLA window + forward FFT; `stft-analysis` feature; used by satch/warp-zone). Zero external deps by default; the FFT modules are feature-gated. `examples/tract_dsp_profile.rs` is the profiling harness.

## Key Design Decisions

**Rendering / GUI**
- CPU rendering (softbuffer + tiny-skia + fontdue) instead of vizia/OpenGL — saves ~25 MB GPU driver overhead per instance, enabling 100s of instances per project.
- All editors freely resizable: scale = `physical_width / WINDOW_WIDTH`; size persisted via `EditorState`; host resize via packed `AtomicU64` (`pending_resize`) consumed next frame.
- Right-click text entry on continuous dials/sliders (shared `TextEditState<A>`): edit field seeded with unit-stripped value; Enter commits via `string_to_normalized_value`, Escape cancels, click-outside/drag-start auto-commits, right-click-during-drag ignored, key-ups swallowed while editing. Stepped selectors/buttons/toggles are non-editable.
- Direct-pixel-write fast paths bypass tiny-skia's AA raster pipeline: `fill_column_opaque` (pope-scope waveform), cached-pixmap `copy_from_slice` blit (wavetable-filter 3D bg). `draw_rect` auto-switches to `BlendMode::Source` on opaque colors.

**Metering (gs-meter / tinylimit)**
- True peak: exact ITU-R BS.1770-4 coefficients (48-tap 4-phase polyphase FIR), double-buffered history. Sample-rate-aware: 4× OS <96 kHz, 2× 96–192 kHz, bypass ≥192 kHz.
- Stereo RMS = sum-of-power `sqrt(ms_L + ms_R)` (dpMeter5 SUM). Crest = peak_stereo vs rms_stereo (dpMeter5 convention, not `max(crest_L, crest_R)`).
- RMS momentary uses O(1) running sum (f64, incremental) not O(N) ring scan.

**gain-brain**
- Cross-instance group linking via in-process static atomics (16 slots, lock-free `fetch_add` cumulative_delta + absolute_gain + epoch + generation).
- Inversion applied on reads and writes; slot stores writer's coordinate-space value; Invert toggle triggers local rebaseline (not shared-epoch bump); Relative readers track `last_seen_cumulative` for self-echo suppression.

**tinylimit**
- Feed-forward lookahead with backward-pass GR ramp (DanielRudrich). Flow: gain computer → lookahead backward pass → dual-stage envelope → apply to delayed audio → safety clip. Hard-knee fast path skips log/exp sub-threshold; `exp()` not `powf()`; threshold/ceiling lerped per block.

**warp-zone**
- Phase vocoder, 4096-pt FFT, 1024 hop (75% overlap, Hann). Bin remapping = linear interp + max-magnitude-wins collision. Phase accum = `expected_target_increment + source_phase_deviation` (deviation NOT scaled by ratio). Identity short-circuit (shift=0, stretch=1) copies bins directly.
- Freeze stops input-ring writes; STFT keeps re-analyzing; transport-aware (silences output when transport stopped). Feedback feeds clamped (±4.0) wet back into input. Spectral display = lock-free AtomicU32 (128 bins × 256 cols). Frequency range bins outside pass through with original phase.

**pope-scope**
- Cursor tooltip: vertical line + time/bar label + per-track dB rows. Readings via `peak_at_column` (integer `div_ceil` mirrors `decimate_to_columns`' floor mapping); sparse path interpolates. Vertical mode restricts tooltip/cursor to hovered lane. `DragState::mouse_in_window` (CursorEntered/Left) prevents phantom (0,0) hover.
- Freeze consistency: `sync_mode`/`timebase_ms`/`sync_unit_bars` cached in `CachedViewParams` each rebuild; while frozen, grid + tooltip read cached params so sync/timebase edits can't desync the frozen waveform.
- Waveform = direct pixel-write pipeline (`fill_column_opaque`), no tiny-skia Paint/blitter. Half-split envelope smoothing: each column's top/bot is `min(own, (prev+own)/2, (own+next)/2)` — half-span polyline rasterization, smoother + symmetric. ~52% GUI CPU reduction vs the original path renderer.

**six-pack**
- Distorts the difference, not the signal: each band runs dry through a unity-when-flat SVF, `diff = svf_out − dry` (the EQ boost only); at 0 dB the SVF is analytically unity so diff is exactly zero. Saturation runs on the diff → band-gain knob = per-band drive. SVF exact unity at 0 dB is load-bearing for the whole architecture.
- M/S routing before saturation: SVF runs on L/R (response routing-independent); diff routed Stereo/Mid/Side; saturate; recombine to L/R additively.
- TPT SVF mix-form `dry + (peak_gain−1)·k·bandpass` (k=1/Q cancels bandpass peak height → peak magnitude = peak_gain regardless of Q). Shelves use lowpass/highpass branches. All unity at 0 dB.
- De-emphasis subtracts `Σ diff_i` so wet collapses to `dry + Σ(saturate(diff_i) − diff_i)` — only saturation harmonics audible. At drive→0 and mix≤50% output equals dry exactly.

**imagine**
- Width law = Ozone-style scale-the-side: `S_gain = (width+100)/100` (0..2), `M_gain = 1` always. The earlier constant-power law removed mid at +100 and gutted volume on mid-dominant music. At +100 with strong stereo, output may exceed 0 dBFS — handled with downstream gain.
- Stereoize Mode II = real Schroeder/Gerzon decorrelator (6-stage all-pass, mutually-prime delays {41,53,67,79,97,113}, SR-scaled) — xcorr <0.3. The original Hilbert-90 left xcorr ~+0.8.
- Recover Sides gated per-band by Width sign (only width<0 contributes to S_removed_total); Hilbert FIR rotates the residue, recover_amount mixes into mid. It's a *perceptual* control, not energy-preserving.
- IIR crossover = Lipshitz/Vanderkooy delay-matched cascade → 4-band sum allpass-equivalent (magnitude-flat ±0.05 dB). FIR crossover = double-buffered taps + sample-wise crossfade on swap, redesign gated on >0.5 Hz change.
- Hilbert is FIR-only (len 65, ~32 samples latency). An IIR all-pass cascade can't hit 90° at low freq; the FIR is exact and ~0.7 ms is sub-perceptual.
- Vectorscope ring = per-sample `AtomicU32` (f32 bit-pattern, no unsafe), Acquire/Release on write_pos. A torn (L,R) pair is sub-pixel given per-frame decimation.

**miff**
- Bipolar tap map `kernel[i] = 2·value − 1` places the MSEG midline (0.5) at a zero tap → values above 0.5 give positive taps, below give negative; flat-0.5 → all-zero kernel. Enables highpass/bandpass/comb FIRs from a [0,1] editor.
- Peak-magnitude normalization (not L1): bake taps → MAX_KERNEL FFT → divide by peak bin magnitude. Filter never boosts >0 dB, consistent loudness for any shape. L1 would only be correct for lowpass-ish shapes.
- Flat-0.5 default bakes to all-zero kernel → `Kernel::is_zero` makes Raw + Phaseless short-circuit to dry passthrough. Safe to insert before drawing.
- Phaseless mode uses a fixed 4096-pt STFT frame (= MAX_KERNEL) regardless of `Length` → constant 2048-sample latency that never jumps.
- `Length` is non-automatable (`IntParam::non_automatable()`) so the O(N log N) re-bake stays GUI-thread-triggered, off the audio thread.
- `bake_taps` walks the curve with a forward-only segment cursor, reproducing `mseg::value_at_phase` exactly without its per-tap rescan — O(N + nodes) not O(N·nodes).

**Dependencies**
- nih-plug points to `xxx/nih-plug` branch `finish-vst3-pr` — fork adds `Editor::set_size()`, `update_track_info()` + `TrackInfo`, `BYPASS_BUFFER_COPY`, nightly SIMD compat, VST3 license fix.
- baseview pinned to tag `v0.1.1` across every crate (and the nih-plug fork) so the tree resolves a single baseview. v0.1.1 fixes an x11 modifier-mask bug (`KeyButMask::BUTTON1/2/4` mis-wired to ALT/NUM_LOCK/META) that broke the MSEG editor's Alt-held stepped-draw.

## Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous equal-size chunks (256/512/1024/2048 samples).
- `.wt` — Surge-compatible; frame metadata in header.

Sample: `wavetable-filter/tests/fixtures/phaseless-bass.wt`.
