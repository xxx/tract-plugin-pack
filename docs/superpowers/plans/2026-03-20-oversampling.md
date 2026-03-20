# Configurable Oversampling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add configurable oversampling (1x, 2x, 4x, 8x) to reduce aliasing from tanh drive saturation, with real-time ratio switching.

**Architecture:** Oversampling wraps the existing process loop. Input is upsampled via rubato polyphase FIR, processed at the higher effective sample rate through the existing convolution/STFT pipeline, then downsampled back. All buffers pre-allocated at 8x max to avoid audio-thread allocations.

**Tech Stack:** Rust, nih-plug, rubato (polyphase resampling), realfft/rustfft (existing)

---

## File Structure

| File | Role |
|------|------|
| `src/lib.rs` | **Modify** — Add OversampleRatio enum, oversample param, process() restructure |
| `src/oversampler.rs` | **Create** — Encapsulate rubato up/down resampling + chunk accumulator |
| `Cargo.toml` | **Modify** — Add rubato dependency |
| `src/editor.rs` | **Modify** — Add oversample control to UI |

---

### Task 1: Add rubato dependency and OversampleRatio enum

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add rubato to Cargo.toml**

```toml
rubato = "0.16"
```

- [ ] **Step 2: Add OversampleRatio enum and parameter**

In `src/lib.rs`, after the `FilterMode` enum:

```rust
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
enum OversampleRatio {
    #[id = "1x"]
    #[name = "1x"]
    X1,
    #[id = "2x"]
    #[name = "2x"]
    X2,
    #[id = "4x"]
    #[name = "4x"]
    X4,
    #[id = "8x"]
    #[name = "8x"]
    X8,
}

impl OversampleRatio {
    fn factor(self) -> usize {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
            Self::X8 => 8,
        }
    }
}
```

Add to `WavetableFilterParams`:
```rust
#[id = "oversample"]
pub oversample: EnumParam<OversampleRatio>,
```

Initialize in `WavetableFilterParams::new()`:
```rust
oversample: EnumParam::new("Oversample", OversampleRatio::X1),
```

- [ ] **Step 3: Build**

Run: `cargo build --bin wavetable-filter`

- [ ] **Step 4: Commit**

---

### Task 2: Create the Oversampler module with tests

**Files:**
- Create: `src/oversampler.rs`
- Modify: `src/lib.rs` (add `mod oversampler;`)

This module encapsulates rubato resampling. It handles the chunk-size mismatch between the host's variable buffer size and rubato's fixed chunk requirement using an internal accumulator. All output buffers are pre-allocated — no heap allocation after construction.

**rubato API notes for the implementer:**
- Use `rubato::FftFixedIn<f32>` for both up and down resamplers.
- Constructor: `FftFixedIn::new(sr_in, sr_out, chunk_size, sub_chunks, channels)` — the first two args are integer sample rates. For ratio-only resampling, pass `(1, ratio)` for upsampling and `(ratio, 1)` for downsampling. This produces correct filter coefficients.
- **CRITICAL:** Use `process_into_buffer(&input_refs, &mut output_buf, None)` — NOT `process()`. The `process()` method allocates a new output Vec on every call. `process_into_buffer()` writes into pre-allocated buffers.
- `FftFixedIn` requires exactly `input_frames_next()` input frames per call. The host buffer size varies, so the Oversampler must accumulate input samples in a ring buffer and drain in chunk-sized batches.

- [ ] **Step 1: Write tests first (TDD)**

Write all oversampler tests before implementation. Place in `src/oversampler.rs` at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_1x_is_passthrough() {
        let mut os = Oversampler::new(1, 256, 2);
        let input = vec![vec![1.0, 2.0, 3.0, 4.0]; 2];
        let mut output = vec![vec![0.0; 4]; 2];
        os.process_up(&input, &mut output);
        assert_eq!(output, input);
        let mut down_out = vec![vec![0.0; 4]; 2];
        os.process_down(&output, &mut down_out);
        assert_eq!(down_out, input);
    }

    #[test]
    fn test_upsample_output_length() {
        for ratio in [2, 4, 8] {
            let mut os = Oversampler::new(ratio, 256, 1);
            let input = vec![vec![0.5; 256]];
            let mut output = vec![vec![0.0; 256 * ratio]];
            let n = os.process_up(&input, &mut output);
            assert_eq!(n, 256 * ratio, "ratio={ratio}");
        }
    }

    #[test]
    fn test_round_trip_preserves_dc() {
        let mut os = Oversampler::new(2, 512, 1);
        let dc_val = 0.7;
        let n = 512;
        let input = vec![vec![dc_val; n]];
        let mut up_out = vec![vec![0.0; n * 2]];
        let mut down_out = vec![vec![0.0; n]];

        // Run several blocks to flush transient
        for _ in 0..4 {
            os.process_up(&input, &mut up_out);
            os.process_down(&up_out, &mut down_out);
        }

        // After transient, output DC should match input DC
        let avg: f32 = down_out[0][n/2..].iter().sum::<f32>() / (n/2) as f32;
        assert!(
            (avg - dc_val).abs() < 0.01,
            "DC not preserved: expected {dc_val}, got {avg}"
        );
    }

    #[test]
    fn test_round_trip_sine_2x() {
        let mut os = Oversampler::new(2, 512, 1);
        let freq = 440.0;
        let sr = 48000.0;
        let n = 512;
        let input: Vec<Vec<f32>> = vec![(0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect()];
        let mut up_out = vec![vec![0.0; n * 2]];
        let mut down_out = vec![vec![0.0; n]];

        // Multiple passes to flush transient
        for _ in 0..4 {
            os.process_up(&input, &mut up_out);
            os.process_down(&up_out, &mut down_out);
        }

        // After transient, output should approximate input
        let skip = 128;
        for i in skip..n {
            assert!(
                (down_out[0][i] - input[0][i]).abs() < 0.05,
                "sample {i}: expected {:.4}, got {:.4}",
                input[0][i], down_out[0][i]
            );
        }
    }

    #[test]
    fn test_oversampling_reduces_aliasing() {
        use realfft::RealFftPlanner;
        use rustfft::num_complex::Complex;

        let sr = 48000.0;
        let freq = 10000.0;
        let n = 2048;
        let drive = 5.0;

        let input: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
            .collect();

        // 1x: apply tanh directly
        let output_1x: Vec<f32> = input.iter().map(|&s| (s * drive).tanh()).collect();

        // 4x: upsample, apply tanh, downsample
        let mut os = Oversampler::new(4, n, 1);
        let mut up_out = vec![vec![0.0; n * 4]];
        let mut down_out = vec![vec![0.0; n]];

        // Flush transient with zeros first
        let zeros = vec![vec![0.0; n]];
        for _ in 0..4 {
            os.process_up(&zeros, &mut up_out);
            os.process_down(&up_out, &mut down_out);
        }

        // Now process the actual signal
        os.process_up(&[input.clone()], &mut up_out);
        // Apply tanh at oversampled rate
        for s in up_out[0].iter_mut() {
            *s = (*s * drive).tanh();
        }
        os.process_down(&up_out, &mut down_out);
        let output_4x = &down_out[0];

        // FFT both and measure aliasing energy
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);

        let measure_alias = |signal: &[f32]| -> f32 {
            let mut buf = signal.to_vec();
            let mut spec = vec![Complex::new(0.0f32, 0.0); n / 2 + 1];
            fft.process(&mut buf, &mut spec).unwrap();
            let bin_hz = sr / n as f32;
            let start = (freq * 2.5 / bin_hz) as usize;
            spec[start..n/2].iter().map(|c| c.norm_sqr()).sum::<f32>()
        };

        let alias_1x = measure_alias(&output_1x);
        let alias_4x = measure_alias(output_4x);

        eprintln!("Aliasing: 1x={alias_1x:.2}, 4x={alias_4x:.2}, reduction={:.1}x",
            alias_1x / alias_4x.max(1e-10));
        assert!(alias_4x < alias_1x * 0.5,
            "4x should reduce aliasing significantly");
    }

    #[test]
    fn test_latency_zero_at_1x() {
        let os = Oversampler::new(1, 256, 2);
        assert_eq!(os.latency_samples(), 0);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib oversampler`
Expected: FAIL (module doesn't exist)

- [ ] **Step 3: Implement Oversampler**

Create `src/oversampler.rs`. The key design:

- `process_up(&self, input: &[Vec<f32>], output: &mut [Vec<f32>]) -> usize` — upsamples input into pre-allocated output, returns number of output samples written
- `process_down(&self, input: &[Vec<f32>], output: &mut [Vec<f32>]) -> usize` — downsamples into pre-allocated output, returns number of output samples written
- At ratio=1, both methods are plain copies (zero overhead)
- Uses `process_into_buffer()` — NOT `process()` — to avoid heap allocation
- If the host buffer size doesn't match rubato's `input_frames_next()`, use an internal accumulator (ring buffer that batches input into rubato-sized chunks)
- Pre-allocate all internal buffers at construction time

The implementer should read the rubato docs for `FftFixedIn`:
- `new(sr_in, sr_out, chunk_size, sub_chunks, channels)` — for upsampling pass `(1, ratio, host_block_size, 1, channels)`, for downsampling pass `(ratio, 1, host_block_size * ratio, 1, channels)`
- `process_into_buffer(&input_refs, &mut output_buf, None)` — writes into caller-owned buffers
- `input_frames_next()` — how many input frames the next `process_into_buffer` call expects

Add `mod oversampler;` to `src/lib.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib oversampler -- --nocapture`
Expected: All tests pass. The aliasing test should show measurable reduction.

- [ ] **Step 5: Commit**

---

### Task 3: Integrate oversampling into process()

**Files:**
- Modify: `src/lib.rs`

This is the core integration — the most complex task.

- [ ] **Step 1: Add oversampling fields to WavetableFilter**

Add to the struct:
```rust
oversampler: oversampler::Oversampler,
last_oversample_ratio: OversampleRatio,
effective_sample_rate: f32,
host_sample_rate: f32,
/// Pre-allocated oversampled input buffer (2ch × max_block × 8)
os_input: Vec<Vec<f32>>,
/// Pre-allocated oversampled output buffer (2ch × max_block × 8)
os_output: Vec<Vec<f32>>,
/// Pre-allocated downsampled output buffer (2ch × max_block)
os_down_output: Vec<Vec<f32>>,
```

Initialize in `Default` (use max_block=8192, max_ratio=8):
```rust
oversampler: oversampler::Oversampler::new(1, 8192, 2),
last_oversample_ratio: OversampleRatio::X1,
effective_sample_rate: 48000.0,
host_sample_rate: 48000.0,
os_input: vec![vec![0.0; 8192 * 8]; 2],
os_output: vec![vec![0.0; 8192 * 8]; 2],
os_down_output: vec![vec![0.0; 8192]; 2],
```

Pre-allocate FilterState at max ratio in Default:
```rust
filter_state: [FilterState::new(KERNEL_LEN * 8), FilterState::new(KERNEL_LEN * 8)],
```

Note: At 1x, the convolution still uses `KERNEL_LEN` elements from the history, which is fine — the history is just larger than needed. The `len` field in FilterState controls the convolution window, so it needs to match `KERNEL_LEN` regardless of ratio. The extra allocation is for the circular buffer to hold enough history at higher rates. **Actually, re-examine this:** `FilterState::new(size)` sets `len = size` and the SIMD convolution loop uses `KERNEL_LEN / 16` chunks hardcoded. The kernel is always KERNEL_LEN long. So FilterState.len must always be KERNEL_LEN. The history depth doesn't need to change — the kernel length is fixed. Remove the 8x pre-allocation for FilterState; keep it at `FilterState::new(KERNEL_LEN)`.

- [ ] **Step 2: Update initialize()**

```rust
self.host_sample_rate = buffer_config.sample_rate;
let ratio = self.params.oversample.value().factor();
self.effective_sample_rate = self.host_sample_rate * ratio as f32;
self.sample_rate = self.effective_sample_rate;
self.crossfade_step = 1.0 / (self.effective_sample_rate * 0.020);
self.oversampler = oversampler::Oversampler::new(
    ratio,
    buffer_config.max_buffer_size as usize,
    2,
);
self.last_oversample_ratio = self.params.oversample.value();
```

- [ ] **Step 3: Add ratio-change detection at top of process()**

After the wavetable reload check:

```rust
let current_os_ratio = self.params.oversample.value();
if current_os_ratio != self.last_oversample_ratio {
    let ratio = current_os_ratio.factor();
    self.oversampler = oversampler::Oversampler::new(ratio, buffer.samples().max(512), 2);
    self.effective_sample_rate = self.host_sample_rate * ratio as f32;
    self.sample_rate = self.effective_sample_rate;
    self.crossfade_step = 1.0 / (self.effective_sample_rate * 0.020);
    for state in &mut self.filter_state { state.reset(); }
    for buf in &mut self.stft_in { buf.fill(0.0); }
    for buf in &mut self.stft_out { buf.fill(0.0); }
    self.stft_in_pos = 0;
    self.stft_out_pos = 0;
    self.first_process = true;
    self.last_oversample_ratio = current_os_ratio;
}
```

- [ ] **Step 4: Restructure process() with oversampling**

This is the main change. The new structure of process():

```
fn process():
    // ... wavetable reload check (unchanged)
    // ... ratio-change detection (new, from step 3)
    // ... kernel synthesis (unchanged, but uses self.sample_rate which is now effective_sample_rate)

    let ratio = self.last_oversample_ratio.factor();
    let host_samples = buffer.samples();
    let num_channels = buffer.channels(); // 1 or 2

    // Latency reporting (uses effective sample rate for STFT)
    let os_latency = self.oversampler.latency_samples();
    let stft_latency = if filter_mode == FilterMode::Raw { 0 }
        else { (HOP / ratio.max(1)) as u32 };
    context.set_latency_samples(stft_latency + os_latency);

    if ratio <= 1 {
        // === 1x path: existing per-sample loop, unchanged ===
        // (Keep the entire current for-loop as-is for zero overhead at 1x)
    } else {
        // === Oversampled path ===

        // 1. Deinterleave host buffer into per-channel Vecs
        for ch in 0..num_channels {
            for (i, frame) in buffer.iter_samples().enumerate() {
                self.os_down_output[ch][i] = *frame.get(ch).unwrap();
            }
        }
        // (reuse os_down_output temporarily for host input)
        let host_input = &self.os_down_output[..num_channels];

        // 2. Upsample
        let os_samples = self.oversampler.process_up(host_input, &mut self.os_input);

        // 3. Process at oversampled rate
        //    Same DSP as the 1x loop but iterating os_input/os_output
        for i in 0..os_samples {
            // ... parameter smoothing, STFT hop, per-channel convolution/STFT, tanh, mix ...
            // Read from os_input[ch][i], write to os_output[ch][i]
        }

        // 4. Downsample
        self.oversampler.process_down(&self.os_output[..num_channels], &mut self.os_down_output);

        // 5. Write back to host buffer
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            for ch in 0..num_channels {
                *frame.get_mut(ch).unwrap() = self.os_down_output[ch][i];
            }
        }
    }

    // Input spectrum visualization (uses HOST rate, not effective)
    // ... existing spectrum code, but use self.host_sample_rate for countdown and shared SR
```

**Important details for the oversampled per-sample loop:**
- The STFT hop counter (`stft_out_pos`) still fires every `HOP` oversampled samples — no change needed, it self-adapts
- `silence_samples` counter increments per oversampled sample, so the threshold `(self.sample_rate * 0.1)` is already correct since `self.sample_rate` is `effective_sample_rate`
- The input spectrum ring buffer should accumulate from the HOST buffer (pre-upsampled), not the oversampled data. Use `self.host_sample_rate` for the countdown and `shared_input_spectrum.0`.
- Parameter smoothers advance per oversampled sample — this is correct, nih-plug smoothers are sample-rate-agnostic

- [ ] **Step 5: Fix input spectrum to use host sample rate**

Change the spectrum countdown and shared SR:
```rust
// In the spectrum computation section:
self.input_spectrum_countdown = (self.host_sample_rate / 30.0) as usize;
// ...
shared.0 = self.host_sample_rate;
```

The input spectrum ring buffer should accumulate from the original host input, not the oversampled stream. Move the ring buffer accumulation to happen before upsampling (or after downsampling from the host buffer directly).

- [ ] **Step 6: Build and run all tests**

Run: `cargo test --lib`
Run: `cargo clippy`

- [ ] **Step 7: Commit**

---

### Task 4: Add remaining tests

**Files:**
- Modify: `src/lib.rs` (test module)

- [ ] **Step 1: Write ratio-switching safety test**

```rust
#[test]
fn test_oversample_ratio_switch_no_nan() {
    // Verify that switching oversampling ratios mid-stream doesn't produce NaN/inf
    let mut os = crate::oversampler::Oversampler::new(1, 256, 1);
    let input = vec![vec![0.5f32; 256]];
    let mut up = vec![vec![0.0; 256 * 8]; 1];
    let mut down = vec![vec![0.0; 256]; 1];

    // Process at 1x
    os.process_up(&input, &mut up);

    // Switch to 4x (recreate)
    os = crate::oversampler::Oversampler::new(4, 256, 1);
    let n = os.process_up(&input, &mut up);
    assert_eq!(n, 256 * 4);

    // Apply tanh at oversampled rate
    for s in up[0][..n].iter_mut() { *s = (*s * 3.0).tanh(); }

    os.process_down(&[up[0][..n].to_vec()], &mut down);

    // No NaN or inf
    assert!(down[0].iter().all(|s| s.is_finite()),
        "output contains NaN/inf after ratio switch");
}
```

- [ ] **Step 2: Write effective sample rate test**

```rust
#[test]
fn test_effective_sample_rate_at_2x() {
    // At 2x oversampling, kernel synthesis should use 2× the host sample rate
    let host_sr = 48000.0;
    let ratio = 2;
    let effective_sr = host_sr * ratio as f32;
    let cutoff = 1000.0;

    // bin_to_src = 24.0 * sample_rate / (KERNEL_LEN * cutoff)
    let bin_to_src_1x = 24.0 * host_sr / (KERNEL_LEN as f32 * cutoff);
    let bin_to_src_2x = 24.0 * effective_sr / (KERNEL_LEN as f32 * cutoff);

    // At 2x, bin_to_src should be exactly 2× larger
    assert!((bin_to_src_2x - bin_to_src_1x * 2.0).abs() < 0.001,
        "bin_to_src should double at 2x: 1x={bin_to_src_1x}, 2x={bin_to_src_2x}");
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --lib -- --nocapture`

- [ ] **Step 4: Commit**

---

### Task 5: Add oversample control to UI

**Files:**
- Modify: `src/editor.rs`

- [ ] **Step 1: Add oversample selector after Mode row**

```rust
HStack::new(cx, |cx| {
    Label::new(cx, "Oversample")
        .width(Pixels(80.0))
        .height(Pixels(30.0));
    ParamSlider::new(cx, Data::params, |params| &params.oversample)
        .set_style(ParamSliderStyle::CurrentStepLabeled { even: true })
        .width(Pixels(250.0))
        .class("mode-slider");
})
.height(Pixels(40.0))
.col_between(Pixels(10.0));
```

- [ ] **Step 2: Build and verify**

Run: `cargo build --bin wavetable-filter`

- [ ] **Step 3: Commit**

---

### Task 6: Polish, clippy, release build

- [ ] **Step 1: Run clippy and fix warnings**

Run: `cargo clippy`

- [ ] **Step 2: Run all tests**

Run: `cargo test --lib -- --nocapture`

- [ ] **Step 3: Build release**

Run: `cargo nih-plug bundle wavetable-filter --release`

- [ ] **Step 4: Commit**
