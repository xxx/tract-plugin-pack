# Warp Zone — Spectral Shifting/Stretching Effect Plugin

**Date:** 2026-04-01
**Status:** Design approved

## Overview

Warp Zone is a psychedelic spectral effect plugin that shifts and stretches audio in the frequency domain using a phase vocoder. It makes familiar sounds alien — voices from another dimension, instruments with impossible harmonic structures. A built-in LFO and envelope follower modulate the spectral parameters over time for evolving, liquid textures.

The GUI features flowing waveform ribbons that visualize per-band spectral activity, matching the psychedelic character of the effect.

## DSP Architecture

### Phase Vocoder

STFT-based analysis/resynthesis with bin remapping for frequency shifting and harmonic stretching.

**Signal flow:**

```
Input → Analysis STFT → Bin Remapping (shift + stretch) → Phase Accumulation → Synthesis STFT → Overlap-Add → Output
                                    ↑
                          Modulation (LFO / Envelope Follower)
```

**FFT configuration:**
- FFT size: 4096
- Hop size: 1024 (75% overlap)
- Window: Hann
- COLA normalization: same approach as satch (analysis × synthesis window with COLA factor)
- Latency: 4096 samples (~85 ms at 48 kHz)

**Bin remapping:**
- **Shift** remaps bins by a frequency ratio derived from semitone offset. Bin `k` maps to bin `k * 2^(shift/12)`.
- **Stretch** scales bin indices from the fundamental outward. Bin `k` maps to bin `k * stretch_factor`.
- When shift and stretch are combined, stretch is applied first, then shift.
- Output bins are computed via linear interpolation between source bins to avoid staircase artifacts.

**Phase accumulation:**
- Track expected phase increment per bin: `Δφ_expected = 2π × hop_size × k / fft_size`
- Compute phase deviation: `Δφ_dev = (measured_phase - last_phase) - Δφ_expected`, wrapped to [-π, π]
- Instantaneous frequency per bin: `ω_k = Δφ_expected + Δφ_dev`
- Accumulate phase into output bin: `out_phase[target_bin] += ω_k × (target_bin / source_bin)`
- This preserves phase coherence and prevents metallic artifacts from naive bin copying.

### Core Parameters

| Parameter | Range | Default | Unit | Description |
|-----------|-------|---------|------|-------------|
| Shift | -24 to +24 | 0 | semitones | Frequency bin offset |
| Stretch | 0.5 to 2.0 | 1.0 | ratio | Harmonic spacing scale factor |
| Mix | 0–100 | 100 | % | Dry/wet blend |

All parameters use `SmoothingStyle::Linear(50.0)` for per-block interpolation. Smoothed values are read once per hop (spectral parameters only change between FFT frames).

## Modulation System

### LFO

- **Waveform:** sine, triangle, sample-and-hold (random). Stepped selector.
- **Rate:** 0.01–20 Hz, skewed toward low frequencies (most useful range 0.1–5 Hz).
- **Shift depth:** 0–100%, scales LFO contribution to shift amount.
- **Stretch depth:** 0–100%, scales LFO contribution to stretch amount.
- Free-running phase, no tempo sync.

LFO is computed per-hop (every 1024 samples). The LFO phase advances by `rate × hop_size / sample_rate` per hop.

### Envelope Follower

- **Detection:** RMS-based amplitude, running-sum approach (same O(1) technique as gs-meter).
- **Attack:** 1–500 ms.
- **Release:** 10–2000 ms.
- **Shift depth:** -100% to +100%. Negative = louder signals shift down.
- **Stretch depth:** -100% to +100%. Negative = louder signals compress harmonics.

Envelope is computed per-sample for smooth tracking, but the modulation value is sampled once per hop for application to spectral parameters.

### Modulation Routing

```
final_shift   = base_shift   + (lfo_value × lfo_shift_depth)   + (envelope × env_shift_depth)
final_stretch = base_stretch + (lfo_value × lfo_stretch_depth) + (envelope × env_stretch_depth)
```

Final values are clamped to valid ranges (shift: -24..+24 semitones, stretch: 0.5..2.0).

## GUI & Visualization

### Layout

Single window, approximately 600×400 pixels. Softbuffer + tiny-skia CPU rendering, matching the existing plugin style (satch, tinylimit, gain-brain).

### Controls

- **Top row:** Shift dial, Stretch dial, Mix dial
- **Middle row:** LFO section — waveform selector, rate dial, shift depth dial, stretch depth dial
- **Bottom row:** Envelope section — attack dial, release dial, shift depth dial, stretch depth dial

All controls use `tiny-skia-widgets` (draw_dial, draw_slider, draw_stepped_selector).

### Waveform Ribbon Visualization

The main display area shows flowing ribbon paths representing spectral activity:

- **Bands:** 6 frequency bands spanning low to high frequencies (logarithmically spaced).
- **Motion:** Ribbons scroll left to right over time.
- **Y-position:** Undulates based on each band's spectral magnitude.
- **Color:** Per-band from a psychedelic palette (purples, magentas, cyans, electric blues).
- **Thickness:** Varies with magnitude — louder = thicker ribbons.
- **Trails:** Fading alpha trails (~0.5 s of scroll history) for a flowing/curling effect.
- **Parameter response:** Stretch visually spreads ribbons apart; shift drifts ribbons up/down.
- **Update rate:** 60 FPS, driven by editor timer.

### Rendering Approach

- Audio thread sends per-band magnitudes to GUI via bounded `crossbeam::channel` (`try_send`, never blocks).
- Current LFO and envelope values shared as `AtomicF32` for optional modulation indicators.
- Each frame: draw N ribbon paths as cubic Bezier curves through recent magnitude sample points.
- Trails achieved by not fully clearing the background — partial alpha fill (fade-to-black) each frame, drawing new ribbon positions on top.

## Code Structure

New crate `warp-zone/` added to the workspace.

| File | Role |
|------|------|
| `warp-zone/src/lib.rs` | Plugin struct, params, `process()`, metering atomics for GUI |
| `warp-zone/src/spectral.rs` | Phase vocoder: STFT analysis, bin remapping, phase accumulation, synthesis |
| `warp-zone/src/modulation.rs` | LFO (sine/tri/S&H) and envelope follower |
| `warp-zone/src/editor.rs` | Softbuffer + baseview editor, ribbon visualization, hit testing, controls |
| `warp-zone/src/main.rs` | Standalone entry point |

### Dependencies

Same stack as satch:
- `nih_plug` (git fork, `finish-vst3-pr` branch, `standalone` feature)
- `rustfft` 6
- `baseview` (git, `opengl` feature)
- `softbuffer` 0.4 (kms + x11 features)
- `raw-window-handle` 0.5 + 0.6
- `tiny-skia` 0.12
- `tiny-skia-widgets` (workspace path)
- `crossbeam` 0.8
- `serde` 1.0 (derive)
- `keyboard-types` 0.6

### Stereo Handling

Two independent `SpectralShifter` instances (L/R), same pattern as satch's `spectral_l`/`spectral_r`.

### Audio Thread → GUI Communication

- Per-band magnitude data (6 bands): bounded `crossbeam::channel`, `try_send` on audio thread.
- Current LFO value: `AtomicF32`.
- Current envelope value: `AtomicF32`.

## Testing Strategy

### `spectral.rs`
- Bin remapping correctness: verify target bin indices for known shift/stretch values.
- Phase accumulation: verify phase continuity across hops.
- Unity passthrough: shift=0, stretch=1.0 must reproduce input (within overlap-add tolerance).
- Frequency shift accuracy: feed known sine wave, verify output frequency matches expected shift.
- Stretch accuracy: feed harmonic series, verify spacing scales correctly.

### `modulation.rs`
- LFO waveform shapes: verify sine/triangle/S&H output ranges and periodicity.
- Envelope attack/release timing: verify rise and fall times match parameter values.
- Depth scaling: verify modulation output scales linearly with depth parameter.

### `lib.rs`
- Integration: full plugin process with known input, verify output is spectrally shifted.
- Latency compensation: dry/wet blend with latency-aligned dry path.
- Parameter smoothing: verify no clicks/pops on parameter changes.
- Zero input produces zero output (silence passthrough).
