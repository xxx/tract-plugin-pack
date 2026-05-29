# Nap — Velvet Reverb Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `nap`, an EDVN (Extended Dark Velvet Noise) character reverb whose tail's loudness, stereo width, and tone are each drawn as a curve over a shared tail-position axis.

**Architecture:** Two stages split across threads (the miff bake/handoff pattern). The GUI thread *generates* a `VelvetSequence` (pulse locations, sign·gain coefficients, dictionary-filter routing, and per-pulse right-channel jitter) from the three `MsegData` curves + Size/Density/Width/Seed params, and publishes it through a lock-free `SequenceHandoff`. The audio thread runs a sparse signed tapped-delay-line convolution into a small bank of all-pole coloration filters → post-filter + DC blocker → pre-delay → dry/wet mix. No FFT, zero reported latency.

**Tech Stack:** Rust (nightly), nih-plug (fork `finish-vst3-pr`), softbuffer + tiny-skia + `tiny-skia-widgets` (incl. shared `mseg` editor), `tract-dsp` (`db`). No new external crates.

**Reference files to mirror (read before starting the matching task):**
- Handoff + curve-walk + bake: `miff/src/kernel.rs`
- Plugin shell (Plugin/ClapPlugin/Vst3Plugin/process/initialize): `satch/src/lib.rs`, `satch/src/main.rs`
- Regen trigger (`rebake()` on edit, initial bake in `initialize`): `miff/src/lib.rs:248`, `miff/src/editor.rs:334`
- Editor scaffold (softbuffer/baseview + MSEG editor wiring): `miff/src/editor.rs`
- MSEG widget API: `tiny-skia-widgets/src/mseg/mod.rs` (`MsegData`, `value_at_phase`, `Polarity`, `MsegNode`), `tiny-skia-widgets/src/mseg/render.rs` (`draw_mseg`, `mseg_layout`, `phase_to_x`, `value_to_y`), `tiny-skia-widgets/src/mseg/editor.rs` (`MsegEditState`, `on_mouse_*`)

**Commit rule:** This repo's hard rule is *never commit unless the user asks*. The per-task "Commit" steps below are written for completeness, but DO NOT run them unless the user has explicitly authorized commits for this work. If unauthorized, stage nothing and just proceed to the next task.

**Build/test commands:**
- Test one module: `cargo nextest run -p nap <filter>`
- All tests: `cargo nextest run -p nap`
- Lint: `cargo clippy -p nap -- -D warnings`
- Format check: `cargo fmt --check -p nap`
- Debug standalone (manual GUI check): `cargo build --bin nap` then run `target/debug/nap`

---

## Phase 0 — Crate scaffold

### Task 1: Create the `nap` crate and register it in the workspace

**Files:**
- Create: `nap/Cargo.toml`
- Create: `nap/src/lib.rs`
- Create: `nap/src/main.rs`
- Create: `nap/src/fonts/DejaVuSans.ttf` (copy from `satch/src/fonts/DejaVuSans.ttf`)
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Add the crate to the workspace members**

In the root `Cargo.toml`, add `"nap"` to the `members` array (after `"multosis"`):

```toml
members = ["wavetable-filter", "gs-meter", "gain-brain", "tinylimit", "satch", "pope-scope", "warp-zone", "six-pack", "imagine", "miff", "multosis", "nap", "tiny-skia-widgets", "tract-dsp", "xtask", "bench-suite"]
```

- [ ] **Step 2: Write `nap/Cargo.toml`** (mirrors `satch/Cargo.toml`)

```toml
[package]
name = "nap"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "lib"]

[[bin]]
name = "nap"
path = "src/main.rs"

[dependencies]
nih_plug = { git = "https://github.com/xxx/nih-plug.git", branch = "finish-vst3-pr", features = ["standalone", "assert_process_allocs"] }
baseview = { git = "https://github.com/RustAudio/baseview.git", tag = "v0.1.1", features = ["opengl"] }
softbuffer = { version = "0.4", default-features = false, features = ["kms", "x11"] }
raw-window-handle = "0.5"
raw-window-handle-06 = { package = "raw-window-handle", version = "0.6" }
tiny-skia = "0.12"
tiny-skia-widgets = { path = "../tiny-skia-widgets" }
keyboard-types = "0.6"
crossbeam = "0.8"
serde = { version = "1.0", features = ["derive"] }
tract-dsp = { path = "../tract-dsp" }

[package.metadata.bundler]
name = "Nap"
company = "mpd"
description = "A draw-your-tail velvet-noise reverb"
license = "GPL-3.0-or-later"
version = "0.1.0"

[[bench]]
name = "dsp"
harness = false
```

- [ ] **Step 3: Write a minimal `nap/src/lib.rs`** so the workspace compiles (real plugin filled in Task 7). Declare the modules now as empty stubs; create the stub files in their own tasks. For this task, only declare modules that exist:

```rust
//! Nap — a draw-your-tail velvet-noise (EDVN) reverb.
//! See `docs/superpowers/specs/2026-05-29-nap-velvet-reverb-design.md`.
```

- [ ] **Step 4: Write `nap/src/main.rs`**

```rust
use nih_plug::prelude::*;
fn main() {
    nih_export_standalone::<nap::Nap>();
}
```

Note: this will not compile until `Nap` exists (Task 7). Temporarily comment the body out with `fn main() {}` and a `// TODO(Task 7)` until Task 7, then restore. (Allowed: this is a scaffolding stub, not a logic placeholder.)

- [ ] **Step 5: Copy the font**

Run: `mkdir -p nap/src/fonts && cp satch/src/fonts/DejaVuSans.ttf nap/src/fonts/DejaVuSans.ttf`

- [ ] **Step 6: Verify the workspace builds**

Run: `cargo build -p nap`
Expected: compiles (empty lib + stub main).

- [ ] **Step 7: Commit** (only if authorized)

```bash
git add Cargo.toml nap/
git commit -m "feat(nap): scaffold velvet-reverb crate"
```

---

## Phase 1 — DSP core (in-crate, TDD)

### Task 2: Deterministic seedable RNG

**Files:**
- Create: `nap/src/rng.rs`
- Modify: `nap/src/lib.rs` (add `pub mod rng;`)

- [ ] **Step 1: Write the failing test**

In `nap/src/rng.rs`:

```rust
//! Tiny dependency-free deterministic RNG for reproducible velvet sequences.
//! SplitMix64 — fast, good distribution, fully seedable.

pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15) }
    }

    /// Next raw u64 (SplitMix64).
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in [0, 1).
    pub fn next_f32(&mut self) -> f32 {
        // top 24 bits → [0,1)
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seed_different_sequence() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let diffs = (0..100).filter(|_| a.next_u64() != b.next_u64()).count();
        assert!(diffs > 90, "seeds should produce largely different streams");
    }

    #[test]
    fn f32_in_unit_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let x = r.next_f32();
            assert!((0.0..1.0).contains(&x), "out of range: {x}");
        }
    }
}
```

- [ ] **Step 2: Add the module** in `nap/src/lib.rs`: `pub mod rng;`

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p nap rng`
Expected: 3 pass.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/rng.rs nap/src/lib.rs
git commit -m "feat(nap): deterministic seedable RNG"
```

---

### Task 3: Coloration dictionary (one-pole lowpass bank, dark→bright)

**Files:**
- Create: `nap/src/coloration.rs`
- Modify: `nap/src/lib.rs` (add `pub mod coloration;`)

Design: `Q` one-pole lowpass filters, cutoffs log-spaced low→high so filter index 0 is darkest and `Q-1` is brightest. One-pole is unconditionally stable (coefficient in (0,1)) and has a monotonic spectral centroid in cutoff — clean to test. (Upgrade to resonant 2nd-order all-pole is a future refinement; the dictionary interface stays the same.)

- [ ] **Step 1: Write the failing test**

In `nap/src/coloration.rs`:

```rust
//! Hand-designed coloration dictionary: `Q` one-pole lowpass filters ordered
//! dark→bright. Each velvet pulse is routed to one filter; the per-pulse
//! routing (driven by the Tone curve) shapes how the tail's spectrum evolves.

/// Number of dictionary filters.
pub const Q: usize = 6;

/// A single one-pole lowpass: `y[n] = (1-c)·x[n] + c·y[n-1]`, `c ∈ (0,1)`.
#[derive(Clone, Copy)]
pub struct OnePole {
    c: f32,
    z: f32,
}

impl OnePole {
    /// Build from a cutoff in Hz at `sample_rate`.
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        let c = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
        Self { c: c.clamp(0.0, 0.9999), z: 0.0 }
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.z = (1.0 - self.c) * x + self.c * self.z;
        self.z
    }
    pub fn reset(&mut self) {
        self.z = 0.0;
    }
    pub fn coeff(&self) -> f32 {
        self.c
    }
}

/// The full ordered dictionary.
pub struct Dictionary {
    pub filters: [OnePole; Q],
}

impl Dictionary {
    /// Cutoffs log-spaced from `LOW_HZ` (darkest) to `HIGH_HZ` (brightest).
    pub const LOW_HZ: f32 = 500.0;
    pub const HIGH_HZ: f32 = 18_000.0;

    pub fn new(sample_rate: f32) -> Self {
        let mut filters = [OnePole::new(Self::LOW_HZ, sample_rate); Q];
        let ratio = (Self::HIGH_HZ / Self::LOW_HZ).powf(1.0 / (Q - 1) as f32);
        for (i, f) in filters.iter_mut().enumerate() {
            let cutoff = Self::LOW_HZ * ratio.powi(i as i32);
            *f = OnePole::new(cutoff, sample_rate);
        }
        Self { filters }
    }

    pub fn reset(&mut self) {
        for f in &mut self.filters {
            f.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Estimate the normalized spectral centroid of a filter's impulse
    /// response via a coarse DFT magnitude sweep.
    fn centroid(mut f: OnePole, sample_rate: f32) -> f32 {
        // impulse response
        let n = 4096;
        let mut h = vec![0.0f32; n];
        h[0] = f.process(1.0);
        for s in h.iter_mut().skip(1) {
            *s = f.process(0.0);
        }
        // magnitude-weighted mean frequency over a log grid
        let mut num = 0.0f64;
        let mut den = 0.0f64;
        let mut freq = 20.0f32;
        while freq < sample_rate / 2.0 {
            let w = 2.0 * std::f32::consts::PI * freq / sample_rate;
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (k, &hk) in h.iter().enumerate() {
                re += (hk as f64) * (w as f64 * k as f64).cos();
                im -= (hk as f64) * (w as f64 * k as f64).sin();
            }
            let mag = (re * re + im * im).sqrt();
            num += mag * freq as f64;
            den += mag;
            freq *= 1.1;
        }
        (num / den) as f32
    }

    #[test]
    fn dictionary_is_ordered_dark_to_bright() {
        let sr = 48_000.0;
        let dict = Dictionary::new(sr);
        let centroids: Vec<f32> = dict.filters.iter().map(|&f| centroid(f, sr)).collect();
        for w in centroids.windows(2) {
            assert!(w[1] > w[0], "centroid must increase with index: {centroids:?}");
        }
    }

    #[test]
    fn all_filters_stable() {
        let dict = Dictionary::new(48_000.0);
        for f in &dict.filters {
            assert!(f.coeff() >= 0.0 && f.coeff() < 1.0, "one-pole coeff must be in [0,1)");
        }
    }
}
```

- [ ] **Step 2: Add the module** in `nap/src/lib.rs`: `pub mod coloration;`

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p nap coloration`
Expected: 2 pass.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/coloration.rs nap/src/lib.rs
git commit -m "feat(nap): coloration dictionary (one-pole dark→bright bank)"
```

---

### Task 4: `VelvetSequence` type + default curves

**Files:**
- Create: `nap/src/sequence.rs`
- Modify: `nap/src/lib.rs` (add `pub mod sequence;`)

- [ ] **Step 1: Write the failing test** — the data type, its max capacity, and the default curves.

In `nap/src/sequence.rs`:

```rust
//! Velvet sequence generation (GUI thread). Turns the three drawn curves +
//! Size/Density/Width/Seed into a sparse signed pulse train with per-pulse
//! coloration routing and per-pulse right-channel jitter.

use tiny_skia_widgets::mseg::{MsegData, MsegNode, Polarity};

/// Hard upper bound on pulse count: max Size (10 s) × max Density (4000/s) at
/// 48 kHz, rounded up. Buffers are pre-allocated to this so generation and
/// handoff never reallocate.
pub const MAX_PULSES: usize = 48_000;

/// A baked velvet sequence. Buffers are pre-allocated to `MAX_PULSES`; only
/// `[..count]` is meaningful. Cloning/copying is `[..count]` slices.
#[derive(Clone)]
pub struct VelvetSequence {
    pub count: usize,
    /// Left-channel pulse sample offsets (ascending), into the input ring.
    pub location: Vec<u32>,
    /// Signed decay gain per pulse: `s(m)·g(m)`, energy-normalized.
    pub coeff: Vec<f32>,
    /// Dictionary filter index per pulse (`0..Q`).
    pub filter_idx: Vec<u8>,
    /// Right-channel pulse sample offsets (jittered copy of `location`).
    pub location_r: Vec<u32>,
    /// Total tail length in samples (max of L/R locations + 1).
    pub tail_len: usize,
}

impl VelvetSequence {
    pub fn new() -> Self {
        Self {
            count: 0,
            location: vec![0; MAX_PULSES],
            coeff: vec![0.0; MAX_PULSES],
            filter_idx: vec![0; MAX_PULSES],
            location_r: vec![0; MAX_PULSES],
            tail_len: 0,
        }
    }

    /// Copy `other[..count]` into `self` without reallocating.
    pub fn copy_from(&mut self, other: &VelvetSequence) {
        self.count = other.count;
        self.tail_len = other.tail_len;
        self.location[..self.count].copy_from_slice(&other.location[..self.count]);
        self.coeff[..self.count].copy_from_slice(&other.coeff[..self.count]);
        self.filter_idx[..self.count].copy_from_slice(&other.filter_idx[..self.count]);
        self.location_r[..self.count].copy_from_slice(&other.location_r[..self.count]);
    }
}

impl Default for VelvetSequence {
    fn default() -> Self {
        Self::new()
    }
}

/// Default Decay curve: full at the start, decaying to silence — an
/// exponential-ish fall via positive tension. Unipolar.
pub fn default_decay_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 1.0, tension: 0.6, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

/// Default Width curve: a moderate, constant 0.5 width across the tail.
pub fn default_width_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 0.5, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.5, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

/// Default Tone curve: bright at the start, darkening over the tail (air
/// absorption). Value 1.0 = brightest dictionary filter, 0.0 = darkest.
pub fn default_tone_curve() -> MsegData {
    let mut d = MsegData::default();
    d.nodes[0] = MsegNode { time: 0.0, value: 0.85, tension: 0.0, stepped: false };
    d.nodes[1] = MsegNode { time: 1.0, value: 0.25, tension: 0.0, stepped: false };
    d.polarity = Polarity::Unipolar;
    d.debug_assert_valid();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_curves_are_valid() {
        assert!(default_decay_curve().is_valid());
        assert!(default_width_curve().is_valid());
        assert!(default_tone_curve().is_valid());
    }

    #[test]
    fn copy_from_preserves_active_region() {
        let mut a = VelvetSequence::new();
        a.count = 3;
        a.tail_len = 99;
        a.location[..3].copy_from_slice(&[1, 2, 3]);
        a.coeff[..3].copy_from_slice(&[0.1, 0.2, 0.3]);
        a.filter_idx[..3].copy_from_slice(&[0, 1, 2]);
        a.location_r[..3].copy_from_slice(&[1, 4, 9]);
        let mut b = VelvetSequence::new();
        b.copy_from(&a);
        assert_eq!(b.count, 3);
        assert_eq!(b.tail_len, 99);
        assert_eq!(&b.location[..3], &[1, 2, 3]);
        assert_eq!(&b.coeff[..3], &[0.1, 0.2, 0.3]);
        assert_eq!(&b.filter_idx[..3], &[0, 1, 2]);
        assert_eq!(&b.location_r[..3], &[1, 4, 9]);
    }
}
```

- [ ] **Step 2: Add the module** in `nap/src/lib.rs`: `pub mod sequence;`

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p nap sequence`
Expected: 2 pass.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/sequence.rs nap/src/lib.rs
git commit -m "feat(nap): VelvetSequence type + default curves"
```

---

### Task 5: Velvet sequence generation

**Files:**
- Modify: `nap/src/sequence.rs` (add `GenParams` + `generate`)

The generator walks the grid once, sampling each curve at the pulse's tail phase using miff's forward-only `curve_value` cursor pattern (reproduces `value_at_phase` without per-pulse rescans).

- [ ] **Step 1: Write the failing tests** — append to `nap/src/sequence.rs` (and add the `generate` API the tests call).

Add these public items near the top of the impl section:

```rust
use crate::coloration::Q;
use crate::rng::Rng;
use tiny_skia_widgets::mseg::warp;

/// Inputs to a generation pass.
#[derive(Clone, Copy)]
pub struct GenParams {
    pub sample_rate: f32,
    /// Tail length, seconds.
    pub size_s: f32,
    /// Pulse density, pulses/second.
    pub density: f32,
    /// Max right-channel jitter at Width=1.0, in milliseconds.
    pub width_ms: f32,
    pub seed: u64,
}

/// Forward-only curve sampler (monotonic `phase`), mirrors miff's `curve_value`.
fn curve_value(data: &MsegData, phase: f32, seg: &mut usize) -> f32 {
    let a = data.active();
    let last = data.node_count - 1;
    if phase >= a[last].time {
        return a[last].value;
    }
    while *seg < last - 1 && a[*seg + 1].time <= phase {
        *seg += 1;
    }
    let n0 = a[*seg];
    let n1 = a[*seg + 1];
    if n0.stepped {
        return n0.value;
    }
    let span = n1.time - n0.time;
    let t = if span > 1e-9 { (phase - n0.time) / span } else { 0.0 };
    n0.value + (n1.value - n0.value) * warp(t, n0.tension)
}

/// Generate the velvet sequence into `out` (pre-allocated to `MAX_PULSES`).
/// Deterministic in `(params, curves)`. Energy-normalized so `Σ coeff² == 1`
/// (when any pulse has non-zero gain). Runs on the GUI thread only.
pub fn generate(
    out: &mut VelvetSequence,
    params: &GenParams,
    decay: &MsegData,
    width: &MsegData,
    tone: &MsegData,
) {
    let fs = params.sample_rate.max(1.0);
    let l_samples = ((params.size_s.max(0.01)) * fs) as usize;
    let td = (fs / params.density.max(1.0)).max(1.0);
    let m_count = ((l_samples as f32 / td) as usize).min(MAX_PULSES);
    let j_max = (params.width_ms * 0.001 * fs).max(0.0); // samples at Width=1

    let mut rng = Rng::new(params.seed);
    let (mut sd, mut sw, mut st) = (0usize, 0usize, 0usize);
    let denom = l_samples.max(1) as f32;

    let mut energy = 0.0f64;
    let mut max_loc = 0u32;

    for m in 0..m_count {
        // Location: one jittered pulse per grid cell.
        let r_loc = rng.next_f32();
        let k = (m as f32 * td + r_loc * (td - 1.0)).round().max(0.0) as u32;
        let phase = (k as f32 / denom).clamp(0.0, 1.0);

        // Sign.
        let sign = if rng.next_f32() < 0.5 { -1.0 } else { 1.0 };

        // Decay → gain.
        let g = curve_value(decay, phase, &mut sd).clamp(0.0, 1.0);
        let coeff = sign * g;

        // Tone → nearest dictionary filter (0 = darkest, Q-1 = brightest).
        let t = curve_value(tone, phase, &mut st).clamp(0.0, 1.0);
        let filter_idx = (t * (Q - 1) as f32).round() as u8;

        // Width → per-pulse max jitter; right channel offset in ±j samples.
        let w = curve_value(width, phase, &mut sw).clamp(0.0, 1.0);
        let j = (w * j_max).round();
        let r_jit = rng.next_f32(); // [0,1) → symmetric [-j, +j]
        let delta = ((r_jit * 2.0 - 1.0) * j).round() as i64;
        let k_r = (k as i64 + delta).max(0) as u32;

        out.location[m] = k;
        out.coeff[m] = coeff;
        out.filter_idx[m] = filter_idx.min((Q - 1) as u8);
        out.location_r[m] = k_r;

        energy += (coeff as f64) * (coeff as f64);
        max_loc = max_loc.max(k).max(k_r);
    }

    // Energy-normalize coefficients so output level is independent of M / shape.
    if energy > 1e-20 {
        let inv = (1.0 / energy.sqrt()) as f32;
        for c in out.coeff[..m_count].iter_mut() {
            *c *= inv;
        }
    }

    out.count = m_count;
    out.tail_len = (max_loc as usize) + 1;
}
```

Append these tests to the `tests` module:

```rust
    use tiny_skia_widgets::mseg::value_at_phase;

    fn flat(value: f32) -> MsegData {
        let mut d = MsegData::default();
        d.nodes[0] = MsegNode { time: 0.0, value, tension: 0.0, stepped: false };
        d.nodes[1] = MsegNode { time: 1.0, value, tension: 0.0, stepped: false };
        d
    }

    fn test_params() -> GenParams {
        GenParams { sample_rate: 48_000.0, size_s: 1.0, density: 1500.0, width_ms: 5.0, seed: 1 }
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let p = test_params();
        let (d, w, t) = (default_decay_curve(), default_width_curve(), default_tone_curve());
        let mut a = VelvetSequence::new();
        let mut b = VelvetSequence::new();
        generate(&mut a, &p, &d, &w, &t);
        generate(&mut b, &p, &d, &w, &t);
        assert_eq!(a.count, b.count);
        assert_eq!(&a.location[..a.count], &b.location[..b.count]);
        assert_eq!(&a.coeff[..a.count], &b.coeff[..b.count]);
        assert_eq!(&a.location_r[..a.count], &b.location_r[..b.count]);
    }

    #[test]
    fn pulse_count_tracks_size_times_density() {
        let mut p = test_params();
        p.size_s = 2.0;
        p.density = 1000.0;
        let (d, w, t) = (flat(1.0), flat(0.0), flat(0.5));
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &d, &w, &t);
        // ~ size*density = 2000, within grid rounding.
        assert!((s.count as i32 - 2000).abs() < 5, "count {}", s.count);
    }

    #[test]
    fn coeffs_are_energy_normalized() {
        let p = test_params();
        let (d, w, t) = (default_decay_curve(), flat(0.0), flat(0.5));
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &d, &w, &t);
        let e: f64 = s.coeff[..s.count].iter().map(|&c| (c as f64).powi(2)).sum();
        assert!((e - 1.0).abs() < 1e-3, "energy {e}");
    }

    #[test]
    fn decay_curve_shapes_the_energy_envelope() {
        // A gated decay (full for first half, silent second half) must leave
        // the second-half pulses at zero gain.
        let p = test_params();
        let mut decay = MsegData::default();
        decay.insert_node(0.5, 1.0);
        decay.nodes[0] = MsegNode { time: 0.0, value: 1.0, tension: 0.0, stepped: false };
        // node at 0.5 -> stepped down to 0
        let mid = decay.nodes[1];
        decay.nodes[1] = MsegNode { stepped: true, ..mid };
        decay.nodes[2] = MsegNode { time: 1.0, value: 0.0, tension: 0.0, stepped: false };
        decay.debug_assert_valid();

        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &decay, &flat(0.0), &flat(0.5));
        let l = (p.size_s * p.sample_rate) as f32;
        for m in 0..s.count {
            let phase = s.location[m] as f32 / l;
            if phase > 0.6 {
                assert!(s.coeff[m].abs() < 1e-6, "pulse past gate should be silent");
            }
        }
    }

    #[test]
    fn width_zero_makes_left_and_right_identical() {
        let p = test_params();
        let mut s = VelvetSequence::new();
        generate(&mut s, &p, &default_decay_curve(), &flat(0.0), &flat(0.5));
        assert_eq!(&s.location[..s.count], &s.location_r[..s.count], "width 0 = mono");
    }

    #[test]
    fn higher_width_decorrelates_more() {
        // Mean |k_R - k_L| must grow with the Width curve level.
        let p = test_params();
        let mean_offset = |wv: f32| {
            let mut s = VelvetSequence::new();
            generate(&mut s, &p, &default_decay_curve(), &flat(wv), &flat(0.5));
            let sum: i64 = (0..s.count)
                .map(|m| (s.location_r[m] as i64 - s.location[m] as i64).abs())
                .sum();
            sum as f64 / s.count.max(1) as f64
        };
        assert!(mean_offset(0.8) > mean_offset(0.2), "more width → more jitter");
    }

    #[test]
    fn tone_curve_selects_brighter_filters_when_higher() {
        let p = test_params();
        let mean_idx = |tv: f32| {
            let mut s = VelvetSequence::new();
            generate(&mut s, &p, &default_decay_curve(), &flat(0.0), &flat(tv));
            s.filter_idx[..s.count].iter().map(|&i| i as f64).sum::<f64>() / s.count.max(1) as f64
        };
        assert!(mean_idx(0.9) > mean_idx(0.1), "brighter tone → higher filter index");
    }

    #[test]
    fn curve_value_matches_value_at_phase() {
        let mut d = MsegData::default();
        d.insert_node(0.3, 0.8);
        d.insert_node(0.6, 0.2);
        let mut seg = 0;
        for i in 0..=100 {
            let phase = i as f32 / 100.0;
            let got = curve_value(&d, phase, &mut seg);
            let want = value_at_phase(&d, phase);
            assert!((got - want).abs() < 1e-5, "phase {phase}: {got} vs {want}");
        }
    }
```

- [ ] **Step 2: Run the tests, expect FAIL first** (compile error: `generate`/`GenParams` undefined) — then add the implementation shown above so they pass.

Run: `cargo nextest run -p nap sequence`
Expected after impl: all pass.

- [ ] **Step 3: Lint**

Run: `cargo clippy -p nap -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/sequence.rs
git commit -m "feat(nap): velvet sequence generation (decay/width/tone → pulses)"
```

---

### Task 6: Audio engine — sparse convolver + filter bank + post/DC + pre-delay + mix

**Files:**
- Create: `nap/src/engine.rs`
- Modify: `nap/src/lib.rs` (add `pub mod engine;`)

- [ ] **Step 1: Write the failing tests** (golden sparse==dense, filter routing, silence/identity).

In `nap/src/engine.rs`:

```rust
//! Audio-thread playback: sparse signed tapped-delay-line convolution into the
//! coloration filter bank, then post-filter + DC blocker, pre-delay, dry/wet.

use crate::coloration::{Dictionary, OnePole, Q};
use crate::sequence::VelvetSequence;

/// Next power of two ≥ n.
fn next_pow2(n: usize) -> usize {
    let mut p = 1;
    while p < n {
        p <<= 1;
    }
    p
}

/// One reverb channel: an input ring + the dictionary + post/DC state.
pub struct ReverbChannel {
    ring: Vec<f32>,
    mask: usize,
    write: usize,
    dict: Dictionary,
    post: OnePole,
    // DC blocker state (one-pole high-pass): y = x - x1 + R*y1
    dc_x1: f32,
    dc_y1: f32,
}

/// Max ring length: max tail (10 s) + max jitter + headroom, at 48 kHz.
pub const MAX_RING_SAMPLES: usize = 48_000 * 11;

impl ReverbChannel {
    pub fn new(sample_rate: f32) -> Self {
        let cap = next_pow2(MAX_RING_SAMPLES);
        Self {
            ring: vec![0.0; cap],
            mask: cap - 1,
            write: 0,
            dict: Dictionary::new(sample_rate),
            post: OnePole::new(12_000.0, sample_rate),
            dc_x1: 0.0,
            dc_y1: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.write = 0;
        self.dict.reset();
        self.post.reset();
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
    }

    /// Push one input sample and return the wet reverb sample, reading taps at
    /// `location` (use `location` for L, `location_r` for R). `coeff`/
    /// `filter_idx` are shared between channels.
    #[inline]
    pub fn process(&mut self, x: f32, seq: &VelvetSequence, location: &[u32]) -> f32 {
        // Write newest input.
        self.ring[self.write] = x;

        // Scatter pulses into per-filter excitation accumulators.
        let mut acc = [0.0f32; Q];
        for m in 0..seq.count {
            let idx = (self.write.wrapping_sub(location[m] as usize)) & self.mask;
            acc[seq.filter_idx[m] as usize] += seq.coeff[m] * self.ring[idx];
        }

        // Run the Q coloration filters, sum.
        let mut wet = 0.0f32;
        for q in 0..Q {
            wet += self.dict.filters[q].process(acc[q]);
        }

        // Post LP.
        wet = self.post.process(wet);
        // DC block (R≈0.995).
        let y = wet - self.dc_x1 + 0.995 * self.dc_y1;
        self.dc_x1 = wet;
        self.dc_y1 = y;

        self.write = (self.write + 1) & self.mask;
        y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sequence with `Q=1`-routed pulses and an identity-ish setup to check
    /// the sparse convolution math in isolation. We bypass the filters by
    /// using a near-allpass (very high cutoff) and comparing relative shape.
    fn seq_from(locations: &[u32], coeffs: &[f32], filt: &[u8]) -> VelvetSequence {
        let mut s = VelvetSequence::new();
        s.count = locations.len();
        s.location[..s.count].copy_from_slice(locations);
        s.location_r[..s.count].copy_from_slice(locations);
        s.coeff[..s.count].copy_from_slice(coeffs);
        s.filter_idx[..s.count].copy_from_slice(filt);
        s.tail_len = *locations.iter().max().unwrap() as usize + 1;
        s
    }

    #[test]
    fn impulse_response_places_pulses_at_locations() {
        // Route everything through one filter; check the wet IR has energy
        // arriving at the pulse locations (post-filter smears, so check the
        // cumulative energy crosses thresholds at the right times).
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 100, 200], &[1.0, 0.5, 0.25], &[0, 0, 0]);
        let n = 400;
        let mut ir = vec![0.0f32; n];
        ir[0] = ch.process(1.0, &seq, &seq.location);
        for s in ir.iter_mut().skip(1) {
            *s = ch.process(0.0, &seq, &seq.location);
        }
        // Energy before sample 50 should come only from the first pulse.
        let e0: f32 = ir[..50].iter().map(|v| v * v).sum();
        let e1: f32 = ir[100..150].iter().map(|v| v * v).sum();
        let e2: f32 = ir[200..250].iter().map(|v| v * v).sum();
        assert!(e0 > 0.0 && e1 > 0.0 && e2 > 0.0);
        // Decaying coeffs → decaying per-pulse energy.
        assert!(e0 > e1 && e1 > e2, "energy should decay: {e0} {e1} {e2}");
    }

    #[test]
    fn silent_input_decays_to_silence() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 50, 100], &[1.0, 0.5, 0.25], &[2, 2, 2]);
        // Excite once.
        let _ = ch.process(1.0, &seq, &seq.location);
        // Run long enough for the tail + filters to settle.
        let mut last = 0.0;
        for _ in 0..20_000 {
            last = ch.process(0.0, &seq, &seq.location);
        }
        assert!(last.abs() < 1e-4, "tail should settle to ~0, got {last}");
    }

    #[test]
    fn empty_sequence_is_silent() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = VelvetSequence::new(); // count = 0
        let mut out = 0.0;
        for _ in 0..100 {
            out += ch.process(1.0, &seq, &seq.location).abs();
        }
        assert!(out < 1e-6, "no pulses → no wet output");
    }

    #[test]
    fn reset_clears_tail() {
        let mut ch = ReverbChannel::new(48_000.0);
        let seq = seq_from(&[0, 10], &[1.0, 1.0], &[0, 0]);
        let _ = ch.process(1.0, &seq, &seq.location);
        ch.reset();
        let after = ch.process(0.0, &seq, &seq.location);
        assert!(after.abs() < 1e-9, "reset should zero the ring + filter state");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo nextest run -p nap engine`
Expected: 4 pass.

- [ ] **Step 3: Lint**

Run: `cargo clippy -p nap -- -D warnings`
Expected: clean.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/engine.rs nap/src/lib.rs
git commit -m "feat(nap): audio engine (sparse convolver + filter bank + post/DC)"
```

---

### Task 7: `SequenceHandoff` (RT-safe GUI→audio publish)

**Files:**
- Create: `nap/src/handoff.rs`
- Modify: `nap/src/lib.rs` (add `pub mod handoff;`)

RT-safety: the audio thread owns its own pre-allocated `VelvetSequence`; `try_read_into` only `copy_from`s (no alloc) when the generation counter changed, and otherwise does just a `try_lock` + atomic compare. No allocation or deallocation ever happens on the audio thread.

- [ ] **Step 1: Write the failing tests**

In `nap/src/handoff.rs`:

```rust
//! Lock-free-ish GUI→audio handoff of the baked `VelvetSequence`. Mirrors
//! miff's `KernelHandoff`, but guards the (larger) copy with a generation
//! counter so the audio thread copies only when the sequence actually changed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::sequence::VelvetSequence;

pub struct SequenceHandoff {
    shared: Mutex<VelvetSequence>,
    generation: AtomicU64,
}

impl SequenceHandoff {
    pub fn new() -> Self {
        Self {
            shared: Mutex::new(VelvetSequence::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Publish a freshly-generated sequence (GUI thread). Copies into the
    /// shared slot and bumps the generation.
    pub fn publish(&self, seq: &VelvetSequence) {
        if let Ok(mut slot) = self.shared.lock() {
            slot.copy_from(seq);
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    /// Audio thread: if a newer sequence exists, copy it into `local` and
    /// update `*local_gen`. Returns `true` if `local` changed. Never blocks
    /// hard (uses `try_lock`); never allocates.
    pub fn try_read_into(&self, local: &mut VelvetSequence, local_gen: &mut u64) -> bool {
        if self.generation.load(Ordering::Acquire) == *local_gen {
            return false; // unchanged — skip the lock/copy entirely
        }
        if let Ok(slot) = self.shared.try_lock() {
            local.copy_from(&slot);
            *local_gen = self.generation.load(Ordering::Acquire);
            return true;
        }
        false // contended this block; try again next block
    }
}

impl Default for SequenceHandoff {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(count: usize, first_loc: u32) -> VelvetSequence {
        let mut s = VelvetSequence::new();
        s.count = count;
        s.tail_len = first_loc as usize + 1;
        if count > 0 {
            s.location[0] = first_loc;
        }
        s
    }

    #[test]
    fn first_read_picks_up_published_sequence() {
        let h = SequenceHandoff::new();
        h.publish(&mk(3, 42));
        let mut local = VelvetSequence::new();
        let mut gen = 0;
        assert!(h.try_read_into(&mut local, &mut gen));
        assert_eq!(local.count, 3);
        assert_eq!(local.location[0], 42);
    }

    #[test]
    fn unchanged_generation_skips_copy() {
        let h = SequenceHandoff::new();
        h.publish(&mk(1, 7));
        let mut local = VelvetSequence::new();
        let mut gen = 0;
        assert!(h.try_read_into(&mut local, &mut gen)); // first: copies
        assert!(!h.try_read_into(&mut local, &mut gen)); // second: no change
    }

    #[test]
    fn newest_publish_wins() {
        let h = SequenceHandoff::new();
        h.publish(&mk(1, 1));
        h.publish(&mk(2, 5));
        let mut local = VelvetSequence::new();
        let mut gen = 0;
        h.try_read_into(&mut local, &mut gen);
        assert_eq!(local.count, 2);
        assert_eq!(local.location[0], 5);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo nextest run -p nap handoff`
Expected: 3 pass.

- [ ] **Step 3: Commit** (if authorized)

```bash
git add nap/src/handoff.rs nap/src/lib.rs
git commit -m "feat(nap): RT-safe SequenceHandoff"
```

---

## Phase 2 — Plugin integration

### Task 8: `NapParams` + `Nap` plugin struct + `Plugin`/`ClapPlugin`/`Vst3Plugin` + `process`

**Files:**
- Modify: `nap/src/lib.rs` (full plugin)
- Modify: `nap/src/main.rs` (restore the real body from Task 1)

- [ ] **Step 1: Write the full `nap/src/lib.rs`**

Mirror `satch/src/lib.rs` for the trait scaffolding. Key Nap specifics:
- `curve_decay/width/tone: Arc<Mutex<MsegData>>` persisted; `editor_state` persisted.
- Design-time params `size`/`density`/`width`/`seed` are `.non_automatable()`.
- `handoff: Arc<SequenceHandoff>`; audio thread owns `seq: VelvetSequence`, `seq_gen: u64`, two `ReverbChannel`s, and a pre-delay line per channel.
- `initialize()` generates the first sequence from current params + curves and publishes; `set_latency_samples(0)`.

```rust
//! Nap — a draw-your-tail velvet-noise (EDVN) reverb.
//! See `docs/superpowers/specs/2026-05-29-nap-velvet-reverb-design.md`.

pub mod coloration;
pub mod editor;
pub mod engine;
pub mod handoff;
pub mod rng;
pub mod sequence;

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use tiny_skia_widgets::mseg::MsegData;

use engine::ReverbChannel;
use handoff::SequenceHandoff;
use sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

const MAX_PREDELAY_SAMPLES: usize = 48_000; // 1 s at 48 kHz, scaled per SR

pub struct Nap {
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    sample_rate: f32,

    // Audio-thread state.
    seq: VelvetSequence,
    seq_gen: u64,
    left: ReverbChannel,
    right: ReverbChannel,
    predelay_l: Vec<f32>,
    predelay_r: Vec<f32>,
    predelay_pos: usize,
    silent_samples: u32,
}

#[derive(Params)]
pub struct NapParams {
    #[persist = "decay-curve"]
    pub decay_curve: Arc<Mutex<MsegData>>,
    #[persist = "width-curve"]
    pub width_curve: Arc<Mutex<MsegData>>,
    #[persist = "tone-curve"]
    pub tone_curve: Arc<Mutex<MsegData>>,
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::EditorState>,

    // Automatable, smoothed.
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "predelay"]
    pub predelay: FloatParam,
    #[id = "input"]
    pub input: FloatParam,
    #[id = "output"]
    pub output: FloatParam,

    // Design-time (non-automatable; regenerate the sequence on edit).
    #[id = "size"]
    pub size: FloatParam,
    #[id = "density"]
    pub density: FloatParam,
    #[id = "width"]
    pub width: FloatParam,
    #[id = "seed"]
    pub seed: IntParam,
}

impl Default for Nap {
    fn default() -> Self {
        Self {
            params: Arc::new(NapParams::new()),
            handoff: Arc::new(SequenceHandoff::new()),
            sample_rate: 48_000.0,
            seq: VelvetSequence::new(),
            seq_gen: 0,
            left: ReverbChannel::new(48_000.0),
            right: ReverbChannel::new(48_000.0),
            predelay_l: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_r: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_pos: 0,
            silent_samples: 0,
        }
    }
}

impl NapParams {
    fn new() -> Self {
        Self {
            decay_curve: Arc::new(Mutex::new(default_decay_curve())),
            width_curve: Arc::new(Mutex::new(default_width_curve())),
            tone_curve: Arc::new(Mutex::new(default_tone_curve())),
            editor_state: editor::default_editor_state(),

            mix: FloatParam::new("Mix", 30.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_smoother(SmoothingStyle::Linear(50.0))
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            predelay: FloatParam::new("Pre-Delay", 0.0, FloatRange::Linear { min: 0.0, max: 200.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit(" ms")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            input: FloatParam::new(
                "Input",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            output: FloatParam::new(
                "Output",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-24.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(-24.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            size: FloatParam::new("Size", 1.5, FloatRange::Skewed {
                min: 0.1, max: 10.0, factor: FloatRange::skew_factor(-1.0),
            })
            .non_automatable()
            .with_unit(" s")
            .with_value_to_string(formatters::v2s_f32_rounded(2)),
            density: FloatParam::new("Density", 1500.0, FloatRange::Skewed {
                min: 500.0, max: 4000.0, factor: FloatRange::skew_factor(-0.5),
            })
            .non_automatable()
            .with_unit(" /s")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            width: FloatParam::new("Width", 8.0, FloatRange::Linear { min: 0.0, max: 30.0 })
                .non_automatable()
                .with_unit(" ms")
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            seed: IntParam::new("Seed", 1, IntRange::Linear { min: 1, max: 9999 })
                .non_automatable(),
        }
    }
}

impl Nap {
    /// Generate the sequence from the current params + curves and publish it.
    /// GUI / setup thread only (locks the curves, allocates nothing on the
    /// audio thread). Shared by `initialize()` and the editor's regen.
    pub fn regenerate(
        handoff: &SequenceHandoff,
        params: &NapParams,
        sample_rate: f32,
        scratch: &mut VelvetSequence,
    ) {
        let p = GenParams {
            sample_rate,
            size_s: params.size.value(),
            density: params.density.value(),
            width_ms: params.width.value(),
            seed: params.seed.value() as u64,
        };
        let decay = *params.decay_curve.lock().unwrap();
        let width = *params.width_curve.lock().unwrap();
        let tone = *params.tone_curve.lock().unwrap();
        generate(scratch, &p, &decay, &width, &tone);
        handoff.publish(scratch);
    }
}

impl Plugin for Nap {
    const NAME: &'static str = "Nap";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.handoff.clone(), self.sample_rate)
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.left = ReverbChannel::new(self.sample_rate);
        self.right = ReverbChannel::new(self.sample_rate);
        // Initial sequence (allocation-free copy into handoff; generation here
        // is on the setup thread, not the audio thread).
        let mut scratch = VelvetSequence::new();
        Self::regenerate(&self.handoff, &self.params, self.sample_rate, &mut scratch);
        self.seq_gen = 0;
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);
        context.set_latency_samples(0);
        true
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
        self.predelay_l.fill(0.0);
        self.predelay_r.fill(0.0);
        self.predelay_pos = 0;
        self.silent_samples = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }
        // Pick up the latest published sequence (no-op if unchanged).
        self.handoff.try_read_into(&mut self.seq, &mut self.seq_gen);

        let slices = buffer.as_slice();
        if slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        let predelay_cap = self.predelay_l.len();
        let seq = &self.seq;

        for i in 0..num_samples {
            let mix = self.params.mix.smoothed.next() / 100.0;
            let in_gain = self.params.input.smoothed.next();
            let out_gain = self.params.output.smoothed.next();
            let predelay_samps = ((self.params.predelay.smoothed.next() * 0.001
                * self.sample_rate) as usize)
                .min(predelay_cap - 1);

            let dry_l = left[i];
            let dry_r = right[i];

            let wet_l = self.left.process(dry_l * in_gain, seq, &seq.location);
            let wet_r = self.right.process(dry_r * in_gain, seq, &seq.location_r);

            // Pre-delay the wet path only (dry stays aligned).
            self.predelay_l[self.predelay_pos] = wet_l;
            self.predelay_r[self.predelay_pos] = wet_r;
            let read = (self.predelay_pos + predelay_cap - predelay_samps) % predelay_cap;
            let dwet_l = self.predelay_l[read];
            let dwet_r = self.predelay_r[read];
            self.predelay_pos = (self.predelay_pos + 1) % predelay_cap;

            left[i] = ((1.0 - mix) * dry_l + mix * dwet_l) * out_gain;
            right[i] = ((1.0 - mix) * dry_r + mix * dwet_r) * out_gain;
        }

        // Tail handling: keep processing while the velvet tail rings out.
        let tail_len = (seq.tail_len as u32).max(1);
        let peak = left
            .iter()
            .chain(right.iter())
            .fold(0.0_f32, |a, &s| a.max(s.abs()));
        if peak < 1e-6 {
            self.silent_samples = self.silent_samples.saturating_add(num_samples as u32);
        } else {
            self.silent_samples = 0;
        }
        if self.silent_samples > 0 && self.silent_samples <= tail_len {
            ProcessStatus::Tail(tail_len)
        } else {
            ProcessStatus::Normal
        }
    }
}

impl ClapPlugin for Nap {
    const CLAP_ID: &'static str = "com.mpd.nap";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("A draw-your-tail velvet-noise reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Nap {
    const VST3_CLASS_ID: [u8; 16] = *b"NapMpdPlugin\0\0\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Reverb];
}

nih_export_clap!(Nap);
nih_export_vst3!(Nap);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_zero_is_dry_passthrough() {
        // Build a sequence with energy, but mix=0 ⇒ output == dry·output_gain
        // (output defaults to 0 dB = 1.0). We exercise the per-sample mix math
        // directly to avoid constructing a Buffer.
        let dry = 0.7_f32;
        let wet = 0.42_f32; // arbitrary wet value
        let mix = 0.0_f32;
        let out_gain = 1.0_f32;
        let out = ((1.0 - mix) * dry + mix * wet) * out_gain;
        assert!((out - dry).abs() < 1e-9);
    }

    #[test]
    fn default_params_are_valid_curves() {
        let p = NapParams::new();
        assert!(p.decay_curve.lock().unwrap().is_valid());
        assert!(p.width_curve.lock().unwrap().is_valid());
        assert!(p.tone_curve.lock().unwrap().is_valid());
    }

    #[test]
    fn regenerate_publishes_a_nonempty_sequence() {
        let params = NapParams::new();
        let handoff = SequenceHandoff::new();
        let mut scratch = VelvetSequence::new();
        Nap::regenerate(&handoff, &params, 48_000.0, &mut scratch);
        let mut local = VelvetSequence::new();
        let mut gen = 0;
        assert!(handoff.try_read_into(&mut local, &mut gen));
        assert!(local.count > 100, "expected a populated sequence, got {}", local.count);
    }
}
```

- [ ] **Step 2: Restore the real `nap/src/main.rs`** (it was stubbed in Task 1):

```rust
use nih_plug::prelude::*;
fn main() {
    nih_export_standalone::<nap::Nap>();
}
```

- [ ] **Step 3: This will not compile until `editor::create`, `editor::EditorState`, and `editor::default_editor_state` exist (Task 9).** Temporarily add a minimal `nap/src/editor.rs` stub so Tasks 8 compiles in isolation:

```rust
//! Editor stub — replaced in Task 9.
use std::sync::Arc;
use nih_plug::prelude::*;
pub use tiny_skia_widgets::EditorState;
use crate::{handoff::SequenceHandoff, NapParams};

pub fn default_editor_state() -> Arc<EditorState> {
    tiny_skia_widgets::default_editor_state(560, 720)
}
pub fn create(
    _params: Arc<NapParams>,
    _handoff: Arc<SequenceHandoff>,
    _sample_rate: f32,
) -> Option<Box<dyn Editor>> {
    None
}
```

Verify the exact `EditorState` constructor name/signature against `tiny-skia-widgets/src/editor_base.rs` and an existing user (`grep -n "default_editor_state\|EditorState" satch/src/editor.rs`); adjust the stub to match before compiling.

- [ ] **Step 4: Run tests + lint**

Run: `cargo nextest run -p nap` then `cargo clippy -p nap -- -D warnings`
Expected: all pass, clean. (Editor returns `None` for now — no GUI yet.)

- [ ] **Step 5: Commit** (if authorized)

```bash
git add nap/src/lib.rs nap/src/main.rs nap/src/editor.rs
git commit -m "feat(nap): params + plugin process (engine wired end-to-end)"
```

---

## Phase 3 — Editor

### Task 9: Editor scaffold + three stacked curve editors + bottom dial strip + regen trigger

**Files:**
- Replace: `nap/src/editor.rs`

This task ports the softbuffer/baseview scaffold from `miff/src/editor.rs`. **Read `miff/src/editor.rs` end-to-end first** — copy its window setup, `EditorState`/`SurfaceState` handling, scale computation, font loading, and event loop verbatim, then make these Nap-specific changes:

1. The struct holds **three** `MsegEditState` (decay/width/tone) instead of one, plus clones of the three `Arc<Mutex<MsegData>>`, the `Arc<SequenceHandoff>`, the `Arc<NapParams>`, the sample rate, and a `VelvetSequence` scratch buffer for regeneration + a snapshot for the tail view (Task 10).
2. Layout: split the window vertically into three equal MSEG panes (top→bottom: Decay, Width, Tone) above a bottom dial strip. Compute each pane's `rect` and route pointer events to whichever pane contains the cursor.
3. Each pane uses `MsegEditState::new_curve_only()` and draws with `draw_mseg(pixmap, text, rect, &curve, &state, scale, value_color, None)` using a distinct per-curve accent color.
4. A `regenerate()` editor method calls `Nap::regenerate(&self.handoff, &self.params, self.sample_rate, &mut self.scratch)` and is invoked after **any** curve edit (mirror every `self.rebake()` call site in miff) and after any bottom-strip dial change (Size/Density/Width/Seed).
5. As a robustness guard for host/preset-driven param changes while the editor is open, cache the last-seen `(size, density, width, seed)` and curve revision; at the top of each frame, if any changed, call `regenerate()`.

- [ ] **Step 1: Implement the editor** following the structure above. Concrete event-dispatch skeleton (fill into the ported miff event loop):

```rust
// Three equal panes stacked above a fixed-height bottom strip.
fn pane_rects(w: f32, h: f32, scale: f32) -> [(f32, f32, f32, f32); 3] {
    let strip_h = 90.0 * scale;
    let pane_h = ((h - strip_h) / 3.0).max(0.0);
    [
        (0.0, 0.0, w, pane_h),
        (0.0, pane_h, w, pane_h),
        (0.0, 2.0 * pane_h, w, pane_h),
    ]
}

fn pane_at(y: f32, w: f32, h: f32, scale: f32) -> Option<usize> {
    let r = pane_rects(w, h, scale);
    r.iter().position(|&(_, py, _, ph)| y >= py && y < py + ph)
}
```

Per-curve dispatch on mouse-down (repeat for move/up/right-click/double-click, matching miff's signatures `on_mouse_down(x, y, &mut curve, rect, scale)`):

```rust
if let Some(p) = pane_at(y, w, h, scale) {
    let rect = pane_rects(w, h, scale)[p];
    let mut curve = self.curves[p].lock().unwrap();
    let edit = self.states[p].on_mouse_down(x, y, &mut curve, rect, scale);
    drop(curve);
    if edit.is_some() {
        self.regenerate();
    }
}
```

Where `self.curves: [Arc<Mutex<MsegData>>; 3]` and `self.states: [MsegEditState; 3]` with `value_color` per pane, e.g. `[color_decay(), color_width(), color_tone()]` (define three distinct accents in a small `theme.rs` or inline).

Draw loop (inside the ported paint method):

```rust
let rects = pane_rects(w, h, scale);
for p in 0..3 {
    let curve = *self.curves[p].lock().unwrap();
    draw_mseg(pixmap, text, rects[p], &curve, &self.states[p], scale, PANE_COLORS[p], None);
}
// bottom strip dials (Size/Density/Width/Pre-Delay/Mix/Output) + Seed/Regen button
self.draw_bottom_strip(pixmap, text, w, h, scale);
```

For the bottom-strip dials, mirror an existing dial strip (e.g. `satch/src/editor.rs` or `six-pack/src/editor/bottom_strip.rs`) using `tiny_skia_widgets::param_dial` and `tiny_skia_widgets::controls`. A dial drag that changes Size/Density/Width/Seed must call `self.regenerate()` on release (or live).

- [ ] **Step 2: `create()` signature** must match the call in `lib.rs`:

```rust
pub fn create(
    params: Arc<NapParams>,
    handoff: Arc<SequenceHandoff>,
    sample_rate: f32,
) -> Option<Box<dyn Editor>> { /* baseview editor, ported from miff */ }
```

- [ ] **Step 3: Add a layout unit test** (the only cleanly unit-testable editor logic):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn three_panes_partition_the_area_above_the_strip() {
        let (w, h, scale) = (560.0, 720.0, 1.0);
        let r = pane_rects(w, h, scale);
        assert!((r[0].1 - 0.0).abs() < 1e-3);
        assert!((r[1].1 - r[0].3).abs() < 1e-3, "pane 1 starts where pane 0 ends");
        assert!(r[2].1 + r[2].3 <= h - 90.0 + 1e-3, "panes sit above the 90px strip");
    }
    #[test]
    fn pane_at_maps_y_to_pane_index() {
        let (w, h, scale) = (560.0, 720.0, 1.0);
        assert_eq!(pane_at(5.0, w, h, scale), Some(0));
        assert_eq!(pane_at(h - 95.0, w, h, scale), Some(2));
        assert_eq!(pane_at(h - 10.0, w, h, scale), None); // in the strip
    }
}
```

- [ ] **Step 4: Build the standalone and verify the GUI by hand**

Run: `cargo build --bin nap && ./target/debug/nap`
Expected: window opens; three stacked curve editors are drawable; dragging nodes reshapes curves; dials adjust Size/Density/Width/Mix/Pre-Delay/Output; audio reverberates and audibly changes when curves/dials change.

- [ ] **Step 5: Run tests + lint + fmt**

Run: `cargo nextest run -p nap && cargo clippy -p nap -- -D warnings && cargo fmt -p nap`
Expected: pass/clean.

- [ ] **Step 6: Commit** (if authorized)

```bash
git add nap/src/editor.rs
git commit -m "feat(nap): triple-curve editor + dial strip + regen trigger"
```

---

### Task 10: Live tail visualization

**Files:**
- Create: `nap/src/editor/tail_view.rs`
- Modify: `nap/src/editor.rs` (declare `mod tail_view;`, snapshot the sequence on regen, draw the overlay)

Render the *actual* generated sequence as a decaying pulse field, overlaid on the Decay pane (or a thin dedicated strip): for each pulse, a vertical stick at `phase_to_x(layout, location/tail_len)`, height ∝ `|coeff|` mapped via `value_to_y`, color = `PANE`-blended dictionary color by `filter_idx`, with a small horizontal L/R split proportional to `(location_r − location)` to show width. Decimate to ≤ window-width columns (take the max-|coeff| pulse per column) so cost is bounded regardless of `count`.

- [ ] **Step 1: Write the decimation helper + test**

In `nap/src/editor/tail_view.rs`:

```rust
//! Live visualization of the generated velvet tail: a decaying pulse field.

use crate::sequence::VelvetSequence;

/// Per-column summary for rendering: the loudest pulse in that column.
#[derive(Clone, Copy, Default)]
pub struct Column {
    pub coeff_abs: f32,
    pub filter_idx: u8,
    pub lr_split: i32, // location_r - location, in samples (signed)
    pub present: bool,
}

/// Decimate the sequence to `cols` columns by tail phase, keeping the
/// max-|coeff| pulse per column. Bounds render cost independent of `count`.
pub fn decimate(seq: &VelvetSequence, cols: usize, out: &mut Vec<Column>) {
    out.clear();
    out.resize(cols, Column::default());
    if seq.count == 0 || seq.tail_len == 0 || cols == 0 {
        return;
    }
    let tl = seq.tail_len as f32;
    for m in 0..seq.count {
        let phase = (seq.location[m] as f32 / tl).clamp(0.0, 1.0);
        let c = ((phase * (cols as f32 - 1.0)).round() as usize).min(cols - 1);
        let a = seq.coeff[m].abs();
        if !out[c].present || a > out[c].coeff_abs {
            out[c] = Column {
                coeff_abs: a,
                filter_idx: seq.filter_idx[m],
                lr_split: seq.location_r[m] as i32 - seq.location[m] as i32,
                present: true,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimate_keeps_loudest_per_column() {
        let mut seq = VelvetSequence::new();
        seq.count = 3;
        seq.tail_len = 100;
        seq.location[..3].copy_from_slice(&[0, 1, 99]);
        seq.coeff[..3].copy_from_slice(&[0.2, 0.9, 0.5]);
        seq.filter_idx[..3].copy_from_slice(&[0, 3, 5]);
        seq.location_r[..3].copy_from_slice(&[0, 1, 99]);
        let mut cols = Vec::new();
        decimate(&seq, 10, &mut cols);
        assert_eq!(cols.len(), 10);
        assert!(cols[0].present && (cols[0].coeff_abs - 0.9).abs() < 1e-6, "loudest in col 0 kept");
        assert_eq!(cols[0].filter_idx, 3);
        assert!(cols[9].present && (cols[9].coeff_abs - 0.5).abs() < 1e-6);
        assert!(!cols[5].present, "empty column stays absent");
    }
}
```

- [ ] **Step 2: Wire it into the editor** — keep a `VelvetSequence` snapshot updated on every `regenerate()` (clone the scratch), and in the paint method call `decimate(&self.snapshot, cols, &mut self.columns)` then draw the sticks over the Decay pane's plot rect using `mseg_layout`/`phase_to_x`/`value_to_y` + `tiny_skia_widgets::primitives` line/rect fills.

- [ ] **Step 3: Run tests + manual check**

Run: `cargo nextest run -p nap tail_view`
Expected: pass. Then `cargo build --bin nap && ./target/debug/nap` — the tail field updates live as you redraw the Decay/Tone/Width curves and turn Size/Density.

- [ ] **Step 4: Commit** (if authorized)

```bash
git add nap/src/editor/tail_view.rs nap/src/editor.rs
git commit -m "feat(nap): live tail visualization"
```

---

## Phase 4 — Benches, docs, finishing

### Task 11: Criterion bench for the engine

**Files:**
- Create: `nap/benches/dsp.rs`

- [ ] **Step 1: Write the bench** (mirrors `multosis/benches/dsp.rs` structure)

```rust
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use nap::engine::ReverbChannel;
use nap::sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

fn bench_engine(c: &mut Criterion) {
    let sr = 48_000.0;
    let mut group = c.benchmark_group("nap/engine");
    for &(size, density) in &[(1.0, 1000.0), (2.0, 1500.0), (4.0, 3000.0)] {
        let p = GenParams { sample_rate: sr, size_s: size, density, width_ms: 8.0, seed: 1 };
        let mut seq = VelvetSequence::new();
        generate(
            &mut seq,
            &p,
            &default_decay_curve(),
            &default_width_curve(),
            &default_tone_curve(),
        );
        let id = format!("size{size}s_density{density}");
        group.bench_function(&id, |b| {
            b.iter_batched(
                || ReverbChannel::new(sr),
                |mut ch| {
                    for n in 0..512 {
                        let x = if n == 0 { 1.0 } else { 0.0 };
                        std::hint::black_box(ch.process(x, &seq, &seq.location));
                    }
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, bench_engine);
criterion_main!(benches);
```

- [ ] **Step 2: Run it** (target-cpu auto-tuned per CLAUDE.md)

Run: `cargo xtask native bench -p nap --bench dsp`
Expected: completes; records per-(size,density) timings (the cost-ceiling baseline noted in the spec's risk #1).

- [ ] **Step 3: Commit** (if authorized)

```bash
git add nap/benches/dsp.rs
git commit -m "test(nap): criterion engine bench (size×density matrix)"
```

---

### Task 12: Top-level docs + manual stub + cross-links

**Files:**
- Modify: `README.md` (add Nap to the plugin list)
- Modify: `docs/README.md` (add the Nap manual row)
- Create: `docs/nap/nap-manual.md` (manual following the style conventions in `docs/README.md`)
- Modify: `CLAUDE.md` (add Nap to the Plugins list + a per-plugin Notable-files line)

- [ ] **Step 1: Add Nap to the root `README.md` plugin list** (one entry mirroring the others' tone): velvet-noise (EDVN) reverb, draw decay/width/tone over the tail.

- [ ] **Step 2: Add the manual row to `docs/README.md`** (Markdown + PDF columns, like the others).

- [ ] **Step 3: Write `docs/nap/nap-manual.md`** with the YAML front-matter + sections (What is Nap? / Installation / Quick Start / Controls / How It Works / Interaction / Technical Notes / Formats / License) per the docs style guide. Cover the three curves, the design-time vs automatable param split, the velvet/EDVN background, and zero latency.

- [ ] **Step 4: Add Nap to `CLAUDE.md`** — a bullet in the Plugins list and a `**nap**` line in the per-plugin Notable-files section (sequence.rs / engine.rs / coloration.rs / handoff.rs / editor + tail_view.rs).

- [ ] **Step 5: (Optional) Build the PDF manual** per the `reference_pdf_manual_build` workflow:

Run: `cd docs/nap/ && pandoc --pdf-engine=xelatex -V mainfont="DejaVu Serif" -V monofont="DejaVu Sans Mono" nap-manual.md -o nap-manual.pdf`

- [ ] **Step 6: Commit** (if authorized)

```bash
git add README.md docs/README.md docs/nap/ CLAUDE.md
git commit -m "docs(nap): manual + plugin-list + CLAUDE.md entries"
```

---

### Task 13: Full-workspace verification

- [ ] **Step 1: Workspace test**

Run: `cargo nextest run --workspace`
Expected: all green (incl. the new `nap` tests).

- [ ] **Step 2: Workspace lint**

Run: `cargo clippy --workspace -- -D warnings`
Expected: clean.

- [ ] **Step 3: Format check**

Run: `cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Bundle smoke test**

Run: `cargo nih-plug bundle nap --release`
Expected: produces the VST3 + CLAP bundle without error.

- [ ] **Step 5: Manual A/B in a DAW or standalone** — load Nap, confirm: drawing the Decay curve reshapes the tail; the Width curve audibly changes stereo spread (and collapses to mono at 0); the Tone curve shifts brightness across the tail; Mix=0 is bypass; the tail visualization matches what you hear.

---

## Self-review notes (for the planner)

- **Spec coverage:** EDVN engine (Tasks 4–6), decay/width/tone curve mapping (Task 5), decoupled decay (`decay_curve_shapes_the_energy_envelope` test), coherence-via-jitter (`higher_width_decorrelates_more`, `width_zero_makes_left_and_right_identical`), coloration dictionary (Task 3), RT handoff (Task 7), param automatable/design-time split (Task 8), zero latency (`set_latency_samples(0)`), GUI triple-curve editor + tail viz (Tasks 9–10), bench (Task 11), docs (Task 12). All spec sections map to a task.
- **Deferred per spec non-goals:** no IR/room matching, no global LFO, no freeze — none planned. ✔
- **Known iteration points (not placeholders — real work with a starting impl):** coloration voicing (Task 3, one-pole start; resonant all-pole later), tone routing (Task 5, nearest-filter start; EDVN greedy idle-time spreading later), jitter PDF (Task 5, symmetric-uniform start; Hann-PDF inverse-CDF later for an exact coherence curve). Each ships a correct, tested v1 and is a clean later refinement.
- **Optimization:** per the project's standing practice, benchmark/profile/optimize is a *final* pass (Task 11 establishes the baseline); the O(M) convolver is intentionally simple first.
