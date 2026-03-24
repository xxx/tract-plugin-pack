# tinylimit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a low-latency wideband peak limiter with dual-stage transient/dynamics handling, optional true peak targeting, and CPU-rendered GUI with metering.

**Architecture:** New workspace crate `tinylimit/`. The limiter DSP (`limiter.rs`) is a self-contained module with gain computer, dual-stage envelope, and lookahead backward pass. True peak detection (`true_peak.rs`) is copied from gs-meter. The plugin (`lib.rs`) wires DSP + metering + GUI. The editor (`editor.rs`) follows the softbuffer + tiny-skia pattern with input/output/GR meters.

**Tech Stack:** Rust nightly, nih-plug, tiny-skia-widgets, softbuffer, tiny-skia, fontdue, baseview

**Spec:** `docs/superpowers/specs/2026-03-23-tinylimit-design.md`

---

## File Structure

```
tinylimit/
├── Cargo.toml
├── src/
│   ├── lib.rs          — plugin struct, params, process(), metering atomics
│   ├── main.rs         — standalone entry point
│   ├── limiter.rs      — core DSP: gain computer, envelope, lookahead, dual-stage
│   ├── true_peak.rs    — ITU polyphase FIR (copied from gs-meter/src/meter.rs)
│   ├── editor.rs       — softbuffer GUI with meters and controls
│   └── fonts/DejaVuSans.ttf
```

Also modified:
- `Cargo.toml` (workspace root) — add `tinylimit` to members
- `.github/workflows/build.yml` — add tinylimit bundle + standalone steps

---

### Task 1: Scaffold the crate

**Files:**
- Create: `tinylimit/Cargo.toml`
- Create: `tinylimit/src/lib.rs`
- Create: `tinylimit/src/main.rs`
- Create: `tinylimit/src/limiter.rs` (stub)
- Create: `tinylimit/src/true_peak.rs` (stub)
- Create: `tinylimit/src/editor.rs` (stub)
- Copy: `gs-meter/src/fonts/DejaVuSans.ttf` to `tinylimit/src/fonts/DejaVuSans.ttf`
- Modify: `Cargo.toml` (workspace root)

- [ ] **Step 1: Create Cargo.toml**

Model on `gain-brain/Cargo.toml`. Key differences: name `tinylimit`, description "A low-latency wideband peak limiter", add `nih_plug` with `features = ["simd", "standalone"]` (needs SIMD for true peak FIR). Same softbuffer/tiny-skia/baseview/fontdue/keyboard-types/crossbeam/serde deps. Add `tiny-skia-widgets = { path = "../tiny-skia-widgets" }`. Bundler metadata: name "tinylimit", company "mpd".

- [ ] **Step 2: Create minimal lib.rs**

Bare-minimum pass-through plugin with all 10 parameters defined but not wired to DSP yet. Include `mod editor; pub mod limiter; pub mod true_peak;` declarations. Plugin struct has `params: Arc<TinylimitParams>` and placeholder fields for limiter state and metering atomics. `process()` initially just passes audio through. ClapPlugin ID: `com.mpd.tinylimit`, features: AudioEffect + Mastering. VST3 class ID: `*b"TinylimitMpdPlugin\0"` (16 bytes).

All parameters defined with correct ranges per spec:
- `input`: -60 to +18 dB, default 0, skewed, smoothed
- `threshold`: -60 to 0 dB, default 0
- `ceiling`: -30 to 0 dB, default 0
- `attack`: 0.1 to 10 ms, default 5
- `release`: 1 to 1000 ms, default 200, skewed
- `knee`: 0 to 12 dB, default 0
- `stereo_link`: 0 to 100%, default 100
- `transient`: 0 to 100%, default 50
- `isp`: BoolParam, default false
- `gain_link`: BoolParam, default false

- [ ] **Step 3: Create main.rs, stubs, copy font, add to workspace**

- [ ] **Step 4: Verify compilation**

Run: `cargo check --package tinylimit`

---

### Task 2: Implement gain computer with tests (TDD)

**Files:**
- Create/modify: `tinylimit/src/limiter.rs`

The gain computer is the static characteristic function that maps input level to gain reduction. This is the foundation — everything else builds on it.

- [ ] **Step 1: Write tests first**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Hard knee tests (knee = 0)
    #[test]
    fn test_gain_computer_below_threshold() {
        // Signal at -10 dBFS, threshold at 0 → no reduction
        assert_eq!(gain_computer_db(-10.0, 0.0), 0.0);
    }

    #[test]
    fn test_gain_computer_at_threshold() {
        assert_eq!(gain_computer_db(0.0, 0.0), 0.0);
    }

    #[test]
    fn test_gain_computer_above_threshold() {
        // Signal at +6 dBFS → reduce by 6 dB
        let gr = gain_computer_db(6.0, 0.0);
        assert!((gr - (-6.0)).abs() < 0.01);
    }

    // Soft knee tests
    #[test]
    fn test_soft_knee_below_knee_region() {
        // Well below knee → no reduction
        let gr = gain_computer_db(-20.0, 6.0);
        assert!(gr.abs() < 0.01);
    }

    #[test]
    fn test_soft_knee_in_knee_region() {
        // At threshold (0 dB) with 6 dB knee → partial reduction
        let gr = gain_computer_db(0.0, 6.0);
        assert!(gr < 0.0);
        assert!(gr > -6.0); // not full reduction yet
    }

    #[test]
    fn test_soft_knee_above_knee_region() {
        // Well above knee → full limiting
        let gr = gain_computer_db(10.0, 6.0);
        assert!((gr - (-10.0)).abs() < 0.01);
    }
}
```

- [ ] **Step 2: Implement gain_computer_db**

```rust
/// Compute gain reduction in dB for a given input level.
/// Threshold is 0 dBFS (signal is pre-boosted by threshold param).
/// `knee_db` is the soft knee width (0 = hard knee).
pub fn gain_computer_db(input_db: f32, knee_db: f32) -> f32 {
    if knee_db < 0.01 {
        // Hard knee
        if input_db <= 0.0 { 0.0 } else { -input_db }
    } else {
        let half_knee = knee_db / 2.0;
        if input_db < -half_knee {
            0.0
        } else if input_db <= half_knee {
            -(input_db + half_knee).powi(2) / (2.0 * knee_db)
        } else {
            -input_db
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --package tinylimit`

---

### Task 3: Implement envelope filters with tests (TDD)

**Files:**
- Modify: `tinylimit/src/limiter.rs`

The branching one-pole IIR filter for attack/release smoothing.

- [ ] **Step 1: Write tests**

Test that attack coefficient makes the envelope track downward (more reduction) quickly, and release coefficient makes it recover slowly. Test the dual-stage mixing.

```rust
#[test]
fn test_envelope_attack() {
    let mut env = EnvelopeFilter::new(48000.0, 0.1, 5.0);
    // Feed a sudden -10 dB reduction
    let mut val = 0.0_f32;
    for _ in 0..48 { // ~1ms at 48kHz
        val = env.process(-10.0);
    }
    // After 1ms with 0.1ms attack, should be close to -10
    assert!(val < -5.0);
}

#[test]
fn test_envelope_release() {
    let mut env = EnvelopeFilter::new(48000.0, 0.1, 200.0);
    // Drive to -10 dB
    for _ in 0..480 {
        env.process(-10.0);
    }
    // Now release (input 0 dB)
    let mut val = 0.0;
    for _ in 0..9600 { // 200ms at 48kHz
        val = env.process(0.0);
    }
    // After one time constant, should have recovered ~63%
    assert!(val > -5.0);
}
```

- [ ] **Step 2: Implement EnvelopeFilter**

```rust
pub struct EnvelopeFilter {
    state: f32,
    alpha_attack: f32,
    alpha_release: f32,
}

impl EnvelopeFilter {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self { ... }
    pub fn set_params(&mut self, sample_rate: f32, attack_ms: f32, release_ms: f32) { ... }
    pub fn process(&mut self, gr_db: f32) -> f32 { ... }
    pub fn reset(&mut self) { ... }
}
```

- [ ] **Step 3: Implement DualStageEnvelope**

Wraps two EnvelopeFilters (transient + dynamics) with a mix parameter.

```rust
pub struct DualStageEnvelope {
    transient: EnvelopeFilter,
    dynamics: EnvelopeFilter,
}

impl DualStageEnvelope {
    pub fn new(sample_rate: f32, attack_ms: f32, release_ms: f32) -> Self { ... }
    pub fn process(&mut self, gr_db: f32, transient_mix: f32) -> f32 { ... }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package tinylimit`

---

### Task 4: Implement lookahead backward pass with tests (TDD)

**Files:**
- Modify: `tinylimit/src/limiter.rs`

The backward pass iterates through the gain reduction buffer in reverse, ramping toward each peak over the lookahead window.

- [ ] **Step 1: Write tests**

```rust
#[test]
fn test_lookahead_ramps_before_peak() {
    let lookahead = 240; // 5ms at 48kHz
    let mut gr = vec![0.0_f32; 480];
    // Place a -10 dB peak at sample 300
    gr[300] = -10.0;
    apply_lookahead_backward_pass(&mut gr, lookahead);
    // Gain reduction should start ramping before sample 300
    assert!(gr[300 - lookahead] < 0.0);
    // Should be 0 before the ramp starts
    assert_eq!(gr[0], 0.0);
}

#[test]
fn test_lookahead_deeper_peak_overrides() {
    let lookahead = 240;
    let mut gr = vec![0.0_f32; 480];
    gr[200] = -5.0;
    gr[250] = -10.0; // deeper peak later
    apply_lookahead_backward_pass(&mut gr, lookahead);
    // The -10 ramp should override the -5 ramp where it's deeper
    assert!(gr[200] < -5.0); // overridden by the deeper peak's ramp
}
```

- [ ] **Step 2: Implement apply_lookahead_backward_pass**

```rust
/// Apply lookahead by iterating backwards, linearly ramping (in dB)
/// gain reduction toward each peak over the lookahead window.
pub fn apply_lookahead_backward_pass(gr: &mut [f32], lookahead_samples: usize) { ... }
```

- [ ] **Step 3: Run tests**

---

### Task 5: Copy true peak detector from gs-meter

**Files:**
- Create: `tinylimit/src/true_peak.rs`
- Read: `gs-meter/src/meter.rs` (source)

- [ ] **Step 1: Copy TruePeakDetector**

Extract from `gs-meter/src/meter.rs`: the `ITU_COEFFS` array, `ITU_COEFFS_PADDED`, `dot12_simd`, `TruePeakDetector` struct and impl, and the associated constants (`TRUE_PEAK_TAPS`, `TRUE_PEAK_PHASES`). Place in `tinylimit/src/true_peak.rs`.

Add `#![feature(portable_simd)]` to `tinylimit/src/lib.rs` since the true peak detector uses `std::simd::f32x16`.

- [ ] **Step 2: Add a test that the detector finds inter-sample peaks**

```rust
#[test]
fn test_true_peak_detects_intersample() {
    let mut det = TruePeakDetector::new();
    det.set_sample_rate(48000.0);
    // Two samples that produce an inter-sample peak > either sample
    det.process_sample(0.9);
    det.process_sample(-0.9);
    assert!(det.true_peak_max() > 0.9);
}
```

- [ ] **Step 3: Verify**

Run: `cargo test --package tinylimit`

---

### Task 6: Wire up the limiter in process()

**Files:**
- Modify: `tinylimit/src/lib.rs`
- Modify: `tinylimit/src/limiter.rs`

- [ ] **Step 1: Create the Limiter struct**

A high-level struct in `limiter.rs` that owns all DSP state: delay line, gain reduction buffer, dual-stage envelope, and exposes a `process_block` method.

```rust
pub struct Limiter {
    delay_line: Vec<[f32; 2]>,  // stereo delay ring buffer
    delay_pos: usize,
    gr_buffer: Vec<f32>,        // per-sample gain reduction for the block
    envelope: DualStageEnvelope,
    lookahead_samples: usize,
    sample_rate: f32,
}

impl Limiter {
    pub fn new(sample_rate: f32, max_lookahead_ms: f32) -> Self { ... }
    pub fn set_params(&mut self, attack_ms: f32, release_ms: f32) { ... }
    pub fn set_sample_rate(&mut self, sample_rate: f32) { ... }
    pub fn reset(&mut self) { ... }

    /// Process a stereo block in-place. Returns max gain reduction in dB.
    pub fn process_block(
        &mut self,
        left: &mut [f32],
        right: &mut [f32],
        knee_db: f32,
        transient_mix: f32,
        stereo_link: f32,
        ceiling_linear: f32,
        true_peak_det: Option<&mut [TruePeakDetector; 2]>,
    ) -> f32 { ... }
}
```

- [ ] **Step 2: Wire into lib.rs process()**

The plugin's `process()` method:
1. Apply input gain + threshold boost (smoothed)
2. Call `limiter.process_block()` with current params
3. Apply ceiling gain
4. Update metering atomics (input peak, output peak, GR)
5. If gain_link: ceiling = threshold

- [ ] **Step 3: Add MeterReadings struct**

Shared atomics for the GUI (same pattern as gs-meter):

```rust
pub struct MeterReadings {
    pub input_peak_l: AtomicI32,   // fixed-point dB * 100
    pub input_peak_r: AtomicI32,
    pub output_peak_l: AtomicI32,
    pub output_peak_r: AtomicI32,
    pub gain_reduction: AtomicI32, // current GR in dB * 100
}
```

- [ ] **Step 4: Verify with a simple integration test**

```rust
#[test]
fn test_limiter_output_below_ceiling() {
    let mut limiter = Limiter::new(48000.0, 10.0);
    limiter.set_params(5.0, 200.0);
    let mut left = vec![2.0_f32; 1024];  // +6 dBFS, above ceiling
    let mut right = vec![2.0_f32; 1024];
    limiter.process_block(&mut left, &mut right, 0.0, 0.5, 1.0, 1.0, None);
    for &s in &left[240..] {  // after lookahead settles
        assert!(s.abs() <= 1.001);  // within ~0 dBFS ceiling
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --package tinylimit`

---

### Task 7: Build the editor GUI

**Files:**
- Create/modify: `tinylimit/src/editor.rs`
- Modify: `tinylimit/src/lib.rs` (wire editor)

- [ ] **Step 1: Implement the editor**

Follow the gain-brain/gs-meter editor pattern. Target window: 500 x 500 at 1x.

Layout:
- Title "tinylimit" with +/- scale buttons
- Left: input meter (two vertical bars for L/R)
- Center: 8 dials (Input, Thresh, Ceiling, Attack, Release, Knee, Link%, Transient) + 2 toggle buttons (ISP, Gain Link) + GR readout
- Right: output meter (two vertical bars for L/R)

Dials use `tiny_skia_widgets::draw_dial`. Toggles use `draw_button` with active state. Meters are custom: vertical filled rectangles that read from MeterReadings atomics.

The implementer should use gs-meter's editor as the primary template and adapt it for tinylimit's layout and parameters.

- [ ] **Step 2: Wire editor into lib.rs**

Add `editor_state` to params, pass `readings` and `params` to `editor::create()`, implement `editor()` method on Plugin.

- [ ] **Step 3: Verify**

Run: `cargo build --bin tinylimit`

---

### Task 8: Add to CI, final verification

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Add tinylimit to build.yml**

Add bundle and standalone build steps after gain-brain.

- [ ] **Step 2: Run full workspace tests**

Run: `cargo test --workspace`

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`

- [ ] **Step 4: Build release bundle**

Run: `cargo nih-plug bundle tinylimit --release`

- [ ] **Step 5: Dispatch review agents**

Use `feature-dev:code-reviewer` and `superpowers:code-reviewer` to review all tinylimit code. Focus areas:
- No allocations on audio thread
- Gain computer math matches Giannoulis equations
- Lookahead backward pass doesn't have off-by-one errors
- True peak detector handles sample rate correctly
- Stereo link doesn't allow channel to exceed ceiling
- Safety clip catches all residual overshoots
- Metering atomics are read/written correctly
- All parameters smoothed where needed (input, threshold must not click)
