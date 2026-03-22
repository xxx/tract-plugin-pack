# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Tract Plugin Pack is a Cargo workspace containing multiple audio effect plugins (VST3, CLAP, standalone) built with [nih-plug](https://github.com/robbert-vdh/nih-plug) in Rust.

### Plugins

**Wavetable Filter** — uses wavetable frames as FIR filter kernels. Two modes: Raw (direct convolution, zero latency) and Phaseless (STFT magnitude-only filtering, no pre-ringing). GUI uses nih_plug_vizia.

**GS Meter** — lightweight loudness meter with gain utility for clip-to-zero workflows. Tracks peak, true peak (ITU-R BS.1770-4), RMS integrated/momentary, crest factor. GUI uses softbuffer + tiny-skia (CPU rendering, no GPU). Designed for 50+ instances per project.

## Workspace Structure

```
tract-plugin-pack/
├── wavetable-filter/       # Wavetable-based filter plugin (vizia GUI)
├── gs-meter/               # Loudness meter + gain utility (softbuffer GUI)
├── nih-plug-widgets/       # Shared vizia widgets (ParamDial, CSS theme)
├── docs/                   # Plugin manuals (markdown + PDF)
└── xtask/                  # Build tooling
```

## Build Commands

Requires **nightly Rust** (enforced via `rust-toolchain.toml`) for portable SIMD (`std::simd::f32x16`).

```bash
# Plugin bundles (VST3 + CLAP)
cargo nih-plug bundle wavetable-filter --release
cargo nih-plug bundle gs-meter --release

# Standalone binaries
cargo build --bin wavetable-filter --release
cargo build --bin gs-meter --release

# Debug standalone (for GUI testing without DAW)
cargo build --bin gs-meter
```

## Testing & Linting

```bash
cargo test --workspace                            # all tests (80+)
cargo clippy --workspace -- -D warnings           # lint (CI uses -D warnings)
cargo fmt --check
```

Tests are inline `#[cfg(test)]` modules:
- `wavetable-filter/src/lib.rs` and `wavetable-filter/src/wavetable.rs` — 30 DSP tests
- `gs-meter/src/meter.rs` — 50 meter tests (RMS, peak, true peak, SIMD, stereo)
- `gs-meter/src/widgets.rs` — 13 widget rendering tests
- Test fixtures: `wavetable-filter/tests/fixtures/`

## Development Practices

- **Prefer TDD**: Write tests before or alongside implementation. New DSP functions and data structures should have tests covering normal operation, edge cases, and error paths.
- **Never commit unless asked**: Do not create git commits unless the user explicitly requests it. This is a hard rule with zero exceptions.
- **No allocations on the audio thread**: `process()` must never allocate. Use pre-allocated buffers, `try_lock()` for shared data, and avoid `Vec::new()`, `clone()` of collections, or `String` operations in the hot path.
- **No unsafe code**: Do not use `unsafe` blocks. Find safe alternatives or restructure the code to avoid needing unsafe. Exception: FFI windowing glue (raw-window-handle trait impls, `Send` for window handles) where the underlying API requires it.
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
| `gs-meter/src/widgets.rs` | tiny-skia drawing primitives: labels, buttons, sliders, text renderer |
| `gs-meter/src/fonts/DejaVuSans.ttf` | Embedded font for CPU text rendering |

### Shared

| File | Role |
|------|------|
| `nih-plug-widgets/src/lib.rs` | Re-exports ParamDial, provides `load_style()` for vizia CSS |
| `nih-plug-widgets/src/param_dial.rs` | Custom rotary knob widget with modulation indicator |
| `nih-plug-widgets/src/style.css` | Dark theme CSS for vizia plugins |

### Key Design Decisions

- **GS Meter uses CPU rendering** (softbuffer + tiny-skia + fontdue) instead of vizia/OpenGL. This eliminates 25 MB of GPU driver overhead (Mesa/LLVM) per instance. At 50 instances: 16% CPU, 48 MB total.
- **True peak uses exact ITU-R BS.1770-4 coefficients** (48-tap, 4-phase polyphase FIR). Double-buffered history for contiguous SIMD dot products. Sample-rate-aware: 4x OS at <96kHz, 2x at 96-192kHz, bypass at >=192kHz.
- **Stereo RMS uses sum-of-power** (matches dpMeter5 SUM mode): `sqrt(ms_L + ms_R)`.
- **Crest factor uses dpMeter5's convention** (peak_stereo vs rms_stereo), not the mathematically correct max(crest_L, crest_R). Documented for future "correct mode" toggle.
- **RMS momentary uses O(1) running sum** (f64 precision, incremental add/subtract) instead of O(N) ring scan per buffer.
- **nih-plug dependency** currently points to `davemollen/nih-plug` branch `finish-vst3-pr` for nightly SIMD compatibility and VST3 license fix.

### Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous chunks of equal size (256/512/1024/2048 samples)
- `.wt` — Surge-compatible wavetable format; frame metadata in header

Sample wavetable: `wavetable-filter/tests/fixtures/phaseless-bass.wt`
