# Configurable Oversampling Design Spec

## Goal

Add configurable oversampling (1x, 2x, 4x, 8x) to reduce aliasing artifacts from the `tanh` drive saturation stage. Applies to both Raw and Phaseless filter modes. The ratio is a real-time automatable parameter.

## Architecture

Oversampling wraps around the existing processing pipeline. For each `process()` call:

1. **Upsample** the input buffer N× using `rubato` polyphase FIR
2. **Process** through the existing convolution/STFT pipeline at `effective_sample_rate = host_sample_rate × N`
3. **Downsample** the output buffer back to the host rate via `rubato` polyphase FIR

At 1x, the resamplers are bypassed entirely — zero overhead.

The kernel stays at KERNEL_LEN=2048 samples. Since `effective_sample_rate` is higher, the kernel naturally covers a narrower frequency range, which is correct. Note: at high oversampling ratios (8x), the `bin_to_src` factor in kernel synthesis increases, activating the peak-scan downsampling branch more often. This changes the filter's character slightly at high ratios — an acceptable trade-off.

## Parameter

`OversampleRatio` enum parameter (`#[id = "oversample"]`):
- `1x` (default)
- `2x`
- `4x`
- `8x`

Exposed to the DAW as a normal automatable parameter, same pattern as `FilterMode`.

## Buffers

Pre-allocated on `WavetableFilter` for the maximum ratio (8x) to avoid reallocation on ratio change:

| Buffer | Size (at max 8x) | Purpose |
|--------|-------------------|---------|
| `os_staging_in` | 2ch × max_buffer_size × 8 | Staging buffer for upsampled input |
| `os_staging_out` | 2ch × max_buffer_size × 8 | Staging buffer for oversampled output |
| `upsampler` | `rubato` resampler instance | Polyphase FIR upsampler |
| `downsampler` | `rubato` resampler instance | Polyphase FIR downsampler |

An internal accumulator layer handles the mismatch between rubato's preferred chunk size and the host's variable buffer size. Rubato's `process()` requires a specific number of input frames per call (reported via `input_frames_next()`), which may not match the host buffer size. The staging buffers accumulate samples until enough are available for a rubato chunk.

`FilterState` histories: call `FilterState::new(KERNEL_LEN * ratio)` which internally allocates `2 × KERNEL_LEN × ratio` for double-buffering. Pre-allocate at max ratio (8x) to avoid reallocation.

STFT buffers (`stft_in`, `stft_out`, `stft_window`, `stft_scratch`) and FFT plans stay at KERNEL_LEN — they do NOT scale with ratio. The STFT block size determines filter spectral resolution, which is independent of oversampling.

## Signal Flow

```
Host input (N samples, host SR)
  → accumulator → rubato upsample (N×ratio samples, host SR × ratio)
    → per-sample loop (convolution or STFT at effective SR)
      → tanh drive saturation (at oversampled rate — aliasing pushed above Nyquist)
    → dry/wet mix
  → rubato downsample → accumulator (N samples, host SR)
→ Host output
```

The `tanh` saturation is the main aliasing source. It runs at the oversampled rate, so aliasing products fall above the oversampled Nyquist and are removed by the downsampling anti-alias filter.

## Real-Time Ratio Switching

Detected at the top of `process()` by comparing current enum value vs `last_oversample_ratio`:

1. Recreate rubato resamplers for the new ratio (staging buffers are pre-allocated at max size, no reallocation needed)
2. Reset `FilterState` histories (wrong sample rate) — resize within the pre-allocated max capacity
3. Reset STFT state (clear `stft_in`, `stft_out`, reset positions — buffer sizes and FFT plans are unchanged)
4. Update `effective_sample_rate`
5. Re-synthesize kernel at new effective sample rate
6. Engage the existing 20ms crossfade to smooth the transition
7. Update latency reporting

Recreating rubato resamplers does allocate, but the staging buffers and FilterState are pre-allocated at max ratio so the allocation is limited to rubato internals. This will still cause a brief glitch on the switching buffer — acceptable for a heavyweight parameter change.

## Latency

`rubato` polyphase resamplers add latency (filter_length / 2 samples at the host rate). Added to existing latency:
- Raw mode: `resampler_latency`
- Phaseless mode: `HOP / ratio + resampler_latency` (HOP is in oversampled samples, divide by ratio to get host samples)

Reported via `context.set_latency_samples()`.

## Effective Sample Rate Propagation

`effective_sample_rate = host_sample_rate × ratio` replaces `sample_rate` in:
- Kernel synthesis frequency mapping (`bin_to_src` calculation)
- Silence detection threshold (`effective_sr × 0.1`)
- Crossfade step size (`1.0 / (effective_sr × 0.020)`)

These use `effective_sample_rate` because they operate on the oversampled stream.

The following must use `host_sample_rate` (NOT effective):
- Input spectrum update throttle — the ring buffer accumulates host-rate samples, so countdown should be `host_sr / 30.0`
- `shared_input_spectrum.0` (sample rate sent to GUI for frequency axis) — must be `host_sample_rate` since the spectrum is computed from the pre-upsampled input

## Testing

Written TDD-style before implementation:

1. **Round-trip fidelity** — Upsample then downsample a sine wave at each ratio. Output matches input within tolerance.
2. **Aliasing measurement** — High-frequency sine through `tanh` at 1x vs 4x. FFT output, measure spurious energy below Nyquist/2. 4x should have measurably less.
3. **Ratio switching** — Switch 1x→4x mid-buffer. No NaN/inf, output stays bounded.
4. **Effective sample rate** — At 2x, kernel synthesis uses `sample_rate × 2` for frequency mapping.
5. **Bypass equivalence** — At 1x, verify output matches expected behavior for same input (not bit-identical due to code path differences, but within floating-point tolerance).

## Dependencies

Add `rubato` to `Cargo.toml`. No other new dependencies.
