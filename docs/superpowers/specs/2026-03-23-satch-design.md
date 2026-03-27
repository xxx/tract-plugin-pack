# satch Phase 1 Design Spec — Spectral Clipper MVP

## Overview

satch is a saturator with FFT-based spectral clipping and a Detail knob that controls the blend between standard time-domain saturation (0%) and per-bin spectral clipping (100%). Phase 1 is a minimal viable plugin to validate the core spectral clipping algorithm.

Inspired by Newfangled Audio Saturate's Detail Preservation feature.

## Plugin Parameters (Phase 1)

| Parameter | Type | Range | Default | Notes |
|-----------|------|-------|---------|-------|
| `drive` | FloatParam | 0 to 24 dB | 0 dB | Input boost into the clipper |
| `detail` | FloatParam | 0 to 100% | 0% | 0% = time-domain tanh, 100% = per-bin spectral tanh |
| `mix` | FloatParam | 0 to 100% | 100% | Dry/wet blend |

**Latency:** 2048 samples (fixed, from STFT window size). Reported to host.

## DSP Architecture

### Signal Flow

```
Input → [Drive boost (linear gain)] → ┬→ Time-domain: tanh(drive * sample) → weighted by (1 - detail)
                                       │
                                       └→ Spectral: STFT → per-bin tanh → ISTFT → weighted by detail
                                       │
                                       └→ Sum → [Mix: wet/dry] → Output
```

### STFT Parameters

- **Window size:** 2048 samples (~43ms at 48kHz)
- **Hop size:** 512 samples (75% overlap, 4x redundancy)
- **Window function:** Hann
- **FFT:** Real-to-complex via `rustfft` crate

### STFT Processing Pipeline

1. **Analysis:** Accumulate input samples into a ring buffer. Every `hop_size` samples, extract a 2048-sample frame, apply the Hann window, compute the FFT.
2. **Per-bin saturation:** For each complex bin, extract magnitude and phase. Apply `tanh(drive * magnitude)` to the magnitude. Reconstruct the complex value from the saturated magnitude and original phase.
3. **Synthesis:** Apply the inverse FFT, apply the synthesis window (Hann), accumulate into the output ring buffer with overlap-add.
4. **Output:** Read `hop_size` samples from the output ring buffer.

### Per-Bin Saturation Detail

For each FFT bin `k` (0 to N/2):
```
mag = sqrt(re[k]^2 + im[k]^2)
phase = atan2(im[k], re[k])
sat_mag = tanh(drive_linear * mag) / drive_linear  // normalize to preserve unity gain at low levels
re_out[k] = sat_mag * cos(phase)
im_out[k] = sat_mag * sin(phase)
```

The `/ drive_linear` normalization ensures that at low drive levels, the output magnitude ≈ input magnitude (tanh(x) ≈ x for small x, so tanh(drive * mag) / drive ≈ mag).

### Time-Domain Path

Standard waveshaping: `tanh(drive_linear * sample) / drive_linear` (same normalization).

### Detail Blend

```
output = detail * spectral_output + (1 - detail) * timedomain_output
```

At 0%: pure time-domain clipper.
At 100%: pure spectral clipper.
Between: linear crossfade.

### Dry/Wet Mix

```
final = mix * output + (1 - mix) * dry_input
```

The dry input is delayed by the STFT latency (2048 samples) to align with the wet signal.

## Crate Structure

```
satch/
├── Cargo.toml
├── src/
│   ├── lib.rs          — plugin struct, params, process()
│   ├── main.rs         — standalone entry point
│   ├── spectral.rs     — STFT analysis/synthesis, per-bin saturation
│   ├── editor.rs       — minimal GUI (3 dials + level display)
│   └── fonts/DejaVuSans.ttf
```

### Dependencies

- `nih-plug` (same fork, features = ["standalone"])
- `rustfft` — pure Rust FFT library
- `tiny-skia-widgets` (shared widget crate)
- `softbuffer`, `tiny-skia`, `fontdue`, `baseview`, `crossbeam`, `serde`

## spectral.rs — Core DSP Module

### SpectralClipper struct

```rust
pub struct SpectralClipper {
    // FFT state
    fft: Arc<dyn rustfft::Fft<f32>>,
    ifft: Arc<dyn rustfft::Fft<f32>>,
    fft_size: usize,        // 2048
    hop_size: usize,        // 512

    // Buffers (pre-allocated)
    input_ring: Vec<f32>,    // circular input buffer
    output_ring: Vec<f32>,   // circular output accumulator
    ring_pos: usize,
    hop_counter: usize,

    // Working buffers (reused per FFT frame)
    window: Vec<f32>,        // Hann window (2048)
    fft_buffer: Vec<Complex<f32>>,  // FFT scratch (2048)
    frame_buffer: Vec<f32>,  // windowed frame (2048)
}
```

### API

```rust
impl SpectralClipper {
    pub fn new(fft_size: usize, hop_size: usize) -> Self;
    pub fn reset(&mut self);
    pub fn latency_samples(&self) -> usize; // = fft_size

    /// Process one sample. Returns the output sample (delayed by fft_size).
    /// `drive_linear` is the linear drive amount.
    pub fn process_sample(&mut self, sample: f32, drive_linear: f32) -> f32;
}
```

The per-sample interface hides the STFT complexity. Internally it accumulates samples, triggers FFT processing every `hop_size` samples, and reads from the output buffer.

### No Allocations in process_sample

All buffers are pre-allocated in `new()`. The FFT library's scratch buffer is also pre-allocated. `process_sample` does zero heap allocation.

## GUI (Phase 1 — Minimal)

### Window Size

300 x 300 at 1x scale.

### Layout

```
┌──────────────────────┐
│   satch         - +  │
├──────────────────────┤
│   [Drive dial]       │
│   [Detail dial]      │
│   [Mix dial]         │
└──────────────────────┘
```

Three dials stacked vertically. Same tiny-skia rendering as other plugins.

## Testing Strategy

### Unit Tests (spectral.rs)

1. **Passthrough at drive=0:** Output should equal input (delayed by latency). Verify for sine wave and impulse.
2. **Time-domain tanh at detail=0:** Output matches `tanh(drive * input)` for a sine wave.
3. **Spectral clipper preserves quiet partials:** Feed a loud 100 Hz sine + quiet 5 kHz sine. At detail=0%, the 5 kHz is damaged when 100 Hz clips. At detail=100%, the 5 kHz is preserved.
4. **Output level doesn't exceed drive-normalized ceiling:** No sample in the output exceeds 1.0 for normalized input.
5. **Latency matches fft_size:** The output is delayed by exactly `fft_size` samples relative to the input.
6. **Overlap-add reconstruction:** With drive=0 (no saturation), the STFT analysis-synthesis should perfectly reconstruct the input (within floating-point tolerance).

### Integration Tests

- Process a full buffer through the plugin at various detail levels
- Verify mix=0% produces dry output (delayed)

## Phase 2 (Future — Not in This Spec)

- Shape control (soft to hard clipping curve)
- Symmetry control (even harmonics)
- Ceiling control
- ADAA antialiasing
- Delta listen mode
- Input/output meters
- More waveshaper types (tube, tape, fold)

## Non-Goals (Phase 1)

- No oversampling (STFT handles antialiasing via windowing)
- No multiband
- No presets
- No metering beyond basic level display
- No MIDI
