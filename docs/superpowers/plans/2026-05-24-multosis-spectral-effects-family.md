# Multosis Spectral Effects Family Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 14 Infiltrator-style spectral effects to multosis as new `EffectKind` variants, sharing a single audio-thread-safe FFT engine in `tract-dsp`.

**Architecture:** A new `tract_dsp::spectral_engine::SpectralEngine` holds all four FFT sizes (512/1024/2048/4096) pre-allocated as slots so FFT-size changes never allocate on the audio thread. Each effect implements a `SpectralTransform` trait method that mutates the spectrum in place; the engine owns the analyzer + IFFT + overlap-add scaffolding. Per-effect Rust files (~150 LOC each) wire the trait into multosis's `Effect` plumbing.

**Tech Stack:** Rust (nightly), `tract-dsp` and `multosis` crates in the `tract-plugin-pack` Cargo workspace. `rustfft`, `cargo nextest`.

**Spec:** `docs/superpowers/specs/2026-05-24-multosis-spectral-effects-family-design.md`

**Conventions:**
- Run all `cargo`/`git` from the workspace root `/home/mpd/git-sources/tract-plugin-pack`. Branch: `multosis`.
- Per-crate fast iteration: `cargo build -p multosis`, `cargo nextest run -p tract-dsp`, etc.
- Final CI gate per step: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release`.
- ASCII-only commit messages — em-dash and unicode arrows render garbled in the user's pager.
- Never use `#[allow(...)]` to silence a warning. Fix the cause.
- Commit message trailer MUST be exactly: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Never commit unless this plan explicitly says so at a step boundary. The plan authorises one commit per task.

## Background — the current code

`multosis/src/effects/mod.rs` (~1320 LOC) is the central registry. Each effect lives at `multosis/src/effects/<name>.rs`, implements the `Effect` trait, and is wired in via 14 separate sites in `mod.rs` (re-export, `EffectKind` variant, `ALL` array, `name()` arm, `EffectInstance` variant, `EffectInstance::new` arm, 9 dispatch arms — `kind`, `process_sample`, `set_sample_rate`, `reset`, `parameters`, `set_param`, `set_bpm`, `param_dimmed`, `latency_samples`).

`tract-dsp` exposes:
- `StftAnalyzer` (input ring + periodic-Hann + COLA synthesis window + forward FFT)
- `stft::StftConvolver` (full fixed-frame magnitude-multiply overlap-add — pattern reference)
- `spectral_shifter::SpectralShifter` (phase vocoder; the closest existing pattern to what `SpectralEngine` will be)

`tract-dsp` is feature-gated. `stft` and `stft-analysis` are existing features.

## File structure

**New files:**
- `tract-dsp/src/spectral_engine.rs` — the shared engine + `SpectralTransform` trait + tests.
- `multosis/src/effects/spectral_rotate.rs` — Task 2
- `multosis/src/effects/spectral_bandpass.rs` — Task 3
- `multosis/src/effects/spectral_mirror.rs` — Task 4
- `multosis/src/effects/spectral_shift.rs` — Task 5 (Translate-only) + Task 14 (Scale completed)
- `multosis/src/effects/spectral_spread.rs` — Task 6
- `multosis/src/effects/spectral_lofi.rs` — Task 7
- `multosis/src/effects/spectral_smear.rs` — Task 8
- `multosis/src/effects/spectral_corrupt.rs` — Task 9
- `multosis/src/effects/spectral_compress.rs` — Task 10
- `multosis/src/effects/spectral_cascade.rs` — Task 11
- `multosis/src/effects/spectral_reverb.rs` — Task 12
- `multosis/src/effects/spectral_scatter.rs` — Task 13
- `multosis/src/effects/spectral_twist.rs` — Task 15
- `multosis/src/effects/spectral_stretch.rs` — Task 16

**Modified files:**
- `tract-dsp/Cargo.toml` — add `spectral-engine` feature gate.
- `tract-dsp/src/lib.rs` — `pub mod spectral_engine;` behind the feature.
- `multosis/Cargo.toml` — enable the new feature.
- `multosis/src/effects/mod.rs` — extended 14 times (one per effect).
- `CLAUDE.md` and `multosis/CLAUDE.md` — Task 17.

## Per-effect mod.rs integration recipe

Every per-effect task ends with the same 14-site edit to `multosis/src/effects/mod.rs`. Each task spells out the exact value to insert at each site for *that* effect. The sites, in source order, are:

1. **mod declaration** (top of file). Add `mod spectral_<name>;` in alphabetical order with the other `mod` lines.
2. **re-export** (just below). Add `pub use spectral_<name>::Spectral<Name>Effect;`.
3. **`EffectKind` variant** (~line 365, after `Vocoder`). Add `Spectral<Name>,` with a doc comment.
4. **`EffectKind::ALL` array** (~line 370). Bump the array length and append `EffectKind::Spectral<Name>,`.
5. **`name()` match arm** (~line 419). Add `EffectKind::Spectral<Name> => "Spectral <Display Name>",`.
6. **`reports_latency()` match** (~line 442). Add the variant to the list — all spectral effects report latency (= active FFT hop).
7. **`EffectInstance` variant** (~line 520). Add `Spectral<Name>(Box<Spectral<Name>Effect>),` (boxed to keep enum size bounded).
8. **`EffectInstance::new` arm** (~line 552). Add `EffectKind::Spectral<Name> => EffectInstance::Spectral<Name>(Box::default()),`.
9. **`kind()` arm** (~line 580). Add `EffectInstance::Spectral<Name>(_) => EffectKind::Spectral<Name>,`.
10. **`process_sample` arm** (~line 609). Add `EffectInstance::Spectral<Name>(e) => e.process_sample(left, right),`.
11. **`set_sample_rate` arm** (~line 636). Add `EffectInstance::Spectral<Name>(e) => e.set_sample_rate(sample_rate),`.
12. **`reset` arm** (~line 663). Add `EffectInstance::Spectral<Name>(e) => e.reset(),`.
13. **`parameters` arm** (~line 690). Add `EffectInstance::Spectral<Name>(e) => e.parameters(),`.
14. **`set_param` arm** (~line 717). Add `EffectInstance::Spectral<Name>(e) => e.set_param(index, value),`.
15. **`set_bpm` arm** (~line 744). Add `EffectInstance::Spectral<Name>(e) => e.set_bpm(bpm),`.
16. **`param_dimmed` arm** (~line 771). Add `EffectInstance::Spectral<Name>(e) => e.param_dimmed(index),`.
17. **`latency_samples` arm** (~line 798). Add `EffectInstance::Spectral<Name>(e) => e.latency_samples(),`.
18. **`EffectKind::ALL.len()` assertion** (~line 869). Bump the asserted length by one each effect.
19. **`effect_kind_name_matches` test** (~line 891). Add `assert_eq!(EffectKind::Spectral<Name>.name(), "Spectral <Display Name>");`.
20. **`default_params_for_kind_*` enumeration test** (~line 1037). The test iterates `EffectKind::ALL` so no explicit edit, but the *count* assertion (line 1037 region) may need updating.

(Sites 1, 2, 18-20 are administrative; sites 3-17 are the per-variant additions; everything is mechanical.)

The first effect (Task 2) walks through every site in full. Subsequent tasks list ONLY the inserted lines, not the surrounding context — find them by searching for the analogous Vocoder/etc. lines.

## Per-effect file skeleton

Every spectral effect file follows this skeleton. Per-effect tasks fill in the marked sections.

```rust
//! Spectral <Name>: <one-line description>.
//!
//! <DSP outline — 2-3 sentences referring to the spec doc.>

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform};

/// Per-channel mutable state owned by one [`SpectralEngine`]. Holding the
/// per-channel state separately from the effect lets each channel's
/// `SpectralEngine::process_sample` borrow its own `&mut SpectralTransform`
/// without aliasing the other channel.
struct Spectral<Name>Channel {
    // <effect-specific per-channel state — e.g. bin delay lines>
}

impl Spectral<Name>Channel {
    fn new(sample_rate: f32) -> Self {
        Self {
            // <init>
        }
    }

    fn reset(&mut self) {
        // <zero state>
    }
}

/// Per-channel parameter snapshot. Cached by `set_param` so the per-sample
/// `transform` is allocation-free and uses only the precomputed linear values.
#[derive(Clone, Copy)]
struct Spectral<Name>Params {
    // <linear/derived values for the transform>
}

/// The trait impl carrier — one `&mut TransformCtx` is passed into
/// `SpectralEngine::process_sample` per channel. Wraps the per-channel state
/// + a shared params snapshot.
struct TransformCtx<'a> {
    chan: &'a mut Spectral<Name>Channel,
    params: Spectral<Name>Params,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(
        &mut self,
        spectrum: &mut [rustfft::num_complex::Complex<f32>],
        fft_size: usize,
        sample_rate: f32,
    ) {
        // <per-bin DSP — operates on `spectrum` in place>
    }
}

pub struct Spectral<Name>Effect {
    sample_rate: f32,
    params: Spectral<Name>Params,
    chan_l: Spectral<Name>Channel,
    chan_r: Spectral<Name>Channel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl Spectral<Name>Effect {
    const PARAMS: [ParamSpec; N] = [
        // FFT size always slot 0
        ParamSpec {
            name: "FFT",
            min: 0.0, max: 3.0, default: 2.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum { labels: &["512", "1024", "2048", "4096"] },
        },
        // <effect-specific params>
    ];
}

impl Default for Spectral<Name>Effect {
    fn default() -> Self {
        let sr = 48_000.0;
        let params = Spectral<Name>Params::default_from_specs(&Self::PARAMS, sr);
        Self {
            sample_rate: sr,
            params,
            chan_l: Spectral<Name>Channel::new(sr),
            chan_r: Spectral<Name>Channel::new(sr),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for Spectral<Name>Effect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx { chan: &mut self.chan_l, params: self.params };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx { chan: &mut self.chan_r, params: self.params };
        let ro = self.engine_r.process_sample(right, &mut ctx_r);
        (lo, ro)
    }
    fn parameters(&self) -> &'static [ParamSpec] { &Self::PARAMS }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.engine_l = SpectralEngine::new(sample_rate);
        self.engine_r = SpectralEngine::new(sample_rate);
        self.chan_l = Spectral<Name>Channel::new(sample_rate);
        self.chan_r = Spectral<Name>Channel::new(sample_rate);
        // Re-apply the FFT-size param so the new engines pick it up.
        self.set_param(0, /* current FFT param value */ );
    }
    fn reset(&mut self) {
        self.engine_l.reset();
        self.engine_r.reset();
        self.chan_l.reset();
        self.chan_r.reset();
    }
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                let fft_size = [512, 1024, 2048, 4096][value.round() as usize % 4];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
                self.params.fft_param = value;
            }
            // <other param indices>
            _ => {}
        }
    }
    fn latency_samples(&self) -> usize { self.engine_l.latency_samples() }
}
```

`Spectral<Name>Params::default_from_specs` is a small helper each effect implements over its own `PARAMS` array (just reads the `default` of each `ParamSpec` and maps to the linear cached value). See Task 2 for the concrete pattern.

## ParamSpec conventions

The `ParamScaling` enum has only two variants: `Linear` and `Log`. There is NO `Stepped` variant. Discrete selectors are built by combining `ParamScaling::Linear` over a 0..N-1 integer range with `ParamFormat::Enum { labels }`.

The `ParamFormat` enum:
- `Number { decimals: u8, unit: &'static str }` — fixed-precision numeric. Use for percentages, dB, octaves, ms, etc.
- `Hertz` — auto Hz/kHz formatting (< 1 → 2 decimals; 1..1000 → 0 decimals; ≥ 1000 → 1 decimal kHz). Use for ALL frequency-in-Hz params (Freq, Centre, Rate).
- `Enum { labels: &'static [&'static str] }` — discrete selector matching a `Linear` 0..labels.len()-1 range. Use for the FFT-size selector.

The `Effect` trait has default impls for `set_bpm` and `param_dimmed`, so per-effect impls can omit them. The `EffectInstance` dispatch arms still need entries for those methods (sites 15 and 16 in the per-effect mod.rs integration recipe).

---

## Task 1: Add `SpectralEngine` to `tract-dsp`

Build the shared FFT engine in `tract-dsp` with all four FFT sizes pre-allocated and exhaustively unit-tested. Purely additive — nothing in multosis depends on it yet, the workspace stays green.

**Files:**
- Create: `tract-dsp/src/spectral_engine.rs`
- Modify: `tract-dsp/Cargo.toml`
- Modify: `tract-dsp/src/lib.rs`

- [ ] **Step 1: Add the `spectral-engine` feature**

Open `tract-dsp/Cargo.toml`. Under `[features]`, add this line below the existing `stft-analysis` line:

```toml
spectral-engine = ["stft-analysis"]
```

`rustfft` is pulled in by `stft-analysis` already, so depending on that feature is enough. (Don't add `dep:rustfft` again — it's already optional and gated by `stft-analysis`.)

- [ ] **Step 2: Expose the module behind the feature**

In `tract-dsp/src/lib.rs`, find the existing `#[cfg(feature = "stft-analysis")] pub mod stft_analysis;` line. Immediately below it, add:

```rust
#[cfg(feature = "spectral-engine")]
pub mod spectral_engine;
```

- [ ] **Step 3: Write the engine module with its tests**

Create `tract-dsp/src/spectral_engine.rs` with this content:

```rust
//! Audio-thread-safe per-channel STFT analysis/synthesis with switchable
//! FFT size.
//!
//! [`SpectralEngine`] pre-allocates all four supported FFT sizes
//! (512 / 1024 / 2048 / 4096) at construction. [`SpectralEngine::set_fft_size`]
//! latches a switch that takes effect at the next hop boundary of the new
//! slot, costing zero allocations on the audio thread.
//!
//! Effects implement [`SpectralTransform`] and pass an instance to
//! [`SpectralEngine::process_sample`] per call; the engine drives input
//! ring -> hop boundary -> analyze -> caller transform -> IFFT -> overlap-add
//! -> output sample.
//!
//! Hop ratio is fixed at 50% (`hop = fft_size / 2`), matching the periodic-Hann
//! analysis window's natural COLA point. Effects that need 75% overlap
//! (phase vocoders) hold their own analyzer outside the engine.

use crate::stft_analysis::StftAnalyzer;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// The four supported FFT sizes, in display order. Effect param 0 selects
/// an index into this array.
pub const FFT_SIZES: [usize; 4] = [512, 1024, 2048, 4096];

/// A spectrum transform driven by [`SpectralEngine`]. The engine calls
/// [`transform`](Self::transform) once per hop with the freshest analysis
/// spectrum; the implementer mutates it in place. Magnitude AND phase are
/// fair game.
pub trait SpectralTransform {
    fn transform(
        &mut self,
        spectrum: &mut [Complex<f32>],
        fft_size: usize,
        sample_rate: f32,
    );
}

struct Slot {
    fft_size: usize,
    hop_size: usize,
    analyzer: StftAnalyzer,
    ifft: Arc<dyn Fft<f32>>,
    output_ring: Vec<f32>,
    output_pos: usize,
    hop_counter: usize,
    spectrum_scratch: Vec<Complex<f32>>,
    ifft_scratch: Vec<Complex<f32>>,
}

impl Slot {
    fn new(fft_size: usize, planner: &mut FftPlanner<f32>) -> Self {
        let hop_size = fft_size / 2;
        let ifft = planner.plan_fft_inverse(fft_size);
        let scratch_len = ifft.get_inplace_scratch_len();
        Self {
            fft_size,
            hop_size,
            analyzer: StftAnalyzer::new(fft_size, hop_size),
            ifft,
            output_ring: vec![0.0; fft_size],
            output_pos: 0,
            hop_counter: 0,
            spectrum_scratch: vec![Complex::default(); fft_size],
            ifft_scratch: vec![Complex::default(); scratch_len],
        }
    }

    fn reset(&mut self) {
        self.analyzer.reset();
        self.output_ring.fill(0.0);
        self.output_pos = 0;
        self.hop_counter = 0;
    }
}

/// Per-channel STFT engine. Construct one per audio channel.
pub struct SpectralEngine {
    slots: [Slot; 4],
    active: usize,
    pending: Option<usize>,
    sample_rate: f32,
}

impl SpectralEngine {
    /// Build a new engine with all four FFT sizes pre-allocated. Active
    /// FFT size defaults to 2048 (index 2 in [`FFT_SIZES`]).
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let slots = [
            Slot::new(FFT_SIZES[0], &mut planner),
            Slot::new(FFT_SIZES[1], &mut planner),
            Slot::new(FFT_SIZES[2], &mut planner),
            Slot::new(FFT_SIZES[3], &mut planner),
        ];
        Self { slots, active: 2, pending: None, sample_rate }
    }

    /// Latch an FFT-size switch. Takes effect at the next hop boundary of the
    /// new slot. Unknown sizes are silently ignored.
    pub fn set_fft_size(&mut self, fft_size: usize) {
        if let Some(idx) = FFT_SIZES.iter().position(|&s| s == fft_size) {
            if idx != self.active {
                self.pending = Some(idx);
            }
        }
    }

    /// Current active FFT size.
    pub fn fft_size(&self) -> usize {
        self.slots[self.active].fft_size
    }

    /// Algorithmic latency in samples — equal to the active slot's hop size.
    pub fn latency_samples(&self) -> usize {
        self.slots[self.active].hop_size
    }

    /// Zero all ring buffers in all four slots. Used by `Effect::reset`.
    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.reset();
        }
        self.pending = None;
    }

    /// Push one input sample, optionally drive an analysis + transform +
    /// synthesis hop, pull and return one output sample. Allocation-free.
    pub fn process_sample<T: SpectralTransform>(&mut self, input: f32, t: &mut T) -> f32 {
        let slot = &mut self.slots[self.active];

        // Output read first — matches spectral_shifter and gives the engine
        // its full latency = hop_size (the just-overlap-added samples sit in
        // the ring until the read catches up).
        let out = slot.output_ring[slot.output_pos];
        slot.output_ring[slot.output_pos] = 0.0;
        slot.output_pos = (slot.output_pos + 1) % slot.fft_size;

        slot.analyzer.write(input);
        slot.hop_counter += 1;

        if slot.hop_counter >= slot.hop_size {
            slot.hop_counter = 0;

            let sample_rate = self.sample_rate;
            let fft_size = slot.fft_size;
            let frame = slot.analyzer.analyze();

            // Copy spectrum out, transform, IFFT back into spectrum_scratch.
            slot.spectrum_scratch.copy_from_slice(frame.spectrum);
            t.transform(&mut slot.spectrum_scratch, fft_size, sample_rate);

            // In-place IFFT.
            slot.ifft
                .process_with_scratch(&mut slot.spectrum_scratch, &mut slot.ifft_scratch);

            // 1/N normalisation + window + overlap-add.
            let inv_n = 1.0 / fft_size as f32;
            let synth = frame.synthesis_window;
            let ring = &mut slot.output_ring;
            let pos = slot.output_pos;
            let n = slot.fft_size;
            for i in 0..n {
                let ring_idx = (pos + i) % n;
                ring[ring_idx] += slot.spectrum_scratch[i].re * inv_n * synth[i];
            }

            // Apply pending FFT-size switch at hop boundary.
            if let Some(new_active) = self.pending.take() {
                self.active = new_active;
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustfft::num_complex::Complex;

    /// An identity transform — leaves the spectrum untouched. Lets the tests
    /// validate the engine's analysis/synthesis path on its own.
    struct Identity;
    impl SpectralTransform for Identity {
        fn transform(&mut self, _s: &mut [Complex<f32>], _n: usize, _sr: f32) {}
    }

    /// A constant-magnitude transform — zeroes phase, sets every bin to 1+0i.
    /// Used by the FFT-size-switch test to ensure the new slot is exercised.
    struct AllOnes;
    impl SpectralTransform for AllOnes {
        fn transform(&mut self, s: &mut [Complex<f32>], _n: usize, _sr: f32) {
            for b in s.iter_mut() {
                *b = Complex::new(1.0, 0.0);
            }
        }
    }

    /// Drive `n_samples` of an input function through the engine with the
    /// given transform, returning the collected output.
    fn drive<F: FnMut(usize) -> f32, T: SpectralTransform>(
        engine: &mut SpectralEngine,
        n_samples: usize,
        mut input: F,
        t: &mut T,
    ) -> Vec<f32> {
        (0..n_samples).map(|i| engine.process_sample(input(i), t)).collect()
    }

    #[test]
    fn fft_sizes_constant_matches_doc() {
        assert_eq!(FFT_SIZES, [512, 1024, 2048, 4096]);
    }

    #[test]
    fn default_active_is_2048() {
        let e = SpectralEngine::new(48_000.0);
        assert_eq!(e.fft_size(), 2048);
        assert_eq!(e.latency_samples(), 1024);
    }

    #[test]
    fn set_fft_size_latches_change_until_next_hop() {
        let mut e = SpectralEngine::new(48_000.0);
        e.set_fft_size(512);
        // Active is still 2048 right after the call — the switch is latched
        // until the next hop boundary completes inside process_sample.
        assert_eq!(e.fft_size(), 2048);

        // Drive enough samples to cross at least one hop_size (= 1024 for
        // 2048-pt). The pending switch must be consumed inside that window.
        let mut id = Identity;
        let _ = drive(&mut e, 1100, |_| 0.0, &mut id);
        assert_eq!(e.fft_size(), 512);
        assert_eq!(e.latency_samples(), 256);
    }

    #[test]
    fn set_fft_size_unknown_is_noop() {
        let mut e = SpectralEngine::new(48_000.0);
        e.set_fft_size(777); // not in FFT_SIZES
        assert!(e.pending.is_none());
        assert_eq!(e.fft_size(), 2048);
    }

    #[test]
    fn identity_passes_sine_within_3db_after_latency() {
        let sr = 48_000.0;
        let mut e = SpectralEngine::new(sr);
        let f = 1000.0;
        let n = 8192_usize;
        let mut id = Identity;
        let out = drive(&mut e, n, |i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin(), &mut id);

        // Skip the first 2 * latency samples while the ring fills (warm-up
        // until first analysis frame is OLA-deposited, then read).
        let warmup = 2 * e.latency_samples();
        let peak: f32 = out[warmup..].iter().cloned().fold(0.0, f32::max);
        let trough: f32 = out[warmup..].iter().cloned().fold(0.0, |a, x| a.min(x));
        let amp = (peak - trough) / 2.0;

        // Identity should reconstruct the input sine within 3 dB amplitude.
        // 3 dB linear = 10^(-3/20) = 0.708.
        assert!(
            amp > 0.708,
            "identity sine amplitude {amp} fell below 0.708 (3 dB below unity)"
        );
        assert!(
            amp < 1.0 / 0.708,
            "identity sine amplitude {amp} exceeded 1.41 (3 dB above unity)"
        );
    }

    #[test]
    fn impulse_response_finite_under_identity() {
        let mut e = SpectralEngine::new(48_000.0);
        let mut id = Identity;
        let out = drive(&mut e, 4096, |i| if i == 0 { 1.0 } else { 0.0 }, &mut id);
        assert!(out.iter().all(|x| x.is_finite()));
        // Identity must produce SOME non-zero output after the latency.
        let energy: f32 = out.iter().map(|x| x * x).sum();
        assert!(energy > 0.01, "identity impulse response energy {energy} too low");
    }

    #[test]
    fn reset_zeros_all_slots() {
        let mut e = SpectralEngine::new(48_000.0);
        let mut id = Identity;
        // Run some content through.
        let _ = drive(&mut e, 4096, |i| ((i as f32) * 0.1).sin(), &mut id);
        // Switch slots so all four get exercised.
        for &size in &FFT_SIZES {
            e.set_fft_size(size);
            let _ = drive(&mut e, 2048, |i| ((i as f32) * 0.1).sin(), &mut id);
        }
        e.reset();
        // Drive silence — output must be exactly zero for the first sample
        // (no leftover ring content).
        let first = e.process_sample(0.0, &mut id);
        assert_eq!(first, 0.0);
    }

    #[test]
    fn all_ones_transform_produces_output() {
        // Ensures the synthesis path is wired: a transform that writes
        // non-zero bins must produce non-zero output (sanity test).
        let mut e = SpectralEngine::new(48_000.0);
        let mut t = AllOnes;
        let out = drive(&mut e, 4096, |_| 1.0, &mut t);
        let energy: f32 = out.iter().map(|x| x * x).sum();
        assert!(energy > 0.0);
    }
}
```

- [ ] **Step 4: Build the crate**

```bash
cargo build -p tract-dsp --features spectral-engine
```

Expected: clean build, no warnings.

- [ ] **Step 5: Run the engine tests**

```bash
cargo nextest run -p tract-dsp --features spectral-engine spectral_engine::tests
```

Expected: 8 tests pass.

- [ ] **Step 6: Workspace gate**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace
```

Expected: all green. Workspace test count unchanged from baseline + the 8 engine tests.

- [ ] **Step 7: Commit**

```bash
git add tract-dsp/src/spectral_engine.rs tract-dsp/Cargo.toml tract-dsp/src/lib.rs
git commit -m "feat(tract-dsp): SpectralEngine -- shared switchable-FFT analysis/synthesis

Pre-allocates all four supported FFT sizes (512/1024/2048/4096) as
slots so set_fft_size is zero-allocation on the audio thread. Effects
implement SpectralTransform and pass an instance to process_sample;
the engine drives the input ring, hop counter, analyzer, transform
call, IFFT, and overlap-add. Hop ratio is fixed at 50% (Hann COLA).

Feature-gated behind 'spectral-engine'. Tests cover identity-sine
reconstruction within 3 dB, impulse-response stability, mid-stream
FFT-size switch, unknown-size no-op, reset, and the synthesis path
via an all-ones transform.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: SpectralRotate end-to-end (template effect)

Wire the simplest spectral effect — `SpectralRotate` — through the full multosis integration pipeline. This task spells out every site touched in `mod.rs`; subsequent per-effect tasks list only the inserted lines.

**Files:**
- Create: `multosis/src/effects/spectral_rotate.rs`
- Modify: `multosis/Cargo.toml` (enable the `tract-dsp/spectral-engine` feature)
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Enable the spectral-engine feature in multosis**

Open `multosis/Cargo.toml`. Find the `tract-dsp = { ... }` dependency line. Append the new feature to its `features` array. If the line currently reads:

```toml
tract-dsp = { path = "../tract-dsp", features = ["stft", "stft-analysis"] }
```

change it to:

```toml
tract-dsp = { path = "../tract-dsp", features = ["stft", "stft-analysis", "spectral-engine"] }
```

(Use whatever existing features the line already has — just add `"spectral-engine"` to the list.)

- [ ] **Step 2: Write the effect file**

Create `multosis/src/effects/spectral_rotate.rs` with this content:

```rust
//! Spectral Rotate: circular shift of the spectrum.
//!
//! Bins are rotated by `shift_bins = round(shift_pct/100 * N/2)`. Unlike
//! SpectralShift (which zeros out-of-range bins), Rotate wraps modulo N/2
//! so nothing is lost. See the spec doc for the full DSP outline.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy, Default)]
struct ParamsCache {
    fft_param: f32,
    shift_pct: f32,
}

struct TransformCtx {
    shift_pct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(
        &mut self,
        spectrum: &mut [Complex<f32>],
        fft_size: usize,
        _sample_rate: f32,
    ) {
        // Real-input spectrum is conjugate-symmetric around bin N/2.
        // Operate on the positive-frequency half [1..half) and mirror.
        let half = fft_size / 2;
        let shift_bins =
            ((self.shift_pct * 0.01) * half as f32).round() as i32;
        if shift_bins == 0 {
            return;
        }
        // Buffer the positive-half bins, then write the rotated values back.
        // We use spectrum[half..] as scratch (we'll rewrite it from the
        // mirrored positive half before returning).
        // Safer: clone the positive half into a stack-sized array? half can be
        // up to 2048. Avoid stack-alloc, use a single Vec lifted to a
        // method-local — but allocations on the audio thread are forbidden.
        // Instead: do an in-place cyclic rotation using a temp on the
        // negative half (which is overwritten anyway from the conjugate of
        // the positive half).
        //
        // Plan: copy positive half [1..half] into negative half slots
        // [half+1..N] (using their existing contents as throwaway), then
        // write the rotated values back into [1..half], then rebuild the
        // negative half from the conjugates.
        let n = fft_size;
        // Stash positive-half bins into negative-half slots.
        for k in 1..half {
            spectrum[n - k] = spectrum[k];
        }
        // Rotate write.
        for k in 1..half as i32 {
            let src = (((k - shift_bins).rem_euclid(half as i32 - 1)) + 1) as usize;
            // ^ src ranges over [1..half), wrapping. (half-1 distinct bins.)
            // Read from the stashed mirror: stashed[m] is at spectrum[n - m].
            spectrum[k as usize] = spectrum[n - src];
        }
        // Rebuild negative half from conjugates of rotated positive half.
        for k in 1..half {
            spectrum[n - k] = spectrum[k].conj();
        }
        // DC (k=0) and Nyquist (k=half) are untouched — they're real-valued
        // and don't participate in the rotation.
    }
}

pub struct SpectralRotateEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralRotateEffect {
    pub const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "FFT",
            min: 0.0,
            max: 3.0,
            default: 2.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum { labels: &["512", "1024", "2048", "4096"] },
        },
        ParamSpec {
            name: "Shift",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 0, unit: " %" },
        },
    ];
}

impl Default for SpectralRotateEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache { fft_param: 2.0, shift_pct: 0.0 },
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralRotateEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx { shift_pct: self.params.shift_pct };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx { shift_pct: self.params.shift_pct };
        let ro = self.engine_r.process_sample(right, &mut ctx_r);
        (lo, ro)
    }

    fn parameters(&self) -> &'static [ParamSpec] { &Self::PARAMS }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.engine_l = SpectralEngine::new(sample_rate);
        self.engine_r = SpectralEngine::new(sample_rate);
        // Re-apply the FFT-size param so the new engines pick it up.
        let fft_size = FFT_SIZES[self.params.fft_param.round() as usize % 4];
        self.engine_l.set_fft_size(fft_size);
        self.engine_r.set_fft_size(fft_size);
    }

    fn reset(&mut self) {
        self.engine_l.reset();
        self.engine_r.reset();
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
            1 => self.params.shift_pct = value.clamp(-100.0, 100.0),
            _ => {}
        }
    }

    fn latency_samples(&self) -> usize {
        self.engine_l.latency_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(e: &mut SpectralRotateEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n).map(|i| { let x = src(i); e.process_sample(x, x).0 }).collect()
    }

    #[test]
    fn parameters_lists_fft_and_shift() {
        let e = SpectralRotateEffect::default();
        let p = e.parameters();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "FFT");
        assert_eq!(p[1].name, "Shift");
    }

    #[test]
    fn shift_zero_is_passthrough() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(0, 1.0); // FFT = 1024 for shorter warm-up
        e.set_param(1, 0.0); // Shift = 0
        let f = 1000.0;
        let sr = 48_000.0;
        let n = 4096_usize;
        let out = drive(&mut e, n, |i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin());
        // After warm-up, output should retain non-trivial energy.
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(1, 50.0); // arbitrary non-zero shift
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn fft_size_param_changes_latency() {
        let mut e = SpectralRotateEffect::default();
        for (i, expected) in [(0.0, 256), (1.0, 512), (2.0, 1024), (3.0, 2048)] {
            e.set_param(0, i);
            // Drive enough samples to trigger the pending switch.
            let _ = drive(&mut e, 2200, |_| 0.0);
            assert_eq!(e.latency_samples(), expected);
        }
    }

    #[test]
    fn shift_positive_moves_energy_up() {
        // A 1 kHz sine shifted +50% should produce most of its energy
        // above 1 kHz. Tested via a forward FFT of the output.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let f = 1000.0_f32;
        let mut e = SpectralRotateEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 50.0);
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin());
        // Take the tail after warm-up.
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter().map(|&x| Complex::new(x, 0.0)).collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        // Energy above 1 kHz bin should exceed energy below it.
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let lo: f32 = tail[..bin_1k].iter().map(|c| c.norm_sqr()).sum();
        let hi: f32 = tail[bin_1k..1024].iter().map(|c| c.norm_sqr()).sum();
        assert!(hi > lo, "expected hi-band energy > lo-band; got hi={hi} lo={lo}");
    }
}
```

- [ ] **Step 3: Add `mod` and `pub use` lines (sites 1-2)**

In `multosis/src/effects/mod.rs`, find the alphabetical `mod` block at the top. Insert in alphabetical order:

```rust
mod spectral_rotate;
```

And in the `pub use` block immediately below:

```rust
pub use spectral_rotate::SpectralRotateEffect;
```

- [ ] **Step 4: Add `EffectKind` variant (site 3)**

In the `enum EffectKind` block (around line 365), after the `Vocoder,` line, add:

```rust
    /// Spectral Rotate -- circular shift of the spectrum (wraps modulo N/2).
    /// FFT size selectable 512/1024/2048/4096.
    SpectralRotate,
```

- [ ] **Step 5: Add to `EffectKind::ALL` (site 4)**

Find `pub const ALL: [EffectKind; 22]`. Change `22` to `23`. Inside the array, append after `EffectKind::Vocoder,`:

```rust
        EffectKind::SpectralRotate,
```

- [ ] **Step 6: Add to `name()` (site 5)**

In the `name()` match, after the `EffectKind::Vocoder => "Vocoder",` arm:

```rust
            EffectKind::SpectralRotate => "Spectral Rotate",
```

- [ ] **Step 7: Add to `reports_latency()` (site 6)**

Find `reports_latency` (around line 442). Extend the `matches!` to include `SpectralRotate`:

```rust
        matches!(self, Self::Satch | Self::WarpZone | Self::SpectralRotate)
```

- [ ] **Step 8: Add `EffectInstance` variant (site 7)**

In `pub enum EffectInstance`, after the `Vocoder(Box<VocoderEffect>),` line:

```rust
    SpectralRotate(Box<SpectralRotateEffect>),
```

- [ ] **Step 9: Add to `EffectInstance::new` (site 8)**

After the `EffectKind::Vocoder => EffectInstance::Vocoder(Box::default()),` arm:

```rust
            EffectKind::SpectralRotate => EffectInstance::SpectralRotate(Box::default()),
```

- [ ] **Step 10: Add to all nine dispatch arms (sites 9-17)**

Find each of the nine `EffectInstance` methods and add the `SpectralRotate` arm after the corresponding `Vocoder` arm. The arms:

```rust
// kind()
EffectInstance::SpectralRotate(_) => EffectKind::SpectralRotate,

// process_sample
EffectInstance::SpectralRotate(e) => e.process_sample(left, right),

// set_sample_rate
EffectInstance::SpectralRotate(e) => e.set_sample_rate(sample_rate),

// reset
EffectInstance::SpectralRotate(e) => e.reset(),

// parameters
EffectInstance::SpectralRotate(e) => e.parameters(),

// set_param
EffectInstance::SpectralRotate(e) => e.set_param(index, value),

// set_bpm
EffectInstance::SpectralRotate(e) => e.set_bpm(bpm),

// param_dimmed
EffectInstance::SpectralRotate(e) => e.param_dimmed(index),

// latency_samples
EffectInstance::SpectralRotate(e) => e.latency_samples(),
```

If `Effect` doesn't have `set_bpm` or `param_dimmed` methods, omit those arms — check by reading `Effect`'s definition in `mod.rs`. (Looking at the existing arms in the workspace will tell you which methods Effect actually carries.)

- [ ] **Step 11: Update `EffectKind::ALL.len() == 22` test (site 18)**

Around line 869 there's `assert_eq!(EffectKind::ALL.len(), 22);`. Change to `23`.

- [ ] **Step 12: Add `effect_kind_name_matches` assertion (site 19)**

Around line 891 the test enumerates names. After the `EffectKind::Vocoder.name() == "Vocoder"` assertion, add:

```rust
        assert_eq!(EffectKind::SpectralRotate.name(), "Spectral Rotate");
```

- [ ] **Step 13: Build the crate**

```bash
cargo build -p multosis
```

Expected: clean build, no warnings.

- [ ] **Step 14: Run the new effect's tests**

```bash
cargo nextest run -p multosis spectral_rotate::tests
```

Expected: 5 tests pass.

- [ ] **Step 15: Workspace gate**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
```

Expected: green clippy, all tests pass (workspace total bumps by 5), bundles built (standalone + CLAP + VST3).

- [ ] **Step 16: Commit**

```bash
git add multosis/Cargo.toml multosis/src/effects/spectral_rotate.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralRotate effect -- circular bin shift

First effect in the Infiltrator-inspired spectral family. Rotates the
positive-half spectrum modulo N/2; the negative half is rebuilt from
conjugates so the output stays real. FFT size selectable 512/1024/
2048/4096; modulating it gets you what you get.

Establishes the per-effect template for the rest of the spectral
family (SpectralBandpass, Mirror, Shift, Spread, Lofi, ...). Each
follows the same shape: ParamsCache + TransformCtx + Engine, plus
14 dispatch sites in mod.rs.

EffectKind::ALL grows from 22 to 23.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: SpectralBandpass

Brickwall bandpass: zero all bins outside [Freq * 2^(-bw/2), Freq * 2^(bw/2)].

**Files:**
- Create: `multosis/src/effects/spectral_bandpass.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Create `multosis/src/effects/spectral_bandpass.rs`:

```rust
//! Spectral Bandpass: FFT-based brickwall bandpass filter.
//!
//! Zeros every bin outside [Freq * 2^(-bw/2), Freq * 2^(bw/2)]. No smoothing
//! at the edges -- Infiltrator calls this "brickwall".

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform, FFT_SIZES};

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    freq_hz: f32,
    bw_oct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self { fft_param: 2.0, freq_hz: 1000.0, bw_oct: 1.0 }
    }
}

struct TransformCtx {
    freq_hz: f32,
    bw_oct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let low_hz = self.freq_hz * (-(self.bw_oct * 0.5)).exp2();
        let high_hz = self.freq_hz * (self.bw_oct * 0.5).exp2();
        let low_bin = (low_hz / bin_hz).floor() as i32;
        let high_bin = (high_hz / bin_hz).ceil() as i32;
        for k in 0..=half {
            if (k as i32) < low_bin || (k as i32) > high_bin {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

pub struct SpectralBandpassEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralBandpassEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "FFT", min: 0.0, max: 3.0, default: 2.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum { labels: &["512", "1024", "2048", "4096"] },
        },
        ParamSpec {
            name: "Freq", min: 20.0, max: 20_000.0, default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Width", min: 0.1, max: 4.0, default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number { decimals: 2, unit: " oct" },
        },
    ];
}

impl Default for SpectralBandpassEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralBandpassEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx { freq_hz: self.params.freq_hz, bw_oct: self.params.bw_oct };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx { freq_hz: self.params.freq_hz, bw_oct: self.params.bw_oct };
        let ro = self.engine_r.process_sample(right, &mut ctx_r);
        (lo, ro)
    }
    fn parameters(&self) -> &'static [ParamSpec] { &Self::PARAMS }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.engine_l = SpectralEngine::new(sample_rate);
        self.engine_r = SpectralEngine::new(sample_rate);
        let fft_size = FFT_SIZES[self.params.fft_param.round().clamp(0.0, 3.0) as usize];
        self.engine_l.set_fft_size(fft_size);
        self.engine_r.set_fft_size(fft_size);
    }
    fn reset(&mut self) { self.engine_l.reset(); self.engine_r.reset(); }
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
            1 => self.params.freq_hz = value.clamp(20.0, 20_000.0),
            2 => self.params.bw_oct = value.clamp(0.1, 4.0),
            _ => {}
        }
    }
    fn latency_samples(&self) -> usize { self.engine_l.latency_samples() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(e: &mut SpectralBandpassEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n).map(|i| { let x = src(i); e.process_sample(x, x).0 }).collect()
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralBandpassEffect::default();
        e.set_param(1, 5000.0);
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn narrow_band_kills_out_of_band_content() {
        let sr = 48_000.0;
        let mut e = SpectralBandpassEffect::default();
        e.set_param(0, 1.0); // FFT = 1024
        e.set_param(1, 1000.0);
        e.set_param(2, 0.5); // half-octave
        // Drive a 5 kHz sine -- well outside the passband.
        let out = drive(&mut e, 4096, |i| (2.0 * std::f32::consts::PI * 5000.0 * i as f32 / sr).sin());
        let tail: Vec<f32> = out[2 * e.latency_samples()..].into();
        let peak = tail.iter().cloned().fold(0.0_f32, f32::max);
        assert!(peak < 0.1, "expected out-of-band 5 kHz to be attenuated to ~0, got peak {peak}");
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralBandpassEffect::default().parameters().len(), 3);
    }
}
```

(Note: `ParamScaling::Log` is used by existing effects for Hz params — search `multosis/src/effects/` for `Logarithmic` to confirm spelling and that it's the correct variant. If it's named differently, use whatever Hz-scaled effects like `Svf` use.)

- [ ] **Step 2: Add the 14 mod.rs sites**

In `multosis/src/effects/mod.rs`, find each existing `Vocoder` / `SpectralRotate` site and add the `SpectralBandpass` analog directly after the `SpectralRotate` line. Lines to add at each site:

```rust
// site 1: mod
mod spectral_bandpass;
// site 2: pub use
pub use spectral_bandpass::SpectralBandpassEffect;
// site 3: EffectKind variant
    /// Spectral Bandpass -- brickwall FFT bandpass filter.
    SpectralBandpass,
// site 4: ALL array (also bump len: 23 -> 24)
        EffectKind::SpectralBandpass,
// site 5: name()
            EffectKind::SpectralBandpass => "Spectral Bandpass",
// site 6: reports_latency() -- add to matches!
        // matches!(self, Self::Satch | Self::WarpZone | Self::SpectralRotate | Self::SpectralBandpass)
// site 7: EffectInstance variant
    SpectralBandpass(Box<SpectralBandpassEffect>),
// site 8: EffectInstance::new
            EffectKind::SpectralBandpass => EffectInstance::SpectralBandpass(Box::default()),
// sites 9-17 (all dispatch arms): wherever a `SpectralRotate` arm exists, add the SpectralBandpass equivalent
EffectInstance::SpectralBandpass(_) => EffectKind::SpectralBandpass,
EffectInstance::SpectralBandpass(e) => e.process_sample(left, right),
EffectInstance::SpectralBandpass(e) => e.set_sample_rate(sample_rate),
EffectInstance::SpectralBandpass(e) => e.reset(),
EffectInstance::SpectralBandpass(e) => e.parameters(),
EffectInstance::SpectralBandpass(e) => e.set_param(index, value),
EffectInstance::SpectralBandpass(e) => e.set_bpm(bpm),       // omit if Effect lacks set_bpm
EffectInstance::SpectralBandpass(e) => e.param_dimmed(index), // omit if Effect lacks this
EffectInstance::SpectralBandpass(e) => e.latency_samples(),
// site 18: bump ALL.len() assertion to 24
// site 19: add assert_eq!(EffectKind::SpectralBandpass.name(), "Spectral Bandpass");
```

- [ ] **Step 3: Build + test + bundle (workspace gate)**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
```

Expected: green clippy, all workspace tests pass (count bumps by 3), bundle built.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_bandpass.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralBandpass effect -- brickwall FFT bandpass

Zeros every bin outside [Freq * 2^(-Width/2), Freq * 2^(Width/2)].
No smoothing -- Infiltrator calls this brickwall. Three params:
FFT size, Freq (20-20k Hz log), Width (0.1-4 oct linear).

EffectKind::ALL grows from 23 to 24.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: SpectralMirror

Flip a portion of the spectrum around a centre frequency.

**Files:**
- Create: `multosis/src/effects/spectral_mirror.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Create `multosis/src/effects/spectral_mirror.rs` with the same skeleton as `spectral_bandpass.rs` except for the `transform` body and the param details. Two params after FFT: `Freq` (20-20k Hz log, default 1000), `Width` (0.1-4 oct, default 1.0). Replace the transform body with:

```rust
impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins = ((self.freq_hz * (self.bw_oct * 0.5).exp2() - self.freq_hz) / bin_hz)
            .ceil() as i32;
        // For each offset d in (0..=band_bins], swap centre+d with centre-d.
        // Conjugate the mirrored bin so phase is reflected, not duplicated.
        // Mirror only the positive half; the negative half is rebuilt below.
        for d in 1..=band_bins {
            let kp = centre_bin + d;
            let kn = centre_bin - d;
            if kn > 0 && (kp as usize) < half {
                let a = spectrum[kp as usize];
                let b = spectrum[kn as usize];
                spectrum[kp as usize] = b.conj();
                spectrum[kn as usize] = a.conj();
            }
        }
        // Rebuild negative half from conjugate-mirror of positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}
```

Tests:
- `silence_in_silence_out` (same shape as Bandpass).
- `parameters_count_is_three`.
- `mirror_swaps_test_tones`: drive a 500 Hz sine, mirror around 1000 Hz with 2-octave width; expect tail energy near 2000 Hz (verify by forward-FFT of the tail, same pattern as Rotate's `shift_positive_moves_energy_up` test).

`PARAMS`: `["FFT", "Freq", "Width"]`.

- [ ] **Step 2: Add the 14 mod.rs sites for `SpectralMirror`**

Same recipe as Task 3 — insert after the `SpectralBandpass` lines. Bump `ALL.len()` to 25. Add the name assertion `"Spectral Mirror"`. Add to `reports_latency` matches.

- [ ] **Step 3: Workspace gate**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
```

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_mirror.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralMirror effect -- in-band spectrum flip

Mirrors bins within +/- Width/2 octaves of Freq around the centre.
Conjugate-swap preserves real-output (phase reflected, not just
copied). EffectKind::ALL grows from 24 to 25.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: SpectralShift (Translate-only path)

Per-bin frequency translate by ±100% of Nyquist. Out-of-range bins zeroed. The `Scale` parameter is declared but unused in this task — Task 14 fills in the Scale interpolation path. Doing it in two steps keeps each commit small.

**Files:**
- Create: `multosis/src/effects/spectral_shift.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Create `multosis/src/effects/spectral_shift.rs` with the standard skeleton. Three params after FFT: `Scale` (0.5-2.0 default 1.0; unused in this task), `Translate` (-100..+100% default 0). Transform body:

```rust
impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sample_rate: f32) {
        let half = fft_size as i32 / 2;
        let translate_bins = ((self.translate_pct * 0.01) * half as f32).round() as i32;
        if translate_bins == 0 {
            return;
        }
        // Buffer-then-write the positive half. Use the negative half as
        // scratch (we rebuild it from conjugates at the end).
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k];
        }
        for k in 1..half {
            let src = k - translate_bins;
            spectrum[k as usize] = if (1..half).contains(&src) {
                spectrum[fft_size - src as usize]
            } else {
                Complex::default()
            };
        }
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}
```

`PARAMS`: `["FFT", "Scale", "Translate"]`. `Scale` param is accepted by `set_param(1, v)` and cached in `ParamsCache.scale` — but the transform IGNORES it for now. A `// TODO(Task 14)` comment marks the spot.

Tests:
- `translate_zero_is_passthrough`
- `silence_in_silence_out`
- `translate_positive_moves_energy_up` (same pattern as Rotate test 5 but with the un-wrapped variant — content shifted past Nyquist should be zeroed; mid-band content moves up cleanly).

- [ ] **Step 2: Add the 14 mod.rs sites for `SpectralShift`**

Same recipe. Bump `ALL.len()` to 26. Add `"Spectral Shift"` name assertion.

- [ ] **Step 3: Workspace gate**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
```

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_shift.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralShift effect -- Translate path (Scale TODO)

Per-bin frequency translate; out-of-range bins zero (unlike Rotate's
wrap). Scale param is declared and accepted but doesn't yet affect
the transform -- Task 14 fills in the linear-interp Scale path.

EffectKind::ALL grows from 25 to 26.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: SpectralSpread

Box-blur magnitude across bins. Phase preserved per-bin. Kernel radius capped at 16 so Amount=100% is detail-softening, not spectrum-smashing.

**Files:**
- Create: `multosis/src/effects/spectral_spread.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Same skeleton, one param after FFT: `Amount` (0-100% default 0). The transform needs a scratch buffer to hold pre-blur magnitudes (the in-place blur would corrupt the source). Add a `mags: Vec<f32>` field on `SpectralSpreadChannel`, pre-allocated to `4096 / 2 + 1` (the max half-spectrum size).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let radius = ((self.amount_pct * 0.01) * 16.0).round() as i32;
        if radius == 0 { return; }
        // Snapshot magnitudes.
        let mags = &mut self.chan.mags[..=half];
        for k in 0..=half {
            mags[k] = spectrum[k].norm();
        }
        // Box-blur write: each bin's new magnitude = mean of mags within radius.
        for k in 0..=half {
            let lo = (k as i32 - radius).max(0) as usize;
            let hi = (k as i32 + radius).min(half as i32) as usize;
            let sum: f32 = mags[lo..=hi].iter().sum();
            let new_mag = sum / (hi - lo + 1) as f32;
            let old_mag = mags[k].max(1e-20);
            let ratio = new_mag / old_mag;
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}
```

This uses the per-channel scratch pattern from the skeleton (TransformCtx borrows `&mut SpectralSpreadChannel`). Match the skeleton's per-channel state shape exactly.

`PARAMS`: `["FFT", "Amount"]`.

Tests:
- `amount_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_two`

- [ ] **Step 2: 14 mod.rs sites for `SpectralSpread`**

Same recipe. `ALL.len()` to 27.

- [ ] **Step 3: Workspace gate.** Same gate.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_spread.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralSpread effect -- magnitude box-blur

Amount knob controls box-blur radius (0-16 bins). Phase preserved
per-bin. Kernel cap keeps Amount=100% detail-softening rather than
spectrum-smashing. EffectKind::ALL grows from 26 to 27.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: SpectralLofi

Decimate bins by a Factor with optional randomisation, updated every Slow hops.

**Files:**
- Create: `multosis/src/effects/spectral_lofi.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Skeleton with per-channel state:

```rust
struct SpectralLofiChannel {
    keep_mask: Vec<bool>,    // pre-allocated to max half (2048+1) entries
    hop_counter: u32,
    rng_state: u32,          // xorshift state
}
```

Three params after FFT: `Factor` (0-100% default 0), `Randomise` (0-100% default 0), `Slow` (1-100 default 1).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let factor = (self.params.factor_pct * 0.01).clamp(0.0, 1.0);
        if factor <= 1e-6 { return; }
        let randomise = (self.params.randomise_pct * 0.01).clamp(0.0, 1.0);
        let slow = self.params.slow.clamp(1, 100) as u32;

        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= slow {
            self.chan.hop_counter = 0;
            // Rebuild keep_mask. Regular decimation at randomise=0, full
            // random at randomise=1, lerp in between.
            let step = (1.0 / (1.0 - factor)).max(1.0);
            for k in 0..=half {
                // Regular vote: keep iff k mod step == 0.
                let regular = ((k as f32) % step) < 0.5;
                // Random vote: probability (1 - factor).
                self.chan.rng_state = xorshift(self.chan.rng_state);
                let r = (self.chan.rng_state as f32) / (u32::MAX as f32);
                let random_vote = r > factor;
                // Lerp: P(keep) interpolates from "regular" to random_vote.
                let p = (1.0 - randomise) * (regular as i32 as f32) + randomise * (random_vote as i32 as f32);
                self.chan.keep_mask[k] = p > 0.5;
            }
        }
        // Apply mask.
        for k in 0..=half {
            if !self.chan.keep_mask[k] {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13; s ^= s >> 17; s ^= s << 5; s.max(1)
}
```

The `keep_mask` is initialised to all-true in `Channel::new` so a fresh effect at default params is passthrough until the first hop after `Factor>0`.

`PARAMS`: `["FFT", "Factor", "Randomise", "Slow"]`. `Slow` param uses `ParamFormat::Number { decimals: 0, unit: " hops" }`.

Tests:
- `factor_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_four`

- [ ] **Step 2: 14 mod.rs sites for `SpectralLofi`**. `ALL.len()` to 28.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_lofi.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralLofi effect -- bin decimation with randomness

Bitmask of kept bins refreshes every Slow hops. Factor controls
fraction zeroed; Randomise interpolates between regular decimation
(0%) and independent random keep (100%). xorshift RNG keeps the
audio thread allocation-free. EffectKind::ALL grows 27 -> 28.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: SpectralSmear

Per-bin envelope follower: instant attack, release tau = Length. Chaos randomises phase.

**Files:**
- Create: `multosis/src/effects/spectral_smear.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state:

```rust
struct SpectralSmearChannel {
    last_mag: Vec<f32>,    // pre-allocated to max half + 1
    rng_state: u32,
}
```

Two params after FFT: `Length` (10-2000 ms default 200), `Chaos` (0-100% default 0).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_samples = (self.params.length_ms * 0.001 * sample_rate).max(1.0);
        // tau release per hop: gain decays by exp(-hop_samples / length_samples).
        let decay = (-hop_samples / length_samples).exp();
        let chaos = (self.params.chaos_pct * 0.01).clamp(0.0, 1.0);

        for k in 0..=half {
            let mag_in = spectrum[k].norm();
            let mag_held = self.chan.last_mag[k] * decay;
            let mag = mag_in.max(mag_held);
            self.chan.last_mag[k] = mag;
            // Reconstruct bin with the held magnitude. Phase: original phase
            // plus a Chaos-amount random rotation.
            let phase_in = spectrum[k].im.atan2(spectrum[k].re);
            self.chan.rng_state = xorshift(self.chan.rng_state);
            let r = (self.chan.rng_state as f32) / (u32::MAX as f32) - 0.5;
            let phase = phase_in + chaos * std::f32::consts::TAU * r;
            spectrum[k] = Complex::new(mag * phase.cos(), mag * phase.sin());
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13; s ^= s >> 17; s ^= s << 5; s.max(1)
}
```

`PARAMS`: `["FFT", "Length", "Chaos"]`.

Tests:
- `silence_in_silence_out`
- `parameters_count_is_three`
- `tail_after_pulse_decays_over_length`: drive a single-frame burst of noise, then silence; assert that for `Length=500ms` there's still output 200 ms after the burst and that the output magnitude is below the burst magnitude.

- [ ] **Step 2: 14 mod.rs sites for `SpectralSmear`**. `ALL.len()` to 29.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_smear.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralSmear effect -- per-bin magnitude hold

Instant-attack release-tau envelope per bin. Length controls tau
in ms; Chaos randomises bin phase. Allocation-free xorshift RNG
on the audio thread. EffectKind::ALL grows 28 -> 29.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: SpectralCorrupt

Rank bins by magnitude; zero the quietest (positive Amount) or loudest (negative Amount) fraction. Decay carries the last frame's gate decision to soften cuts.

**Files:**
- Create: `multosis/src/effects/spectral_corrupt.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state needs scratch for ranking AND a per-bin gain memory for Decay:

```rust
struct SpectralCorruptChannel {
    bin_indices: Vec<u16>,   // pre-allocated to max half + 1; used for sort
    bin_gains: Vec<f32>,     // last-frame gain per bin
}
```

Two params after FFT: `Amount` (-100..+100% default 0), `Decay` (0-100% default 0).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let amt = self.params.amount_pct * 0.01;
        if amt.abs() < 1e-3 {
            // Still decay the previous frame's gates back toward 1.
            let d = (self.params.decay_pct * 0.01).clamp(0.0, 0.99);
            for k in 0..=half {
                self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + 1.0 * (1.0 - d);
                spectrum[k].re *= self.chan.bin_gains[k];
                spectrum[k].im *= self.chan.bin_gains[k];
                if k != 0 && k != half {
                    spectrum[fft_size - k] = spectrum[k].conj();
                }
            }
            return;
        }
        // Rank by magnitude.
        let count = half + 1;
        for k in 0..count { self.chan.bin_indices[k] = k as u16; }
        self.chan.bin_indices[..count].sort_unstable_by(|&a, &b| {
            spectrum[a as usize].norm().partial_cmp(&spectrum[b as usize].norm())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        // amt > 0 zeros the |amt|*count quietest; amt < 0 zeros the loudest.
        let cut = ((amt.abs() * count as f32).round() as usize).min(count);
        let zero_set = if amt > 0.0 {
            &self.chan.bin_indices[..cut]
        } else {
            &self.chan.bin_indices[count - cut..count]
        };
        // Build a target gain per bin: 0 if in zero set, 1 otherwise.
        // Apply Decay = exponential carry of last frame's gain toward target.
        let d = (self.params.decay_pct * 0.01).clamp(0.0, 0.99);
        // First reset all to keep=1.
        let target = vec_keep_mask(zero_set, count);
        for k in 0..count {
            let t = target[k];
            self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + t * (1.0 - d);
            spectrum[k].re *= self.chan.bin_gains[k];
            spectrum[k].im *= self.chan.bin_gains[k];
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}
```

The `target` allocation above is illegal on the audio thread. Replace with a scratch `Vec<f32>` field on `SpectralCorruptChannel`: `target_buf: Vec<f32>` pre-allocated. Write it in place using `bin_indices`:

```rust
self.chan.target_buf[..count].fill(1.0);
for &k in zero_set { self.chan.target_buf[k as usize] = 0.0; }
```

Then loop over `target_buf` to apply.

`PARAMS`: `["FFT", "Amount", "Decay"]`.

Tests:
- `amount_zero_with_decay_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_three`

- [ ] **Step 2: 14 mod.rs sites for `SpectralCorrupt`**. `ALL.len()` to 30.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_corrupt.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralCorrupt effect -- partial removal by magnitude

Amount > 0 zeros the quietest |Amount|% bins; Amount < 0 zeros the
loudest. Decay carries the previous frame's per-bin gain toward the
new target so cuts/restores feel smooth. Bin-index scratch is
pre-allocated to keep the audio thread allocation-free.
EffectKind::ALL grows 29 -> 30.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 10: SpectralCompress

Per-bin compression toward a target spectrum interpolated between pink (1/f) and white (f).

**Files:**
- Create: `multosis/src/effects/spectral_compress.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state caches the running magnitude estimate (one-pole follower per bin) so the compression target reacts to the long-term spectrum, not just the instantaneous frame:

```rust
struct SpectralCompressChannel {
    avg_mag: Vec<f32>,
}
```

Two params after FFT: `Amount` (0-100% default 0), `Tone` (-100..+100% default 0).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let amount = (self.params.amount_pct * 0.01).clamp(0.0, 1.0);
        if amount < 1e-3 { return; }
        let tone = (self.params.tone_pct * 0.01).clamp(-1.0, 1.0); // -1 = pink, +1 = white
        let bin_hz = sample_rate / fft_size as f32;
        // One-pole follower coefficient (~50 ms attack/release at hop rate).
        let hop_samples = (fft_size / 2) as f32;
        let tau_samples = 0.050 * sample_rate;
        let alpha = (-hop_samples / tau_samples).exp();
        for k in 1..=half {
            let m = spectrum[k].norm();
            self.chan.avg_mag[k] = self.chan.avg_mag[k] * alpha + m * (1.0 - alpha);
            // Target: pink ~ 1/f, white ~ f, flat between. Interpolate the
            // exponent linearly so tone=-1 gives ~ k^-1, tone=+1 gives ~ k.
            let f_hz = k as f32 * bin_hz;
            let f_norm = (f_hz / 1000.0).max(1e-3); // ref at 1 kHz
            let target = f_norm.powf(tone);
            let current = self.chan.avg_mag[k].max(1e-12);
            let ratio = (target / current).powf(amount);
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}
```

`PARAMS`: `["FFT", "Amount", "Tone"]`.

Tests:
- `amount_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_three`

- [ ] **Step 2: 14 mod.rs sites for `SpectralCompress`**. `ALL.len()` to 31.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_compress.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralCompress effect -- per-bin compression to target

Per-bin one-pole envelope tracks each bin's magnitude (~50 ms tau).
Target = f^tone (tone=-1 pink, 0 flat, +1 white). Ratio applies
target/current raised to Amount. EffectKind::ALL grows 30 -> 31.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 11: SpectralCascade

Per-bin delay = Length * (k - centre_bin) / (N/2). Linear ramp around centre. Feedback.

**Files:**
- Create: `multosis/src/effects/spectral_cascade.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state: a per-bin ring buffer of complex values, depth = max Length in hops at smallest FFT. Worst case: 2000 ms at hop=256 = ~375 hops. Round up to 512. Each bin has its own write head into the shared buffer.

```rust
/// Max number of hops we'll buffer per bin (Length max = 2000 ms at smallest
/// FFT hop = 256 samples = 187 hops at 48 kHz; round up + headroom = 512).
const MAX_DELAY_HOPS: usize = 512;
const MAX_HALF: usize = 4096 / 2 + 1; // largest spectrum half-size

struct SpectralCascadeChannel {
    /// 2D buffer indexed [hop][bin] flattened. ring[h * MAX_HALF + k].
    ring: Vec<Complex<f32>>,
    write_pos: usize, // hop index
}

impl SpectralCascadeChannel {
    fn new(_sr: f32) -> Self {
        Self {
            ring: vec![Complex::default(); MAX_DELAY_HOPS * MAX_HALF],
            write_pos: 0,
        }
    }
    fn reset(&mut self) {
        self.ring.fill(Complex::default());
        self.write_pos = 0;
    }
}
```

Three params after FFT: `Length` (10-2000 ms default 200), `Feedback` (0-95% default 0), `Centre` (20-20k Hz log default 1000).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let hop_samples = (fft_size / 2) as f32;
        let length_hops = ((self.params.length_ms * 0.001 * sample_rate) / hop_samples).round() as i32;
        let centre_bin = (self.params.centre_hz / bin_hz).round() as i32;
        let feedback = (self.params.feedback_pct * 0.01).clamp(0.0, 0.95);

        // Read delayed input + apply feedback + write new sample to the ring
        // at write_pos.
        let h_now = self.chan.write_pos;
        for k in 1..=half {
            let dk = ((k as i32 - centre_bin) * length_hops / (half as i32)).max(0);
            let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
            let delayed = self.chan.ring[read_h * MAX_HALF + k];
            // Output = input + delayed (the "cascade" sound); ring stores
            // input + feedback * delayed so taps come back round.
            let written = spectrum[k] + Complex::new(feedback * delayed.re, feedback * delayed.im);
            self.chan.ring[h_now * MAX_HALF + k] = written;
            spectrum[k] = delayed;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        self.chan.write_pos = (h_now + 1) % MAX_DELAY_HOPS;
    }
}
```

`PARAMS`: `["FFT", "Length", "Feedback", "Centre"]`.

Tests:
- `length_zero_feedback_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_four`

Note: SpectralCascade's ring is ~1 MB per channel (512 * 2049 complex * 8 bytes ≈ 8 MB!). That's too much per instance. Either:
- (a) Reduce MAX_HALF to a smaller bound (e.g. use the full N/2+1 only when needed; alternatively store mag+phase as 2 f32s = same size).
- (b) Dynamic allocation in `set_sample_rate` (allowed — not on audio thread) sized to actual FFT size's half.

Pick (b): the ring is allocated in `set_sample_rate` and resized to `MAX_DELAY_HOPS * (current_fft_size / 2 + 1)`. Changing FFT size triggers a re-alloc in `set_param(0, ..)`. Since set_param IS called from the audio thread in multosis (when modulating), this won't work either.

Better: allocate once at `set_sample_rate` for the LARGEST FFT size only. That's `512 * 2049 * 8 = 8.4 MB per channel × 2 = 16.8 MB per Cascade instance`. Too much.

Acceptable workaround: cap `MAX_DELAY_HOPS` at 128 (still gives 2000ms at 4096-pt hop=2048; at smaller FFTs Length is clamped to what the buffer supports). 128 * 2049 * 8 = 2 MB per channel = 4 MB per Cascade instance. Acceptable.

Revise the constants:

```rust
const MAX_DELAY_HOPS: usize = 128;
```

And in the transform body, clamp `length_hops` to `MAX_DELAY_HOPS - 1` before use, so over-long Lengths just saturate rather than wrap.

- [ ] **Step 2: 14 mod.rs sites for `SpectralCascade`**. `ALL.len()` to 32.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_cascade.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralCascade effect -- per-bin delay ramped around centre

Per-bin complex delay line, depth = 128 hops (saturates Length at long
hops). Linear ramp: delay_k = Length * (k - centre_bin) / (N/2).
Negative delays clamp to 0 (read 'now'). Feedback recycles delayed
values. EffectKind::ALL grows 31 -> 32.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 12: SpectralReverb

Per-bin feedback with per-frequency T60.

**Files:**
- Create: `multosis/src/effects/spectral_reverb.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state: a per-bin one-pole IIR storing the running magnitude. Simpler than Cascade — no ring needed.

```rust
struct SpectralReverbChannel {
    tail: Vec<Complex<f32>>, // pre-allocated to MAX_HALF
}
```

Two params after FFT: `Time` (0.1-20 s default 2.0, log scaling), `Tone` (0-100% default 50, linear; 0 = dark, 1 = bright).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let hop_samples = (fft_size / 2) as f32;
        let time_s = self.params.time_s.max(0.001);
        let tone = (self.params.tone_pct * 0.01).clamp(0.0, 1.0);

        // Per-bin T60 curve: a HF-rolloff at tone=0, flat at tone=0.5,
        // LF-rolloff at tone=1. Implementation: log-frequency-weighted lerp.
        for k in 1..=half {
            let f_hz = (k as f32 * bin_hz).max(1.0);
            // Normalised 0..1: 0 at 20 Hz, 1 at 20 kHz (in log-space).
            let f_norm = ((f_hz / 20.0).ln() / (20_000.0_f32 / 20.0).ln()).clamp(0.0, 1.0);
            // Damping at this bin: 1 = full Time, 0.1 = 10% of Time.
            let bright_damping = 1.0 - 0.9 * (1.0 - f_norm);  // bright: LF gets 0.1*Time
            let dark_damping   = 1.0 - 0.9 * f_norm;           // dark:   HF gets 0.1*Time
            let damp = (1.0 - tone) * dark_damping + tone * bright_damping;
            let t60_k = time_s * damp;
            // Per-hop decay coefficient: g^(hop / t60_in_hops) = 10^(-3/20)
            // => g = 10^(-3 hop_samples / (20 * t60_k * sample_rate)).
            let g = (-3.0 * hop_samples / (20.0 * t60_k * sample_rate)).exp();
            self.chan.tail[k].re = self.chan.tail[k].re * g + spectrum[k].re;
            self.chan.tail[k].im = self.chan.tail[k].im * g + spectrum[k].im;
            spectrum[k] = self.chan.tail[k];
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}
```

`PARAMS`: `["FFT", "Time", "Tone"]`.

Tests:
- `silence_in_silence_out` (after pre-existing tail decays).
- `parameters_count_is_three`.
- `tail_extends_past_input`: drive a 1-sample impulse, observe non-zero output 100 ms later (after warmup).

- [ ] **Step 2: 14 mod.rs sites for `SpectralReverb`**. `ALL.len()` to 33.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_reverb.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralReverb effect -- per-bin feedback with Tone curve

Per-bin one-pole feedback. Tone shapes per-frequency T60: dark
damps HF (Tone=0), bright damps LF (Tone=1), flat at Tone=0.5.
EffectKind::ALL grows 32 -> 33.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 13: SpectralScatter

Per-bin random delays, refreshed at Rate Hz.

**Files:**
- Create: `multosis/src/effects/spectral_scatter.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

Per-channel state: per-bin delay line (like Cascade) PLUS a per-bin delay assignment vector (random hops 0..length_hops) PLUS an RNG state. Reuse the `MAX_DELAY_HOPS = 128` cap from Cascade.

```rust
struct SpectralScatterChannel {
    ring: Vec<Complex<f32>>,
    write_pos: usize,
    delay_per_bin: Vec<u16>,  // 0..MAX_DELAY_HOPS
    hop_counter: u32,
    rng_state: u32,
}
```

Three params after FFT: `Length` (10-2000 ms default 200), `Feedback` (0-95% default 0), `Rate` (0.1-10 Hz default 1.0, log scaling).

Transform body:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_hops = (((self.params.length_ms * 0.001 * sample_rate) / hop_samples).round()
            as usize).min(MAX_DELAY_HOPS - 1).max(1);
        let feedback = (self.params.feedback_pct * 0.01).clamp(0.0, 0.95);
        // Rate -> hops per refresh.
        let hops_per_refresh = ((sample_rate / hop_samples) / self.params.rate_hz.max(0.001))
            .round() as u32;

        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= hops_per_refresh {
            self.chan.hop_counter = 0;
            for k in 1..=half {
                self.chan.rng_state = xorshift(self.chan.rng_state);
                self.chan.delay_per_bin[k] = (self.chan.rng_state as usize % length_hops) as u16;
            }
        }

        let h_now = self.chan.write_pos;
        for k in 1..=half {
            let dk = self.chan.delay_per_bin[k] as i32;
            let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
            let delayed = self.chan.ring[read_h * MAX_HALF + k];
            let written = spectrum[k] + Complex::new(feedback * delayed.re, feedback * delayed.im);
            self.chan.ring[h_now * MAX_HALF + k] = written;
            spectrum[k] = delayed;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        self.chan.write_pos = (h_now + 1) % MAX_DELAY_HOPS;
    }
}
fn xorshift(mut s: u32) -> u32 { s ^= s << 13; s ^= s >> 17; s ^= s << 5; s.max(1) }
```

(`MAX_DELAY_HOPS` and `MAX_HALF` are duplicated from Cascade — extract both into a small `pub mod spectral_engine_const { pub const MAX_DELAY_HOPS: usize = 128; pub const MAX_HALF: usize = 4096 / 2 + 1; }` in `multosis/src/effects/mod.rs` so the two effects share, OR copy locally. The copy is fine for now; deduplication can happen post-merge.)

`PARAMS`: `["FFT", "Length", "Feedback", "Rate"]`.

Tests:
- `silence_in_silence_out`
- `parameters_count_is_four`

- [ ] **Step 2: 14 mod.rs sites for `SpectralScatter`**. `ALL.len()` to 34.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_scatter.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralScatter effect -- random per-bin delays

Per-bin random delay (0..Length hops), reassigned at Rate Hz.
xorshift RNG keeps the audio thread allocation-free. EffectKind::ALL
grows 33 -> 34.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 14: SpectralShift -- complete the Scale path

Fill in the linear-interpolation Scale path that was stubbed in Task 5.

**Files:**
- Modify: `multosis/src/effects/spectral_shift.rs`

- [ ] **Step 1: Update the transform**

Replace `SpectralShift`'s `transform` body with one that handles both Scale and Translate:

```rust
impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size as i32 / 2;
        let scale = self.params.scale.clamp(0.5, 2.0);
        let translate_bins = ((self.params.translate_pct * 0.01) * half as f32);
        let identity = (scale - 1.0).abs() < 1e-6 && translate_bins.abs() < 0.5;
        if identity { return; }

        // Stash positive half.
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k];
        }
        for k in 1..half {
            // Source position in the stashed positive half (fractional).
            let src = (k as f32 - translate_bins) / scale;
            let src_floor = src.floor() as i32;
            let src_ceil = src.ceil() as i32;
            let frac = src - src_floor as f32;
            let pick = |s: i32| -> Complex<f32> {
                if (1..half).contains(&s) {
                    self_scratch_read(spectrum, fft_size, s as usize)
                } else {
                    Complex::default()
                }
            };
            let a = pick(src_floor);
            let b = pick(src_ceil);
            spectrum[k as usize] = Complex::new(
                a.re * (1.0 - frac) + b.re * frac,
                a.im * (1.0 - frac) + b.im * frac,
            );
        }
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

#[inline]
fn self_scratch_read(spectrum: &[Complex<f32>], fft_size: usize, k: usize) -> Complex<f32> {
    spectrum[fft_size - k]
}
```

Add a `scale` field to `ParamsCache` and the `Scale` arm to `set_param`. Remove the `// TODO(Task 14)` comment.

- [ ] **Step 2: Add a Scale test**

```rust
#[test]
fn scale_2_doubles_frequency_content() {
    // A 500 Hz sine fed through Scale=2 should produce most of its energy
    // around 1 kHz.
    use rustfft::num_complex::Complex;
    let sr = 48_000.0_f32;
    let mut e = SpectralShiftEffect::default();
    e.set_param(0, 1.0); // FFT 1024
    e.set_param(1, 2.0); // Scale
    e.set_param(2, 0.0); // Translate
    let n = 8192_usize;
    let out: Vec<f32> = (0..n).map(|i| {
        let x = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr).sin();
        e.process_sample(x, x).0
    }).collect();
    let tail_start = 2 * e.latency_samples();
    let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
        .iter().map(|&x| Complex::new(x, 0.0)).collect();
    let mut planner = rustfft::FftPlanner::<f32>::new();
    planner.plan_fft_forward(2048).process(&mut tail);
    // Energy around 1 kHz should exceed energy around 500 Hz.
    let bin_500 = (500.0 * 2048.0 / sr).round() as usize;
    let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
    let e_500 = tail[bin_500.saturating_sub(2)..=bin_500 + 2].iter().map(|c| c.norm_sqr()).sum::<f32>();
    let e_1k = tail[bin_1k.saturating_sub(2)..=bin_1k + 2].iter().map(|c| c.norm_sqr()).sum::<f32>();
    assert!(e_1k > e_500, "expected energy at 1 kHz > 500 Hz; got 1k={e_1k} 500={e_500}");
}
```

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_shift.rs
git commit -m "feat(multosis): SpectralShift -- complete Scale path

Linear-interp source-bin pickup. Scale=1 + Translate=0 short-circuits
to identity. Adds the scale_2_doubles_frequency_content test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 15: SpectralTwist

Within ±Bandwidth/2 oct of Freq: scale each bin's distance-from-centre by (1 − Twist).

**Files:**
- Create: `multosis/src/effects/spectral_twist.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

No per-channel state — the transform is stateless given params. Three params after FFT: `Freq` (20-20k Hz log default 1000), `Twist` (-100..+100% default 0), `Bandwidth` (0.1-4 oct default 1).

Transform body:

```rust
impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let twist = (self.twist_pct * 0.01).clamp(-1.0, 1.0);
        if twist.abs() < 1e-3 { return; }
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins = (((self.freq_hz * (self.bw_oct * 0.5).exp2()) - self.freq_hz) / bin_hz)
            .ceil() as i32;
        let scale = 1.0 - twist; // twist=+1 -> scale=0 (collapse), twist=-1 -> scale=2 (spread)

        // Stash positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k];
        }
        // Zero the band in the destination.
        for k in 1..half {
            let d = k as i32 - centre_bin;
            if d.abs() <= band_bins {
                spectrum[k] = Complex::default();
            }
        }
        // Map each source bin in the band to its scaled destination.
        for d_src in -band_bins..=band_bins {
            let src = centre_bin + d_src;
            if src < 1 || (src as usize) >= half { continue; }
            let d_dst_f = d_src as f32 * scale;
            let dst = centre_bin + d_dst_f.round() as i32;
            if dst < 1 || (dst as usize) >= half { continue; }
            let src_val = spectrum[fft_size - src as usize]; // from stash
            // Accumulate (multiple sources may map to same dest at twist=+1).
            spectrum[dst as usize].re += src_val.re;
            spectrum[dst as usize].im += src_val.im;
        }
        // Rebuild negative half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}
```

`PARAMS`: `["FFT", "Freq", "Twist", "Bandwidth"]`.

Tests:
- `twist_zero_is_passthrough`
- `silence_in_silence_out`
- `parameters_count_is_four`

- [ ] **Step 2: 14 mod.rs sites for `SpectralTwist`**. `ALL.len()` to 35.

- [ ] **Step 3: Workspace gate.**

- [ ] **Step 4: Commit**

```bash
git add multosis/src/effects/spectral_twist.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralTwist effect -- fold spectrum around a centre

Within +/- Bandwidth/2 oct of Freq, each bin's distance-from-centre
scales by (1 - Twist). Twist=+1 collapses the band onto Freq;
Twist=-1 doubles the spread. Out-of-band passes. EffectKind::ALL
grows 34 -> 35.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 16: SpectralStretch (phase vocoder with own 75% analyzer)

Phase vocoder with own analyzer at hop = fft_size / 4 (75% overlap). Does NOT use `SpectralEngine` — holds its own analyzer + synthesis. The FFT-size param still snaps to the same 4 sizes; the file is otherwise self-contained.

**Files:**
- Create: `multosis/src/effects/spectral_stretch.rs`
- Modify: `multosis/src/effects/mod.rs`

- [ ] **Step 1: Write the effect file**

This file is significantly more complex than the others. The implementer should:

1. **Read `tract-dsp/src/spectral_shifter.rs`** as the reference. SpectralStretch is a close cousin of SpectralShifter — they share the phase-vocoder structure but differ in the per-bin transform (Stretch does NOT remap bins; it manipulates phase advance to slow/speed up the time-evolution while keeping pitch).
2. **Hold 4 analyzers/synthesizers** (one per FFT size) using the same pre-allocated-slots pattern as `SpectralEngine`, but each slot uses `hop = fft_size / 4` (75% overlap) instead of 50%.
3. **Per-bin state**: `last_input_phase[k]`, `accumulated_output_phase[k]` — same as SpectralShifter.
4. **Tempo (1-100%)**: throttles how often a new ANALYSIS frame is consumed. At Tempo=1% only 1 in 100 hops triggers a new analyze; the rest keep advancing phase from the last analyzed frame.
5. **Speed (0.25-4×)**: scales how much synthesis-time-axis advances per hop.
6. **Chaos (0-100%)**: adds random angle to per-bin phase per hop.

The implementer should write Phase A and Phase B as two commits:

- **Phase A** (first commit): write the file with FFT size + ALL the mod.rs glue, but with a stub transform that just identity-passes the spectrum (so the new `EffectKind` variant builds, registers, and routes audio cleanly).
- **Phase B** (second commit): fill in the phase-vocoder math; add the substantive test (`speed_half_stretches_a_burst` — drive a 100 ms burst then silence, observe that with Speed=0.5 the output retains energy beyond 200 ms).

`PARAMS`: `["FFT", "Speed", "Tempo", "Chaos"]`.

Per-channel state struct skeleton:

```rust
struct SpectralStretchChannel {
    last_input_phase: Vec<f32>,
    accumulated_output_phase: Vec<f32>,
    rng_state: u32,
    // ...slots, one per FFT size (full analyzer + IFFT + output ring,
    // mirroring SpectralEngine's Slot but with hop=fft/4)
}
```

- [ ] **Step 2: Phase A — stub commit**

After writing the file with an identity-passing transform:

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
git add multosis/src/effects/spectral_stretch.rs multosis/src/effects/mod.rs
git commit -m "feat(multosis): SpectralStretch scaffolding (phase A, stub transform)

EffectKind variant + 14 mod.rs glue sites + own analyzer slots at
75% overlap (hop = fft_size / 4). Transform is identity-pass for
now; Phase B fills in the phase vocoder math.

EffectKind::ALL grows 35 -> 36.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

- [ ] **Step 3: Phase B — fill in the transform**

Implement the per-bin phase advance:

```rust
// Per hop, for bin k:
//   expected_phase_increment = 2*pi * k * hop_samples / fft_size
//   actual_input_phase = atan2(spectrum[k].im, spectrum[k].re)
//   delta = wrap_pi(actual_input_phase - last_input_phase[k] - expected_phase_increment)
//   true_freq = expected_phase_increment + delta
//   accumulated_output_phase[k] += true_freq * speed
//   spectrum[k] = mag(spectrum[k]) * exp(i * (accumulated_output_phase[k] + chaos * random))
//   last_input_phase[k] = actual_input_phase
```

Then add the substantive Speed test described above.

- [ ] **Step 4: Workspace gate.**

- [ ] **Step 5: Phase B commit**

```bash
git add multosis/src/effects/spectral_stretch.rs
git commit -m "feat(multosis): SpectralStretch phase vocoder math (phase B)

Phase-advance per-bin per hop, scaled by Speed. Tempo throttles
re-analyze rate (Tempo=1% holds the last analyzed frame for 100
hops). Chaos adds random angle. Adds the speed_half_stretches_a_burst
test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 17: Docs and final integration sweep

Update CLAUDE.md and run a final workspace pass to make sure everything is healthy.

**Files:**
- Modify: `CLAUDE.md` (workspace-root)
- Modify: `multosis/CLAUDE.md` (if it exists; if not, skip that file)

- [ ] **Step 1: Update workspace `CLAUDE.md`**

Find the multosis effect table in `CLAUDE.md` (or wherever EffectKind variants are documented). Add a "Spectral family" subsection listing the 14 new effects with one-line summaries each.

If `CLAUDE.md` doesn't already list each effect, just add a single sentence near the multosis description:

```text
Includes a 14-effect Spectral family (`Spectral{Shift,Rotate,Twist,Mirror,Bandpass,Stretch,Scatter,Cascade,Smear,Reverb,Compress,Corrupt,Lofi,Spread}`) sharing the new `tract_dsp::spectral_engine::SpectralEngine` (audio-thread-safe switchable FFT size, hop = N/2 except Stretch which uses hop = N/4 for phase vocoding).
```

- [ ] **Step 2: Final workspace gate**

```bash
cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release
```

Expected: green clippy, all tests pass (`EffectKind::ALL.len() == 36`), bundles built.

- [ ] **Step 3: Verify the EffectKind dropdown by hand**

Launch the standalone:

```bash
cargo build --bin multosis --release && target/release/multosis &
```

Open the effect editor for any row, click the effect dropdown, scroll to the bottom: the 14 `Spectral *` entries should appear after `Vocoder`, in the order they were added.

Close the standalone.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md multosis/CLAUDE.md 2>/dev/null || git add CLAUDE.md
git commit -m "docs(multosis): document the spectral effects family

Adds a one-line summary of the 14 new Spectral* effects and the
shared SpectralEngine in tract-dsp.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Conventions reminder (subagent crib sheet)

Every per-effect task can be summarised as:

1. **Read** `tract-dsp/src/spectral_engine.rs` and the `spectral_rotate.rs` template (committed in Task 2).
2. **Write** the per-effect file at `multosis/src/effects/spectral_<name>.rs` following the skeleton.
3. **Edit** `multosis/src/effects/mod.rs` at the 14 sites listed in the recipe section at the top of this plan. Use `grep -n SpectralRotate multosis/src/effects/mod.rs` to find each site by analogy.
4. **Bump** the `EffectKind::ALL.len()` assertion.
5. **Add** the `name()` test assertion.
6. **Run** the workspace gate: `cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace && cargo xtask native nih-plug bundle multosis --release`.
7. **Commit** with the message template the task specifies.

Allocation rules (audio thread):
- No `Vec::new`, no `vec![...]`, no `String::push_str`. Use pre-allocated scratch held on the per-channel state.
- `SpectralEngine` is allocation-free post-construction; all four FFT sizes are pre-allocated.
- `xorshift` is OK for per-bin random number generation; `rand::*` is not (heap allocation).
- `Vec::fill`, `Vec::copy_from_slice`, slice indexing, arithmetic — all fine.

Test rules:
- Every effect MUST have a "silence in -> silence out" test and a "no-op param is passthrough" test.
- At least one substantive test asserting the effect actually does what it says (energy moved, tail extended, bins zeroed, etc.). Use small FFT size (1024) and short signals (~8192 samples) to keep test times below 100 ms each.
- Use `cargo nextest run -p multosis spectral_<name>::tests` to iterate fast during development.

Build cache:
- Between effects, `cargo build -p multosis` is much faster than the workspace gate. Run the workspace gate only at the commit step.

When NOT to commit:
- Never on `master`. Branch is `multosis`.
- Never without the workspace gate passing.
- Never with `#[allow(...)]` annotations added to silence clippy.

End of plan.
