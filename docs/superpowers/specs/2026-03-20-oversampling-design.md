# Configurable Oversampling Design Spec

## Goal

Add configurable oversampling (1x, 2x, 4x, 8x) to reduce aliasing artifacts from the `tanh` drive saturation stage. Applies to both Raw and Phaseless filter modes. The ratio is a real-time automatable parameter.

## Architecture

Oversampling wraps around the existing processing pipeline. For each `process()` call:

1. **Upsample** the input buffer N× using `rubato` polyphase FIR
2. **Process** through the existing convolution/STFT pipeline at `effective_sample_rate = host_sample_rate × N`
3. **Downsample** the output buffer back to the host rate via `rubato` polyphase FIR

At 1x, the resamplers are bypassed entirely — zero overhead.

The kernel stays at KERNEL_LEN=2048 samples. Since `effective_sample_rate` is higher, the kernel naturally covers a narrower frequency range, which is correct.

## Parameter

`OversampleRatio` enum parameter (`#[id = "oversample"]`):
- `1x` (default)
- `2x`
- `4x`
- `8x`

Exposed to the DAW as a normal automatable parameter, same pattern as `FilterMode`.

## Buffers

Pre-allocated on `WavetableFilter`:

| Buffer | Size | Purpose |
|--------|------|---------|
| `upsample_in` | 2ch × host_buffer_size | Deinterleaved input for rubato |
| `upsample_out` | 2ch × host_buffer_size × ratio | Upsampled input |
| `downsample_in` | 2ch × host_buffer_size × ratio | Oversampled output for rubato |
| `downsample_out` | 2ch × host_buffer_size | Decimated output |
| `upsampler` | `rubato` resampler instance | Polyphase FIR upsampler |
| `downsampler` | `rubato` resampler instance | Polyphase FIR downsampler |

`FilterState` histories scale to `KERNEL_LEN × ratio × 2` (double-buffered). STFT buffers (`stft_in`, `stft_out`, `stft_window`, `stft_scratch`) scale by ratio. FFT plans are recreated at the new size.

## Signal Flow

```
Host input (N samples, host SR)
  → rubato upsample (N×ratio samples, host SR × ratio)
    → per-sample loop (convolution or STFT at effective SR)
      → tanh drive saturation (at oversampled rate — aliasing pushed above Nyquist)
    → dry/wet mix
  → rubato downsample (N samples, host SR)
→ Host output
```

The `tanh` saturation is the main aliasing source. It runs at the oversampled rate, so aliasing products fall above the oversampled Nyquist and are removed by the downsampling anti-alias filter.

## Real-Time Ratio Switching

Detected at the top of `process()` by comparing current enum value vs `last_oversample_ratio`:

1. Resize all oversampling buffers and recreate rubato resamplers
2. Reset `FilterState` histories (wrong sample rate)
3. Reset STFT state (wrong FFT size / hop)
4. Update `effective_sample_rate`
5. Re-synthesize kernel at new effective sample rate
6. Engage the existing 20ms crossfade to smooth the transition
7. Update latency reporting

Buffer reallocation on the audio thread is unavoidable for a real-time parameter, same trade-off as the existing wavetable reload path.

## Latency

`rubato` polyphase resamplers add latency (filter_length / 2 samples at the host rate). Added to existing latency:
- Raw mode: `resampler_latency`
- Phaseless mode: `HOP × ratio + resampler_latency` (scaled HOP at oversampled rate, converted back to host samples)

Reported via `context.set_latency_samples()`.

## Effective Sample Rate Propagation

`effective_sample_rate = host_sample_rate × ratio` replaces `sample_rate` in:
- Kernel synthesis frequency mapping (`bin_to_src` calculation)
- Silence detection threshold (`effective_sr × 0.1`)
- Crossfade step size (`1.0 / (effective_sr × 0.020)`)
- Input spectrum update throttle (`effective_sr / 30.0`)

## Testing

Written TDD-style before implementation:

1. **Round-trip fidelity** — Upsample then downsample a sine wave at each ratio. Output matches input within tolerance.
2. **Aliasing measurement** — High-frequency sine through `tanh` at 1x vs 4x. FFT output, measure spurious energy below Nyquist/2. 4x should have measurably less.
3. **Ratio switching** — Switch 1x→4x mid-buffer. No NaN/inf, output stays bounded.
4. **Effective sample rate** — At 2x, kernel synthesis uses `sample_rate × 2` for frequency mapping.
5. **Bypass equivalence** — At 1x, output is identical to the no-oversampling path.

## Dependencies

Add `rubato` to `Cargo.toml`. No other new dependencies.
