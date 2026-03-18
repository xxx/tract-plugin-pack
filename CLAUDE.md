# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A wavetable-based audio filter plugin (VST3, CLAP, standalone) that uses wavetable frames as FIR filter kernels. Supports two modes:
- **RAW**: Direct time-domain convolution using the wavetable as-is
- **Minimum Phase**: Converts wavetable to minimum-phase before convolution for snappier transients

## Build Commands

Requires **nightly Rust** (enforced via `rust-toolchain.toml`) for portable SIMD (`std::simd::f32x16`).

```bash
# Plugin bundles (VST3 + CLAP)
cargo nih-plug bundle wavetable-filter --release
cargo nih-plug bundle wavetable-filter          # debug

# Standalone binary
cargo build --bin wavetable-filter --release
cargo build --bin wavetable-filter              # debug
```

## Testing & Linting

```bash
cargo test                                        # all tests
cargo test test_wavetable_creation -- --nocapture # single test with output
cargo clippy
cargo fmt --check
```

Tests live in `src/wavetable.rs` (inline `#[cfg(test)]` module).

## Architecture

### Key Modules

| File | Role |
|------|------|
| `src/lib.rs` | Plugin DSP: convolution loop, parameter smoothing, FFT-based kernel resampling |
| `src/wavetable.rs` | Wavetable I/O (`.wav`/`.wt`), frame interpolation, sampling |
| `src/editor.rs` | Vizia UI layout, file browser, parameter controls |
| `src/editor/wavetable_view.rs` | 3D overhead perspective rendering of wavetable frames |
| `src/editor/filter_response_view.rs` | Frequency response visualization |
| `src/main.rs` | Standalone entry point |

### Core Data Structures

**`WavetableFilter`** — main DSP processor (implements nih-plug's `Plugin` trait). Owns `FilterState` per channel, FFT plans, and the current filter kernel.

**`FilterState`** — per-channel convolution state. Uses a power-of-2 circular buffer with bit-mask modulo (`idx & (len - 1)`) for speed. `get_bulk::<16>()` copies history slices directly for SIMD reads.

**`Wavetable`** — multi-frame container. Frames are independent FIR kernels; frame interpolation blends adjacent frames linearly.

**`WavetableFilterParams`** — nih-plug parameter struct: Frequency (log 20 Hz–20 kHz), Frame Position (0–1), Mix, Drive, Mode.

### Processing Pipeline

Per-sample audio loop:
1. Read smoothed parameters
2. On frame position change → interpolate kernel from wavetable frames
3. On cutoff frequency change → resample kernel (maps harmonic 24 of the wavetable to target freq via linear interpolation)
4. SIMD convolution: 16 lanes at a time (`f32x16`), accumulating dot product of history × kernel
5. Dry/wet mix; Drive applies `tanh` saturation

Minimum phase conversion (in `lib.rs`): FFT → log-magnitude cepstrum → Hilbert window (double positive cepstral freqs, zero negative) → IFFT. Preserves magnitude spectrum, eliminates pre-ringing.

### Performance Notes

- SIMD via `std::simd::f32x16` (nightly-only portable SIMD)
- Circular buffer bit-mask avoids division
- Silence detection clears filter state when input is idle
- Typical CPU usage: <5% for 256-sample wavetables at 48 kHz

### Wavetable File Formats

- `.wav` — standard WAV; frames are contiguous chunks of equal size
- `.wt` — Surge-compatible wavetable format; frame metadata in header

Sample wavetable: `phaseless-bass.wt` in repo root.
