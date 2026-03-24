# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tract Plugin Pack is a Cargo workspace containing multiple audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

### Plugins

**Wavetable Filter** — uses wavetable frames as FIR filter kernels. Two modes: Raw (direct convolution, zero latency) and Phaseless (STFT magnitude-only filtering, no pre-ringing). GUI uses nih_plug_vizia.

**GS Meter** — lightweight loudness meter with gain utility for clip-to-zero workflows. dB mode: peak, true peak (ITU-R BS.1770-4), RMS integrated/momentary, crest factor. LUFS mode: EBU R128 integrated/short-term/momentary loudness, LRA, true peak. Per-mode gain and reference with gain-match buttons. GUI uses softbuffer + tiny-skia (CPU rendering, no GPU). Designed for 100+ instances per project.

**Gain Brain** — lightweight gain utility with cross-instance group linking via mmap IPC. 16 groups, Absolute/Relative link modes, Invert toggle for mirrored gain movement. GUI uses softbuffer + tiny-skia (CPU rendering). ~8 KB per instance headless. Inspired by BlueCat's Gain Suite.

**tinylimit** — low-latency wideband peak limiter for track-level use. Feed-forward with lookahead, dual-stage transient/dynamics envelope, soft knee (Giannoulis 2012), optional ISP (ITU-1770 true peak). 7 built-in character presets. GUI uses softbuffer + tiny-skia (CPU rendering). 50 instances @ 6.2% CPU, 50 MB RSS (~1.0 MB, 0.12% CPU per instance). Inspired by DMG Audio TrackLimit.

## Workspace Structure

```
tract-plugin-pack/
├── wavetable-filter/       # Wavetable-based filter plugin (vizia GUI)
├── gs-meter/               # Loudness meter + gain utility (softbuffer GUI)
├── gain-brain/             # Gain utility with group linking (softbuffer GUI)
├── tinylimit/              # Wideband peak limiter (softbuffer GUI)
├── nih-plug-widgets/       # Shared vizia widgets (ParamDial, CSS theme)
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

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release
cargo build --bin gain-brain --release
cargo build --bin tinylimit --release

# Debug standalone (for GUI testing without DAW)
cargo build --bin gs-meter
cargo build --bin gain-brain
cargo build --bin tinylimit
```

## Testing & Linting

```bash
cargo test --workspace                            # all tests (170+)
cargo clippy --workspace -- -D warnings           # lint (CI uses -D warnings)
cargo fmt --check
```

Tests are inline `#[cfg(test)]` modules:
- `wavetable-filter/src/lib.rs` and `wavetable-filter/src/wavetable.rs` — 30 DSP tests
- `gs-meter/src/meter.rs` — 50 meter tests (RMS, peak, true peak, SIMD, stereo)
- `gain-brain/src/groups.rs` — 11 mmap IPC tests
- `gain-brain/src/lib.rs` — 16 sync/conversion tests
- `tinylimit/src/limiter.rs` — 33 limiter tests (gain computer, envelope, lookahead, integration)
- `tiny-skia-widgets/` — 20 widget rendering tests (dial, slider, button, text)
- Test fixtures: `wavetable-filter/tests/fixtures/`

## Development Practices

- **Prefer TDD**: Write tests before or alongside implementation. New DSP functions and data structures should have tests covering normal operation, edge cases, and error paths.
- **Never commit unless asked**: Do not create git commits unless the user explicitly requests it. This is a hard rule with zero exceptions.
- **No allocations on the audio thread**: `process()` must never allocate. Use pre-allocated buffers, `try_lock()` for shared data, and avoid `Vec::new()`, `clone()` of collections, or `String` operations in the hot path.
- **No unsafe code**: Do not use `unsafe` blocks. Find safe alternatives or restructure the code to avoid needing unsafe. Exceptions: FFI windowing glue (raw-window-handle trait impls, `Send` for window handles) where the underlying API requires it; `memmap2` constructors (`MmapMut::map_mut`) for cross-instance shared memory in gain-brain.
- **Don't guess at fixes**: Write tests to verify, add debug logging to diagnose, dispatch agents to review. Never claim a fix works without evidence.
- **Use the LSP tool**: Prefer the LSP tool over grep for code navigation. Fall back to grep only when LSP is unavailable.

## Architecture

### Wavetable Filter

| File | Role |
|------|------|
| `wavetable-filter/src/lib.rs` | Plugin DSP: convolution, STFT, kernel synthesis, parameter smoothing |
| `wavetable-filter/src/wavetable.rs` | Wavetable I/O (`.wav`/`.wt`), frame interpolation |
| `wavetable-filter/src/editor.rs` | Vizia UI layout, file browser, scaling controls |
| `wavetable-filter/src/editor/wavetable_view.rs` | 2D/3D wavetable visualization |
| `wavetable-filter/src/editor/filter_response_view.rs` | Frequency response + input spectrum graph |

### GS Meter

| File | Role |
|------|------|
| `gs-meter/src/lib.rs` | Plugin integration, process() loop, parameter definitions |
| `gs-meter/src/meter.rs` | Core metering DSP: RMS, peak, true peak (ITU BS.1770-4), crest factor, SIMD |
| `gs-meter/src/editor.rs` | Softbuffer + baseview editor, hit testing, mouse interaction, scaling |
| `gs-meter/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### Gain Brain

| File | Role |
|------|------|
| `gain-brain/src/lib.rs` | Plugin struct, params, process(), group sync logic |
| `gain-brain/src/groups.rs` | Mmap IPC: GroupFile, shared memory layout, read/write slots |
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

### Shared

| File | Role |
|------|------|
| `tiny-skia-widgets/src/primitives.rs` | Color palette, draw_rect, draw_rect_outline |
| `tiny-skia-widgets/src/text.rs` | TextRenderer with fontdue glyph cache |
| `tiny-skia-widgets/src/controls.rs` | draw_button, draw_slider, draw_stepped_selector |
| `tiny-skia-widgets/src/param_dial.rs` | Arc-based rotary dial widget (draw_dial) |
| `nih-plug-widgets/src/lib.rs` | Re-exports vizia ParamDial, provides `load_style()` for vizia CSS |
| `nih-plug-widgets/src/param_dial.rs` | Vizia rotary knob widget with modulation indicator |
| `nih-plug-widgets/src/style.css` | Dark theme CSS for vizia plugins |

### Key Design Decisions

- **GS Meter uses CPU rendering** (softbuffer + tiny-skia + fontdue) instead of vizia/OpenGL. This eliminates 25 MB of GPU driver overhead (Mesa/LLVM) per instance. At 300 instances (Bitwig, 48kHz/1024): 15% CPU, 560 MB RSS (~1.8 MB per instance).
- **True peak uses exact ITU-R BS.1770-4 coefficients** (48-tap, 4-phase polyphase FIR). Double-buffered history for contiguous SIMD dot products. Sample-rate-aware: 4x OS at <96kHz, 2x at 96-192kHz, bypass at >=192kHz.
- **Stereo RMS uses sum-of-power** (matches dpMeter5 SUM mode): `sqrt(ms_L + ms_R)`.
- **Crest factor uses dpMeter5's convention** (peak_stereo vs rms_stereo), not the mathematically correct max(crest_L, crest_R). Documented for future "correct mode" toggle.
- **RMS momentary uses O(1) running sum** (f64 precision, incremental add/subtract) instead of O(N) ring scan per buffer.
- **Gain Brain uses mmap IPC** (memmap2) for cross-instance group linking. 272-byte shared file with 16 group slots. The fd is closed after mapping — zero persistent file descriptors. ~8 KB per instance headless.
- **Gain Brain inversion** is applied on both reads and writes. The slot stores the writer's coordinate-space value. `write_slot_rebaseline` (bumps `baseline_generation`) is used only for invert toggle events, not for normal writes. Relative readers re-baseline on `baseline_generation` changes without applying a delta.
- **tinylimit uses feed-forward lookahead** with a backward-pass gain reduction ramp (DanielRudrich approach). Signal flow: gain computer → lookahead backward pass → dual-stage envelope → apply to delayed audio → safety clip. Hard knee fast path skips log/exp for sub-threshold samples. `exp()` instead of `powf()` for gain application (2x faster). Threshold/ceiling lerped per block (2 `exp` calls) instead of per-sample `powf`.
- **nih-plug dependency** currently points to `davemollen/nih-plug` branch `finish-vst3-pr` for nightly SIMD compatibility and VST3 license fix.

### Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous chunks of equal size (256/512/1024/2048 samples)
- `.wt` — Surge-compatible wavetable format; frame metadata in header

Sample wavetable: `wavetable-filter/tests/fixtures/phaseless-bass.wt`
