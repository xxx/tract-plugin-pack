# STFT Magnitude-Only Filtering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the cepstral minimum-phase algorithm with STFT-based magnitude-only filtering that applies the filter's frequency response without altering the signal's phase.

**Architecture:** In `FilterMode::Minimum`, instead of time-domain convolution with a minimum-phase kernel, process audio through an STFT pipeline: Hann-windowed blocks → FFT → multiply each bin by a real-valued magnitude gain (preserving input phase) → IFFT → overlap-add. The magnitude gains come from the same spectrum computation already used for RAW mode. RAW mode is unchanged. Latency of HOP (1024) samples is inherent to STFT and reported to the host.

**Tech Stack:** Rust nightly (portable SIMD), `realfft` crate (RealToComplex/ComplexToReal), `nih-plug` plugin framework.

---

## Constants

```rust
const HOP: usize = KERNEL_LEN / 2;  // 1024 — overlap-add hop size (50% overlap)
```

## File Structure

All changes are in a single file:

- **Modify:** `src/lib.rs` — the entire DSP engine

No new files needed.

## Key Design Decisions

1. **Overlap-add buffer management:** The `stft_out` buffer is KERNEL_LEN samples. After reading HOP output samples, the tail (indices HOP..KERNEL_LEN-1) is shifted down to indices 0..HOP-1, and the upper half is zeroed. Then the new STFT frame's output is added to the entire buffer.

2. **Dedicated STFT scratch buffer:** A separate `stft_scratch` Vec<f32> (KERNEL_LEN) is used as the time-domain FFT/IFFT scratch, avoiding corruption of `synthesized_kernel` which RAW mode needs.

3. **Borrow checker strategy:** `process_stft_frame` is a static method taking individual field references. Rust's borrow checker allows disjoint field borrows (e.g., `&self.stft_in[ch]` and `&mut self.stft_out[ch]` are different fields). The `Arc<dyn Fft>` plans are cloned (cheap Arc clone) into locals before the loop to avoid shared borrows on `self`.

4. **Default mode:** The `FilterMode` enum is reordered so `Raw` is the first variant (and thus the default), since it has no latency.

5. **Mode switching:** When the mode changes at runtime, STFT buffers are cleared and positions reset. The existing reset-fade mechanism produces a brief fade-out to avoid clicks.

6. **Latency reporting:** The plugin reports HOP samples of latency to the host when in STFT mode (0 for RAW).

---

### Task 1: Remove minimum-phase algorithm and related fields

**Files:**
- Modify: `src/lib.rs:53-54` (struct fields `cplx_fft`, `cplx_ifft`)
- Modify: `src/lib.rs:71` (struct field `cplx_work`)
- Modify: `src/lib.rs:112-121` (FilterMode enum — reorder so Raw is first/default)
- Modify: `src/lib.rs:136-138` (Default impl — remove complex FFT planner)
- Modify: `src/lib.rs:163-164,172` (Default impl — remove field init)
- Modify: `src/lib.rs:372-411` (apply_resonance_and_ifft — remove mode/cplx params)
- Delete: `src/lib.rs:416-461` (compute_minimum_phase_kernel_inplace)
- Modify: `src/lib.rs:775-830` (initialize — remove cplx params from synthesis call)
- Modify: `src/lib.rs:899-957` (process — remove cplx params from synthesis call)
- Modify: `src/lib.rs:851-877` (process — wavetable reload path)
- Delete: `src/lib.rs:1245-1277` (make_test_kernel_full)
- Delete: `src/lib.rs:1650-end` (all minimum-phase tests + diagnostic tests)

- [ ] **Step 1: Reorder `FilterMode` enum so `Raw` is first (default)**

```rust
#[derive(Enum, Clone, Copy, Debug, PartialEq)]
pub enum FilterMode {
    #[id = "raw"]
    #[name = "Raw"]
    Raw,

    #[id = "minimum"]
    #[name = "Phaseless"]
    Minimum,
}
```

This makes `Raw` the default variant (no latency). The `#[id = "minimum"]` is preserved for preset compatibility. Display name changes to "Phaseless".

- [ ] **Step 2: Remove `compute_minimum_phase_kernel_inplace` and its call site**

Delete the function (lines 416-461). In `apply_resonance_and_ifft` (line 408-410), remove the `if mode == FilterMode::Minimum` block.

- [ ] **Step 3: Remove `mode`, `cplx_work`, `cplx_fft`, `cplx_ifft` from `apply_resonance_and_ifft` signature**

New signature:
```rust
fn apply_resonance_and_ifft(
    base_mags: &[f32],
    bin_fracs: &[f32],
    resonance: f32,
    spectrum_work: &mut [Complex<f32>],
    kernel_out: &mut [f32],
    kernel_ifft: &Arc<dyn ComplexToReal<f32>>,
) {
```

Update all 3 call sites:
- `initialize()` (~line 809)
- `process()` kernel synthesis (~line 941)
- `process()` wavetable reload path (~line 867)

- [ ] **Step 4: Remove `cplx_fft`, `cplx_ifft`, `cplx_work` from `WavetableFilter` struct and `Default` impl**

Remove from struct (lines 53-54, 71). Remove planner + field initialization from `Default::default()` (lines 136-138, 163-164, 172).

- [ ] **Step 5: Remove min-phase test code**

Delete:
- `make_test_kernel_full` (lines 1245-1277)
- `test_minimum_phase_preserves_magnitude_and_eliminates_tail` (lines 1659-1742)
- `test_minimum_phase_on_brickwall_lowpass` (lines 1744-1805)
- `test_minimum_phase_on_simple_symmetric_kernel` (lines 1807-end)

Simplify `make_test_kernel_with_resonance` to inline the logic instead of calling `make_test_kernel_full`:
```rust
fn make_test_kernel_with_resonance(cutoff_hz: f32, resonance: f32) -> Vec<f32> {
    let sample_rate = 48000.0f32;
    let wt = WavetableFilter::create_default_wavetable();
    let mut planner = RealFftPlanner::<f32>::new();
    let frame_fft = planner.plan_fft_forward(wt.frame_size);
    let kernel_ifft = planner.plan_fft_inverse(KERNEL_LEN);

    let frame = wt.get_frame_interpolated(0.0);
    let (base_mags, bin_fracs) =
        WavetableFilter::compute_base_spectrum(&frame, cutoff_hz, sample_rate, &frame_fft)
            .expect("compute_base_spectrum returned None");

    let mut spectrum_work = vec![Complex::new(0.0_f32, 0.0); KERNEL_LEN / 2 + 1];
    let mut kernel = vec![0.0f32; KERNEL_LEN];

    WavetableFilter::apply_resonance_and_ifft(
        &base_mags,
        &bin_fracs,
        resonance,
        &mut spectrum_work,
        &mut kernel,
        &kernel_ifft,
    );
    kernel
}
```

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All existing tests pass (minus the deleted min-phase tests).

- [ ] **Step 7: Commit**

```bash
git add src/lib.rs
git commit -m "remove cepstral minimum-phase algorithm

The minimum-phase mode is being replaced with STFT-based
magnitude-only filtering. This removes the old algorithm,
simplifies apply_resonance_and_ifft, and reorders FilterMode
so Raw is the default (no latency)."
```

---

### Task 2: Add STFT infrastructure and Hann window

**Files:**
- Modify: `src/lib.rs:20-76` (WavetableFilter struct — add fields)
- Modify: `src/lib.rs:124-177` (Default impl — initialize new fields)
- Modify: `src/lib.rs:775-830` (initialize() — set up STFT buffers)

- [ ] **Step 1: Add STFT fields to WavetableFilter struct**

Add after existing fields (near line 76):
```rust
    // ── STFT state for magnitude-only (Phaseless) mode ──────────────
    /// Forward real FFT plan for STFT input blocks (size KERNEL_LEN).
    stft_fft: Arc<dyn RealToComplex<f32>>,
    /// Per-channel circular input buffer for STFT (KERNEL_LEN samples each).
    stft_in: [Vec<f32>; 2],
    /// Per-channel overlap-add output accumulator (KERNEL_LEN samples each).
    stft_out: [Vec<f32>; 2],
    /// Current filter magnitude spectrum for STFT mode (KERNEL_LEN/2+1 real gains).
    stft_magnitudes: Vec<f32>,
    /// Pre-computed Hann analysis window (KERNEL_LEN samples).
    stft_window: Vec<f32>,
    /// Time-domain scratch buffer for STFT FFT/IFFT (KERNEL_LEN).
    /// Separate from synthesized_kernel to avoid corruption on mode switch.
    stft_scratch: Vec<f32>,
    /// Write position in STFT input circular buffer (0..KERNEL_LEN-1).
    stft_in_pos: usize,
    /// Read position within current STFT output hop (0..HOP-1).
    stft_out_pos: usize,
    /// Tracks the last mode to detect runtime mode switches.
    last_mode: FilterMode,
```

- [ ] **Step 2: Initialize STFT fields in Default impl**

In `Default::default()`, add the forward FFT plan:
```rust
let stft_fft = real_planner.plan_fft_forward(KERNEL_LEN);
```

And add the fields to the struct literal:
```rust
stft_fft,
stft_in: [vec![0.0; KERNEL_LEN], vec![0.0; KERNEL_LEN]],
stft_out: [vec![0.0; KERNEL_LEN], vec![0.0; KERNEL_LEN]],
stft_magnitudes: vec![0.0; KERNEL_LEN / 2 + 1],
stft_window: {
    let mut w = vec![0.0f32; KERNEL_LEN];
    for i in 0..KERNEL_LEN {
        w[i] = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / KERNEL_LEN as f32).cos());
    }
    w
},
stft_scratch: vec![0.0; KERNEL_LEN],
stft_in_pos: 0,
stft_out_pos: 0,
last_mode: FilterMode::Raw,
```

- [ ] **Step 3: Reset STFT state in initialize()**

In `initialize()`, after buffer setup, add:
```rust
for buf in &mut self.stft_in {
    buf.fill(0.0);
}
for buf in &mut self.stft_out {
    buf.fill(0.0);
}
self.stft_in_pos = 0;
self.stft_out_pos = 0;
self.last_mode = self.params.mode.value();
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass (STFT fields exist but aren't used yet).

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs
git commit -m "add STFT infrastructure fields and Hann window"
```

---

### Task 3: Add magnitude spectrum computation for STFT mode

**Files:**
- Modify: `src/lib.rs` — add `compute_stft_magnitudes` method
- Modify: `src/lib.rs` — call it in `initialize()` and `process()` when mode is Minimum

- [ ] **Step 1: Write test for magnitude computation**

```rust
#[test]
fn test_stft_magnitudes_match_spectrum() {
    // Verify that compute_stft_magnitudes produces the expected magnitude values.
    let sample_rate = 48000.0f32;
    let cutoff = 2000.0f32;
    let resonance = 0.3f32;
    let wt = WavetableFilter::create_default_wavetable();
    let mut planner = RealFftPlanner::<f32>::new();
    let frame_fft = planner.plan_fft_forward(wt.frame_size);

    let frame = wt.get_frame_interpolated(0.0);
    let (base_mags, bin_fracs) =
        WavetableFilter::compute_base_spectrum(&frame, cutoff, sample_rate, &frame_fft)
            .expect("spectrum failed");

    let mut stft_mags = vec![0.0f32; KERNEL_LEN / 2 + 1];
    WavetableFilter::compute_stft_magnitudes(
        &base_mags, &bin_fracs, resonance, &mut stft_mags,
    );

    // All values should be finite and non-negative
    assert!(stft_mags.iter().all(|v| v.is_finite() && *v >= 0.0));
    // DC bin should have a value (lowpass passes DC)
    assert!(stft_mags[0] > 0.0, "DC magnitude should be non-zero for lowpass");
    // High-frequency bins should be near zero for lowpass
    let nyquist_mag = stft_mags[KERNEL_LEN / 2];
    assert!(nyquist_mag < 0.01, "Nyquist magnitude should be near zero for 2kHz lowpass");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_stft_magnitudes_match_spectrum`
Expected: FAIL — `compute_stft_magnitudes` does not exist.

- [ ] **Step 3: Implement `compute_stft_magnitudes`**

Add as a static method on `WavetableFilter`:
```rust
/// Compute filter magnitude gains for STFT mode.
///
/// Applies the resonance comb to base magnitudes and writes real-valued
/// gains to `mags_out`. Each gain is the factor by which an FFT bin's
/// magnitude should be scaled (the bin's phase is preserved).
fn compute_stft_magnitudes(
    base_mags: &[f32],
    bin_fracs: &[f32],
    resonance: f32,
    mags_out: &mut [f32],
) {
    let comb_exp = resonance * 8.0;
    for j in 0..base_mags.len().min(mags_out.len()) {
        mags_out[j] = if comb_exp > 0.01 {
            let dist = bin_fracs[j].min(1.0 - bin_fracs[j]);
            let comb = (std::f32::consts::PI * dist).cos().max(0.0).powf(comb_exp);
            base_mags[j] * comb
        } else {
            base_mags[j]
        };
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_stft_magnitudes_match_spectrum`
Expected: PASS

- [ ] **Step 5: Wire up magnitude computation in `process()` parameter-change path**

In the `if needs_update` block (around line 899), after computing `out_mags`/`out_fracs`, add a mode branch:
```rust
if filter_mode == FilterMode::Raw {
    // Existing RAW kernel synthesis path (crossfade bake, apply_resonance_and_ifft, reverse)
    // ... (unchanged)
} else {
    // Magnitude-only: just store the magnitude spectrum for STFT
    Self::compute_stft_magnitudes(
        &self.out_mags,
        &self.out_fracs,
        resonance,
        &mut self.stft_magnitudes,
    );
}
```

Similarly in `initialize()`: if initial mode is Minimum, compute magnitudes instead of a kernel. If Raw, do existing kernel synthesis.

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/lib.rs
git commit -m "add magnitude spectrum computation for STFT mode"
```

---

### Task 4: Implement STFT processing in the per-sample loop

**Files:**
- Modify: `src/lib.rs` — add `process_stft_frame` method
- Modify: `src/lib.rs` — branch in per-sample loop on filter mode

This is the core task. The per-sample loop must branch:
- **RAW:** existing push → SIMD convolution → output (unchanged).
- **Phaseless (STFT):** push to STFT input buffer → read from STFT output buffer → at hop boundary, shift output tail + process a full STFT frame.

- [ ] **Step 1: Write test for STFT pass-through (flat spectrum)**

```rust
#[test]
fn test_stft_passthrough_flat_spectrum() {
    // With all-ones magnitude spectrum, STFT output should approximate
    // the input signal (no phase shift, amplitude preserved).
    let mut plugin = WavetableFilter::default();
    plugin.stft_magnitudes.fill(1.0);

    for buf in &mut plugin.stft_in { buf.fill(0.0); }
    for buf in &mut plugin.stft_out { buf.fill(0.0); }
    plugin.stft_in_pos = 0;
    plugin.stft_out_pos = 0;

    let num_samples = KERNEL_LEN * 4;
    let freq = 1000.0f32;
    let sr = 48000.0f32;
    let input: Vec<f32> = (0..num_samples)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
        .collect();

    let mut output = vec![0.0f32; num_samples];
    for i in 0..num_samples {
        // At hop boundary: shift output tail, then process new frame
        if plugin.stft_out_pos == 0 && i > 0 {
            for ch in 0..2 {
                // Shift tail: move indices HOP..KERNEL_LEN down to 0..HOP
                plugin.stft_out[ch].copy_within(HOP..KERNEL_LEN, 0);
                plugin.stft_out[ch][HOP..].fill(0.0);

                WavetableFilter::process_stft_frame(
                    &plugin.stft_in[ch],
                    plugin.stft_in_pos,
                    &mut plugin.stft_out[ch],
                    &plugin.stft_magnitudes,
                    &plugin.stft_window,
                    &plugin.stft_fft,
                    &plugin.kernel_ifft,
                    &mut plugin.stft_scratch,
                    &mut plugin.spectrum_work,
                );
            }
        }

        // Push input
        plugin.stft_in[0][plugin.stft_in_pos] = input[i];
        plugin.stft_in[1][plugin.stft_in_pos] = input[i];

        // Read output
        output[i] = plugin.stft_out[0][plugin.stft_out_pos];

        plugin.stft_in_pos = (plugin.stft_in_pos + 1) & (KERNEL_LEN - 1);
        plugin.stft_out_pos += 1;
        if plugin.stft_out_pos >= HOP {
            plugin.stft_out_pos = 0;
        }
    }

    // After initial transient (2*KERNEL_LEN samples), output should track input
    let start = KERNEL_LEN * 2;
    let mut max_err = 0.0f32;
    for i in start..num_samples {
        let err = (output[i] - input[i]).abs();
        max_err = max_err.max(err);
    }
    assert!(
        max_err < 0.05,
        "STFT pass-through error too large: {max_err:.4}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_stft_passthrough_flat_spectrum`
Expected: FAIL — `process_stft_frame` does not exist.

- [ ] **Step 3: Implement `process_stft_frame`**

Add as a static method:
```rust
/// Process one STFT frame: window input → FFT → magnitude multiply → IFFT → overlap-add.
///
/// All buffers are caller-provided — zero heap allocation.
/// Call AFTER shifting the output buffer tail (see overlap-add protocol).
fn process_stft_frame(
    stft_in: &[f32],           // Circular input buffer (KERNEL_LEN)
    in_pos: usize,             // Current write position (= oldest sample index)
    stft_out: &mut [f32],      // Overlap-add output accumulator (KERNEL_LEN)
    magnitudes: &[f32],        // Filter magnitude gains (KERNEL_LEN/2+1)
    window: &[f32],            // Hann window (KERNEL_LEN)
    fft: &Arc<dyn RealToComplex<f32>>,
    ifft: &Arc<dyn ComplexToReal<f32>>,
    scratch_time: &mut [f32],  // Time-domain scratch (KERNEL_LEN)
    scratch_freq: &mut [Complex<f32>], // Freq-domain scratch (KERNEL_LEN/2+1)
) {
    let n = KERNEL_LEN;
    let mask = n - 1;

    // 1. Extract + window: read oldest-to-newest from circular buffer
    for i in 0..n {
        scratch_time[i] = stft_in[(in_pos + i) & mask] * window[i];
    }

    // 2. Forward FFT (consumes scratch_time, writes scratch_freq)
    if fft.process(scratch_time, scratch_freq).is_err() {
        return;
    }

    // 3. Multiply each bin by magnitude gain (preserves input phase)
    for (bin, &mag) in scratch_freq.iter_mut().zip(magnitudes.iter()) {
        *bin *= mag;
    }

    // 4. Inverse FFT (consumes scratch_freq, writes scratch_time)
    if ifft.process(scratch_freq, scratch_time).is_err() {
        return;
    }

    // 5. Scale and overlap-add into output buffer
    let scale = 1.0 / n as f32;
    for i in 0..n {
        stft_out[i] += scratch_time[i] * scale;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_stft_passthrough_flat_spectrum`
Expected: PASS

- [ ] **Step 5: Integrate STFT into the per-sample loop in `process()`**

Read `filter_mode` once before the sample loop. The structure becomes:

```rust
let filter_mode = self.params.mode.value();

// Detect mode switch: clear STFT buffers to avoid stale data
if filter_mode != self.last_mode {
    if filter_mode != FilterMode::Raw {
        // Switching TO STFT: clear buffers
        for buf in &mut self.stft_in { buf.fill(0.0); }
        for buf in &mut self.stft_out { buf.fill(0.0); }
        self.stft_in_pos = 0;
        self.stft_out_pos = 0;
    }
    self.last_mode = filter_mode;
}

for mut channel_samples in buffer.iter_samples() {
    // ... (smoothers, mix, drive, reset_gain — unchanged)

    if filter_mode != FilterMode::Raw && self.stft_out_pos == 0 {
        // STFT hop boundary: shift output tail, then process new frame.
        // Clone Arc plans to satisfy borrow checker (cheap ref-count bump).
        let stft_fft = self.stft_fft.clone();
        let kernel_ifft = self.kernel_ifft.clone();
        for ch in 0..2 {
            self.stft_out[ch].copy_within(HOP..KERNEL_LEN, 0);
            self.stft_out[ch][HOP..].fill(0.0);

            Self::process_stft_frame(
                &self.stft_in[ch],
                self.stft_in_pos,
                &mut self.stft_out[ch],
                &self.stft_magnitudes,
                &self.stft_window,
                &stft_fft,
                &kernel_ifft,
                &mut self.stft_scratch,
                &mut self.spectrum_work,
            );
        }
    }

    for (channel_idx, sample) in channel_samples.iter_mut().enumerate() {
        let state_idx = channel_idx.min(1);
        let input = *sample;

        if input.abs() > silence_threshold {
            is_silent = false;
        }

        if filter_mode == FilterMode::Raw {
            // Existing RAW path: drive → push → SIMD convolution (unchanged)
            let driven_input = (input * drive).tanh();
            self.filter_state[state_idx].push(driven_input);
            // ... existing SIMD convolution code ...
            *sample = input * (1.0 - mix) + filtered * mix * reset_gain;
        } else {
            // STFT path: push to input buffer, read from output buffer
            let driven_input = (input * drive).tanh();
            self.stft_in[state_idx][self.stft_in_pos] = driven_input;
            let filtered = self.stft_out[state_idx][self.stft_out_pos];
            *sample = input * (1.0 - mix) + filtered * mix * reset_gain;
        }
    }

    if filter_mode == FilterMode::Raw {
        // Existing crossfade/reset logic (unchanged)
        // ...
    } else {
        // Advance STFT positions
        self.stft_in_pos = (self.stft_in_pos + 1) & (KERNEL_LEN - 1);
        self.stft_out_pos += 1;
        if self.stft_out_pos >= HOP {
            self.stft_out_pos = 0;
        }
    }

    // Reset fade logic (shared between modes, unchanged)
    // ...
}
```

**Important:** The `stft_out` read position does NOT need to be cleared after reading (unlike the original plan). The overlap-add buffer is shifted at the hop boundary, which naturally moves consumed data out and zeroes the tail.

Wait — correction: the shift happens at `stft_out_pos == 0`, which is the BEGINNING of a new hop. At that point, we've read all HOP samples from the previous hop. The shift moves indices HOP..KERNEL_LEN-1 down to 0..HOP-1, and zeroes HOP..KERNEL_LEN-1. Then the new frame is overlap-added. So the values at 0..HOP-1 now contain the tail of the previous frame PLUS the first half of the new frame. Then we read 0..HOP-1 one sample at a time during this hop. This is correct.

- [ ] **Step 6: Handle STFT state reset in silence detection**

In the silence detection block (around line 1042), also clear STFT buffers and reset positions:
```rust
if is_silent {
    self.silence_samples += buffer.samples();
    if self.silence_samples > (self.sample_rate * 0.1) as usize {
        for state in &mut self.filter_state {
            state.reset();
        }
        for buf in &mut self.stft_in {
            buf.fill(0.0);
        }
        for buf in &mut self.stft_out {
            buf.fill(0.0);
        }
        self.stft_in_pos = 0;
        self.stft_out_pos = 0;
    }
}
```

- [ ] **Step 7: Report latency to host**

Add latency reporting. In nih-plug, use `context.set_latency_samples()` at the start of `process()`:

```rust
let filter_mode = self.params.mode.value();
let latency = if filter_mode == FilterMode::Raw { 0 } else { HOP as u32 };
context.set_latency_samples(latency);
```

Note: Some hosts handle dynamic latency changes better than others. This reports the accurate latency per mode.

- [ ] **Step 8: Run all tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/lib.rs
git commit -m "implement STFT magnitude-only filtering for phaseless mode

Replaces the old cepstral minimum-phase algorithm with STFT-based
processing that applies the filter's magnitude response without
altering the signal's phase. Uses Hann-windowed overlap-add with
50% overlap (HOP = KERNEL_LEN/2 = 1024 samples). Reports latency
to host. Handles runtime mode switching gracefully."
```

---

### Task 5: STFT integration tests

**Files:**
- Modify: `src/lib.rs` — add tests in `#[cfg(test)]` module

- [ ] **Step 1: Write test — STFT lowpass attenuates high frequencies**

```rust
#[test]
fn test_stft_lowpass_attenuates_highs() {
    // With a lowpass magnitude spectrum (low bins = 1.0, high bins = 0.0),
    // a high-frequency sine should be attenuated.
    let mut plugin = WavetableFilter::default();

    // Set up a lowpass: pass first 100 bins, zero rest
    let cutoff_bin = 100;
    for i in 0..plugin.stft_magnitudes.len() {
        plugin.stft_magnitudes[i] = if i < cutoff_bin { 1.0 } else { 0.0 };
    }

    for buf in &mut plugin.stft_in { buf.fill(0.0); }
    for buf in &mut plugin.stft_out { buf.fill(0.0); }
    plugin.stft_in_pos = 0;
    plugin.stft_out_pos = 0;

    // High-frequency sine (10 kHz at 48 kHz SR — bin ~426)
    let num_samples = KERNEL_LEN * 4;
    let freq = 10000.0f32;
    let sr = 48000.0f32;
    let input: Vec<f32> = (0..num_samples)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
        .collect();

    let mut output = vec![0.0f32; num_samples];
    for i in 0..num_samples {
        if plugin.stft_out_pos == 0 && i > 0 {
            plugin.stft_out[0].copy_within(HOP..KERNEL_LEN, 0);
            plugin.stft_out[0][HOP..].fill(0.0);
            WavetableFilter::process_stft_frame(
                &plugin.stft_in[0], plugin.stft_in_pos,
                &mut plugin.stft_out[0], &plugin.stft_magnitudes,
                &plugin.stft_window, &plugin.stft_fft, &plugin.kernel_ifft,
                &mut plugin.stft_scratch, &mut plugin.spectrum_work,
            );
        }

        plugin.stft_in[0][plugin.stft_in_pos] = input[i];
        output[i] = plugin.stft_out[0][plugin.stft_out_pos];

        plugin.stft_in_pos = (plugin.stft_in_pos + 1) & (KERNEL_LEN - 1);
        plugin.stft_out_pos += 1;
        if plugin.stft_out_pos >= HOP { plugin.stft_out_pos = 0; }
    }

    // After transient, output energy should be much less than input energy
    let start = KERNEL_LEN * 2;
    let input_energy: f32 = input[start..].iter().map(|x| x * x).sum();
    let output_energy: f32 = output[start..].iter().map(|x| x * x).sum();
    let attenuation = output_energy / input_energy.max(1e-20);
    assert!(
        attenuation < 0.01,
        "High-freq should be attenuated >99%, got {:.1}% through",
        attenuation * 100.0
    );
}
```

- [ ] **Step 2: Write test — STFT preserves input phase**

```rust
#[test]
fn test_stft_preserves_phase() {
    // With flat magnitude spectrum, the output should have the same
    // phase as the input (no systematic phase rotation).
    let mut plugin = WavetableFilter::default();
    plugin.stft_magnitudes.fill(1.0);

    for buf in &mut plugin.stft_in { buf.fill(0.0); }
    for buf in &mut plugin.stft_out { buf.fill(0.0); }
    plugin.stft_in_pos = 0;
    plugin.stft_out_pos = 0;

    let num_samples = KERNEL_LEN * 6;
    let freq = 1000.0f32;
    let sr = 48000.0f32;
    let input: Vec<f32> = (0..num_samples)
        .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sr).sin())
        .collect();

    let mut output = vec![0.0f32; num_samples];
    for i in 0..num_samples {
        if plugin.stft_out_pos == 0 && i > 0 {
            plugin.stft_out[0].copy_within(HOP..KERNEL_LEN, 0);
            plugin.stft_out[0][HOP..].fill(0.0);
            WavetableFilter::process_stft_frame(
                &plugin.stft_in[0], plugin.stft_in_pos,
                &mut plugin.stft_out[0], &plugin.stft_magnitudes,
                &plugin.stft_window, &plugin.stft_fft, &plugin.kernel_ifft,
                &mut plugin.stft_scratch, &mut plugin.spectrum_work,
            );
        }

        plugin.stft_in[0][plugin.stft_in_pos] = input[i];
        output[i] = plugin.stft_out[0][plugin.stft_out_pos];

        plugin.stft_in_pos = (plugin.stft_in_pos + 1) & (KERNEL_LEN - 1);
        plugin.stft_out_pos += 1;
        if plugin.stft_out_pos >= HOP { plugin.stft_out_pos = 0; }
    }

    // Cross-correlation: peak should be at lag 0 (no phase shift)
    let start = KERNEL_LEN * 3;
    let len = KERNEL_LEN;
    let mut best_lag = 0i32;
    let mut best_corr = f32::NEG_INFINITY;
    for lag in -50i32..50 {
        let mut corr = 0.0f32;
        for j in 0..len {
            let ij = (start as i32 + j as i32) as usize;
            let oj = (start as i32 + j as i32 + lag) as usize;
            if oj < num_samples {
                corr += input[ij] * output[oj];
            }
        }
        if corr > best_corr {
            best_corr = corr;
            best_lag = lag;
        }
    }
    assert!(
        best_lag.abs() <= 2,
        "Phase shift detected: best correlation at lag {best_lag}, expected ~0"
    );
}
```

- [ ] **Step 3: Run all tests**

Run: `cargo test`
Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs
git commit -m "add STFT integration tests for magnitude-only filtering"
```

---

### Task 6: Final cleanup

**Files:**
- Modify: `src/lib.rs` — any dead code, warnings

- [ ] **Step 1: Check for dead code and warnings**

Run: `cargo clippy` and `cargo build --release 2>&1 | grep warning`

- [ ] **Step 2: Fix any warnings**

- [ ] **Step 3: Run full test suite + release build**

```bash
cargo test && cargo clippy && cargo nih-plug bundle wavetable-filter --release
```

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs
git commit -m "clean up warnings after STFT implementation"
```
