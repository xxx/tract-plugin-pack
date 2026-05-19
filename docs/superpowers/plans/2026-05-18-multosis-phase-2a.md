# Multosis Phase 2, Milestone 2a — Effect Abstraction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Multosis's two hardwired throwaway effects with a standardized `Effect` trait, an effect registry, and per-track effect instances.

**Architecture:** New `effects.rs` types — a `ParamSpec`, an `Effect` trait, two trait-implementing effects (`LowpassEffect`, `BitcrushEffect`), an `EffectKind` registry enum, an enum-dispatch `EffectInstance`, and a persisted per-track `TrackEffect` config — are added additively (Tasks 1–4). Then the editor's effect-bank control is removed (Task 5) and the audio engine is converted to a `[EffectInstance; 16]` built from the config, dropping the old `EffectBank` path (Task 6).

**Tech Stack:** Rust (nightly), nih-plug, `cargo nextest`, serde.

**Reference:** `docs/superpowers/specs/2026-05-18-multosis-phase-2a-design.md`; Phase 1 design `docs/superpowers/specs/2026-05-17-multosis-phase-1-design.md` §6.

**Working branch:** `multosis` (already checked out). All work commits to it.

**Commit convention:** every commit message ends with the trailer
`Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
The `git commit` lines below omit it for brevity — add it to each.

**Refinement of the spec:** the spec §1 sketched `Effect::process` as a block call (`&mut [f32]`). The current `AudioEngine` processes audio sample-by-sample within step segments; to fit it without an engine restructure and audio-thread scratch-buffer allocation, this plan uses a **per-sample** trait method `process_sample(left, right) -> (f32, f32)` plus a `set_sample_rate` for coefficient recompute. The spec's intent — a standardized effect trait, registry, per-track instances — is unchanged.

**Pre-existing state (145 multosis tests, 922 workspace tests green):**
- `multosis/src/effects.rs` — `EffectBank` enum (`Lowpass`, `Bitcrush`, an nih-plug `#[derive(Enum)]`); `LowpassBank` (per-row one-pole: `state: [[f32;2]; ROWS]`, `coeff: [f32; ROWS]`; `new`, `set_sample_rate`, `reset`, `process(row, channel, x) -> f32`); `BitcrushBank` (per-row quantization: `step: [f32; ROWS]`; `new`, `process(&self, row, x) -> f32`).
- `multosis/src/engine.rs` — `AudioEngine { propagator, clock, lowpass: LowpassBank, bitcrush: BitcrushBank }`; `new`, `set_sample_rate(sr)` (calls `lowpass.set_sample_rate`), `reset` (calls `lowpass.reset`), `wavefront`/`sequence_state`/`step` accessors, `active_rows(grid, wf) -> u16` (bit `r` set if any cell in row `r` is lit+enabled), `process_sample(&mut self, dry_l, dry_r, active: u16, bank: EffectBank) -> (f32,f32)` (loops rows, sums each active row's effect output), `process(&mut self, left, right, playing, samples_per_step, bank: EffectBank, mix, auto_restart, grid)`. Six engine tests call `engine.process(..., EffectBank::Lowpass, ...)`.
- `multosis/src/lib.rs` — `MultosisParams` derives `Params`, fields: `editor_state`, `grid` (`#[persist="grid"] Arc<Mutex<Grid>>`), `speed: EnumParam<Speed>`, `mix: FloatParam`, `output_gain: FloatParam`, `effect_bank: EnumParam<EffectBank>` (`#[id="effect_bank"]`), `auto_restart: BoolParam`. `impl Default for MultosisParams` constructs each. `Multosis` plugin struct has `params`, `grid_handoff`, `grid`, `engine`, `sample_rate`, `was_playing`, `wavefront_display`, `seq_status`, `reset_request`. `process()` reads `bank = self.params.effect_bank.value()` and calls `self.engine.process(&mut *left, &mut *right, playing, sps, bank, mix, auto_restart, &self.grid)`. The persisted grid is bridged into the engine in `initialize`-equivalent code: `if let Ok(grid) = self.params.grid.lock() { self.grid = *grid; self.grid_handoff.publish(*grid); }`.
- `multosis/src/editor/toolbar.rs` — `ToolbarControl` enum has a `Bank` variant; `ToolbarControl::ALL` (6 controls: `Speed, Bank, AutoRestart, Mix, Output, Reset`); `logical_x_w` gives each control's `(x, width)`; `draw_toolbar` draws each control; a `bank_label` helper. `multosis/src/editor.rs` `handle_toolbar_button` has a `ToolbarControl::Bank` arm cycling `effect_bank`.
- `crate::grid` — `ROWS = 16`, `COLS = 32`; `Cell { enabled, is_start, sends }` derives serde; small `[T; ≤32]` structs use plain `#[derive(Serialize, Deserialize)]` (only the 512-cell `Grid` needs hand-rolled serde).
- `multosis/src/handoff.rs` — `GridHandoff` (Mutex + `try_read`/`publish`). 2a does NOT add an effect-config handoff (that is 2c); the persisted config is read once at init.

---

### Task 1: `ParamSpec`, the `Effect` trait, and `LowpassEffect`

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to `effects.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn lowpass_effect_parameters_are_declared() {
        let lp = LowpassEffect::new();
        let specs = lp.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Cutoff");
        assert_eq!(specs[1].name, "Resonance");
        assert!(specs[0].min < specs[0].max);
    }

    #[test]
    fn lowpass_effect_dark_cutoff_attenuates_highs() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 200.0); // cutoff 200 Hz
        lp.set_param(1, 0.0); // resonance 0
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 }; // Nyquist-ish
            let (l, _) = lp.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(peak < 0.5, "a 200 Hz lowpass should kill a fast alternation, got {peak}");
    }

    #[test]
    fn lowpass_effect_open_cutoff_passes_a_constant() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 18_000.0);
        lp.set_param(1, 0.0);
        let mut y = 0.0;
        for _ in 0..2048 {
            y = lp.process_sample(1.0, 1.0).0;
        }
        assert!(y > 0.9, "an open lowpass should pass a constant, got {y}");
    }

    #[test]
    fn lowpass_effect_reset_clears_state() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(0, 300.0);
        for _ in 0..512 {
            lp.process_sample(1.0, 1.0);
        }
        lp.reset();
        let y = lp.process_sample(1.0, 1.0).0;
        assert!(y.abs() < 0.5, "reset should clear filter state, got {y}");
    }

    #[test]
    fn lowpass_effect_set_param_out_of_range_is_ignored() {
        let mut lp = LowpassEffect::new();
        lp.set_sample_rate(48_000.0);
        lp.set_param(99, 1.0); // no panic, no effect
        let y = lp.process_sample(0.25, 0.25);
        assert!(y.0.is_finite());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(lowpass_effect)'`
Expected: build failure — `cannot find type LowpassEffect` / `ParamSpec`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs` (after the existing `use` lines, before `EffectBank` is fine — placement is free):

```rust
/// A modulatable parameter of an effect: its name and value range. Static per
/// effect kind; used by the 2b modulation engine and the 2c effect editor.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// The standardized audio-effect contract. Implemented by each effect struct;
/// dispatched allocation-free through `EffectInstance` (no `dyn`). Audio-thread
/// methods (`process_sample`, `set_param`, `reset`) must not allocate.
pub trait Effect {
    /// Process one stereo sample, returning the wet `(left, right)`. DSP state
    /// persists across calls so the effect does not click on reactivation.
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32);

    /// Recompute sample-rate-dependent coefficients.
    fn set_sample_rate(&mut self, sample_rate: f32);

    /// Clear all DSP state.
    fn reset(&mut self);

    /// The effect's modulatable parameters, in `set_param` index order.
    fn parameters(&self) -> &'static [ParamSpec];

    /// Set parameter `index` to `value` (clamped to the spec's range). An
    /// out-of-range `index` is ignored. In 2a values come from the persisted
    /// config; 2b's MSEGs will write them every block.
    fn set_param(&mut self, index: usize, value: f32);
}

/// A resonant lowpass — a TPT state-variable filter, lowpass output.
pub struct LowpassEffect {
    cutoff: f32,
    resonance: f32,
    sample_rate: f32,
    // Cytomic TPT-SVF coefficients, recomputed on any param / SR change.
    a1: f32,
    a2: f32,
    a3: f32,
    // Per-channel integrator state.
    ic1: [f32; 2],
    ic2: [f32; 2],
}

impl LowpassEffect {
    /// `ParamSpec`s for `LowpassEffect` — index 0 cutoff, 1 resonance.
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Cutoff",
            min: 20.0,
            max: 20_000.0,
            default: 2_000.0,
        },
        ParamSpec {
            name: "Resonance",
            min: 0.0,
            max: 1.0,
            default: 0.1,
        },
    ];

    /// A `LowpassEffect` at its default parameters; call `set_sample_rate`
    /// before processing.
    pub fn new() -> Self {
        let mut lp = Self {
            cutoff: Self::PARAMS[0].default,
            resonance: Self::PARAMS[1].default,
            sample_rate: 48_000.0,
            a1: 0.0,
            a2: 0.0,
            a3: 0.0,
            ic1: [0.0; 2],
            ic2: [0.0; 2],
        };
        lp.recompute();
        lp
    }

    /// Recompute the TPT-SVF coefficients from cutoff / resonance / SR.
    fn recompute(&mut self) {
        let sr = self.sample_rate.max(1.0);
        let fc = self.cutoff.clamp(20.0, sr * 0.49);
        let g = (std::f32::consts::PI * fc / sr).tan();
        let q = 0.5 + self.resonance.clamp(0.0, 1.0) * 9.5; // Q 0.5..10
        let k = 1.0 / q;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }
}

impl Default for LowpassEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for LowpassEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut step = |x: f32, ch: usize| {
            let v3 = x - self.ic2[ch];
            let v1 = self.a1 * self.ic1[ch] + self.a2 * v3;
            let v2 = self.ic2[ch] + self.a2 * self.ic1[ch] + self.a3 * v3;
            self.ic1[ch] = 2.0 * v1 - self.ic1[ch];
            self.ic2[ch] = 2.0 * v2 - self.ic2[ch];
            v2 // lowpass output
        };
        (step(left, 0), step(right, 1))
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.recompute();
    }

    fn reset(&mut self) {
        self.ic1 = [0.0; 2];
        self.ic2 = [0.0; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.cutoff = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.resonance = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => return,
        }
        self.recompute();
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(lowpass_effect)'`
Expected: PASS — 5 tests. Then `cargo build -p multosis` — compiles (the old `LowpassBank` etc. still present and unused-by-new-code; no warnings — they are still used by the engine).

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): add the Effect trait and LowpassEffect"
```

---

### Task 2: `BitcrushEffect`

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to `effects.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn bitcrush_effect_parameters_are_declared() {
        let bc = BitcrushEffect::new();
        let specs = bc.parameters();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, "Bit Depth");
        assert_eq!(specs[1].name, "Rate Reduction");
    }

    #[test]
    fn bitcrush_effect_low_bit_depth_quantizes_coarsely() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 2.0); // 2-bit
        bc.set_param(1, 1.0); // no rate reduction
        let crushed = bc.process_sample(0.1, 0.1).0;
        bc.set_param(0, 16.0); // 16-bit
        let clean = bc.process_sample(0.1, 0.1).0;
        assert!(
            (crushed - 0.1).abs() > (clean - 0.1).abs(),
            "2-bit ({crushed}) should distort more than 16-bit ({clean})"
        );
    }

    #[test]
    fn bitcrush_effect_rate_reduction_holds_samples() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 16.0); // clean bit depth
        bc.set_param(1, 4.0); // hold each sample ~4 input samples
        let first = bc.process_sample(1.0, 1.0).0;
        // The next few inputs differ, but the held output should not change.
        let held = bc.process_sample(-1.0, -1.0).0;
        assert!((first - held).abs() < 1e-6, "rate reduction should hold the sample");
    }

    #[test]
    fn bitcrush_effect_output_is_bounded() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(0, 3.0);
        for &x in &[-1.0_f32, -0.3, 0.0, 0.42, 1.0] {
            let (l, r) = bc.process_sample(x, x);
            assert!(l.abs() <= 1.5 && r.abs() <= 1.5, "x {x} -> ({l},{r}) out of range");
        }
    }

    #[test]
    fn bitcrush_effect_reset_clears_hold_state() {
        let mut bc = BitcrushEffect::new();
        bc.set_sample_rate(48_000.0);
        bc.set_param(1, 8.0);
        bc.process_sample(0.7, 0.7);
        bc.reset();
        // After reset the first sample is taken fresh.
        let y = bc.process_sample(0.25, 0.25).0;
        assert!((y - 0.25).abs() < 0.1, "reset should re-sample, got {y}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(bitcrush_effect)'`
Expected: build failure — `cannot find type BitcrushEffect`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs`, after `LowpassEffect`:

```rust
/// Bit-depth reduction plus sample-rate reduction (sample-and-hold).
pub struct BitcrushEffect {
    bit_depth: f32,
    rate_reduction: f32,
    // Per-channel sample-and-hold state.
    held: [f32; 2],
    phase: [f32; 2],
}

impl BitcrushEffect {
    /// `ParamSpec`s — index 0 bit depth, 1 rate reduction.
    const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Bit Depth",
            min: 1.0,
            max: 16.0,
            default: 16.0,
        },
        ParamSpec {
            name: "Rate Reduction",
            min: 1.0,
            max: 50.0,
            default: 1.0,
        },
    ];

    /// A `BitcrushEffect` at its default (near-clean) parameters.
    pub fn new() -> Self {
        Self {
            bit_depth: Self::PARAMS[0].default,
            rate_reduction: Self::PARAMS[1].default,
            held: [0.0; 2],
            // Start ready to sample on the first call.
            phase: [Self::PARAMS[1].default; 2],
        }
    }

    /// Quantize `x` to the current bit depth.
    fn quantize(&self, x: f32) -> f32 {
        let levels = 2.0_f32.powf(self.bit_depth);
        let step = 2.0 / levels;
        (x / step).round() * step
    }
}

impl Default for BitcrushEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for BitcrushEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut step = |x: f32, ch: usize| {
            self.phase[ch] += 1.0;
            if self.phase[ch] >= self.rate_reduction {
                self.phase[ch] -= self.rate_reduction;
                self.held[ch] = self.quantize(x);
            }
            self.held[ch]
        };
        (step(left, 0), step(right, 1))
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {
        // Bitcrush has no sample-rate-dependent coefficients.
    }

    fn reset(&mut self) {
        self.held = [0.0; 2];
        self.phase = [self.rate_reduction; 2];
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.bit_depth = value.clamp(Self::PARAMS[0].min, Self::PARAMS[0].max),
            1 => self.rate_reduction = value.clamp(Self::PARAMS[1].min, Self::PARAMS[1].max),
            _ => {}
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(bitcrush_effect)'`
Expected: PASS — 5 tests. Then `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): add BitcrushEffect"
```

---

### Task 3: `EffectKind` registry and `EffectInstance` dispatch

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to `effects.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn effect_kind_registry() {
        assert_eq!(EffectKind::ALL.len(), 2);
        assert_eq!(EffectKind::Lowpass.name(), "Lowpass");
        assert_eq!(EffectKind::Bitcrush.name(), "Bitcrush");
    }

    #[test]
    fn effect_instance_dispatches_to_the_right_effect() {
        let mut lp = EffectInstance::new(EffectKind::Lowpass);
        assert_eq!(lp.kind(), EffectKind::Lowpass);
        assert_eq!(lp.parameters().len(), 2);
        let mut bc = EffectInstance::new(EffectKind::Bitcrush);
        assert_eq!(bc.kind(), EffectKind::Bitcrush);
        // The dispatched calls reach the contained effect.
        lp.set_sample_rate(48_000.0);
        bc.set_sample_rate(48_000.0);
        let _ = lp.process_sample(0.5, 0.5);
        let _ = bc.process_sample(0.5, 0.5);
        lp.reset();
        bc.reset();
    }

    #[test]
    fn effect_instance_set_param_changes_behaviour() {
        let mut e = EffectInstance::new(EffectKind::Lowpass);
        e.set_sample_rate(48_000.0);
        e.set_param(0, 200.0);
        e.set_param(1, 0.0);
        let mut peak = 0.0_f32;
        for i in 0..2048 {
            let x = if i % 2 == 0 { 1.0 } else { -1.0 };
            let (l, _) = e.process_sample(x, x);
            if i > 256 {
                peak = peak.max(l.abs());
            }
        }
        assert!(peak < 0.5, "the dispatched lowpass should attenuate, got {peak}");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(effect_kind) + test(effect_instance)'`
Expected: build failure — `cannot find type EffectKind` / `EffectInstance`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs`, after `BitcrushEffect`:

```rust
/// The effect registry — which effects exist. `Copy`, serde-derivable.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum EffectKind {
    Lowpass,
    Bitcrush,
}

impl EffectKind {
    /// Every effect kind, in display / registry order.
    pub const ALL: [EffectKind; 2] = [EffectKind::Lowpass, EffectKind::Bitcrush];

    /// The kind's display name.
    pub fn name(self) -> &'static str {
        match self {
            EffectKind::Lowpass => "Lowpass",
            EffectKind::Bitcrush => "Bitcrush",
        }
    }
}

/// A live effect instance — enum dispatch over the effect structs, so the
/// audio engine holds `[EffectInstance; 16]` with no heap and no `dyn`.
pub enum EffectInstance {
    Lowpass(LowpassEffect),
    Bitcrush(BitcrushEffect),
}

impl EffectInstance {
    /// A fresh instance of `kind` at default parameters.
    pub fn new(kind: EffectKind) -> Self {
        match kind {
            EffectKind::Lowpass => EffectInstance::Lowpass(LowpassEffect::new()),
            EffectKind::Bitcrush => EffectInstance::Bitcrush(BitcrushEffect::new()),
        }
    }

    /// Which kind this instance is.
    pub fn kind(&self) -> EffectKind {
        match self {
            EffectInstance::Lowpass(_) => EffectKind::Lowpass,
            EffectInstance::Bitcrush(_) => EffectKind::Bitcrush,
        }
    }
}

impl Effect for EffectInstance {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        match self {
            EffectInstance::Lowpass(e) => e.process_sample(left, right),
            EffectInstance::Bitcrush(e) => e.process_sample(left, right),
        }
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        match self {
            EffectInstance::Lowpass(e) => e.set_sample_rate(sample_rate),
            EffectInstance::Bitcrush(e) => e.set_sample_rate(sample_rate),
        }
    }

    fn reset(&mut self) {
        match self {
            EffectInstance::Lowpass(e) => e.reset(),
            EffectInstance::Bitcrush(e) => e.reset(),
        }
    }

    fn parameters(&self) -> &'static [ParamSpec] {
        match self {
            EffectInstance::Lowpass(e) => e.parameters(),
            EffectInstance::Bitcrush(e) => e.parameters(),
        }
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match self {
            EffectInstance::Lowpass(e) => e.set_param(index, value),
            EffectInstance::Bitcrush(e) => e.set_param(index, value),
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(effect_kind) + test(effect_instance)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs
git commit -m "feat(multosis): add the EffectKind registry and EffectInstance dispatch"
```

---

### Task 4: `TrackEffect` — the per-track effect config

**Files:**
- Modify: `multosis/src/effects.rs`

- [ ] **Step 1: Write the failing test**

Add to `effects.rs`'s `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn track_effect_serde_round_trips() {
        let te = TrackEffect {
            kind: EffectKind::Bitcrush,
            params: [3.0, 8.0, 0.0, 0.0],
        };
        let json = serde_json::to_string(&te).unwrap();
        let back: TrackEffect = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, EffectKind::Bitcrush);
        assert_eq!(back.params, [3.0, 8.0, 0.0, 0.0]);
    }

    #[test]
    fn track_effect_array_serde_round_trips() {
        let config: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
        let json = serde_json::to_string(&config).unwrap();
        let back: [TrackEffect; 16] = serde_json::from_str(&json).unwrap();
        assert_eq!(back, config);
    }

    #[test]
    fn default_for_row_varies_and_exercises_both_kinds() {
        let config: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
        // Both effect kinds appear across the 16 rows.
        assert!(config.iter().any(|t| t.kind == EffectKind::Lowpass));
        assert!(config.iter().any(|t| t.kind == EffectKind::Bitcrush));
        // The config is not 16 identical rows.
        assert!(config.iter().any(|t| *t != config[0]));
    }
```

NOTE: `serde_json` — confirm it is available as a dev-dependency or dependency of `multosis` (the `Grid` serde tests use it). If `serde_json` is not already a dependency, add it under `[dev-dependencies]` in `multosis/Cargo.toml` (it is already in the workspace lock — used by other crates' tests).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p multosis --lib -E 'test(track_effect) + test(default_for_row)'`
Expected: build failure — `cannot find type TrackEffect`.

- [ ] **Step 3: Write minimal implementation**

Add to `multosis/src/effects.rs`, after `EffectInstance`:

```rust
/// Maximum modulatable parameters any effect declares — fixes the
/// `TrackEffect::params` array length so the persisted config is stable as
/// effects are added (current max is 2; 4 leaves headroom).
pub const MAX_EFFECT_PARAMS: usize = 4;

/// One track row's persisted effect configuration: which effect, and its
/// parameter values. `params[i]` is the value for the kind's `parameters()[i]`;
/// entries past the kind's parameter count are unused.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackEffect {
    pub kind: EffectKind,
    pub params: [f32; MAX_EFFECT_PARAMS],
}

impl TrackEffect {
    /// The default effect for track row `row` (`0..16`). Alternates the two
    /// kinds and spreads parameters by row so the sequencer plays with
    /// audible per-track variety before the 2c assignment UI exists.
    pub fn default_for_row(row: usize) -> Self {
        let t = row as f32 / 15.0; // 0.0 at row 0 .. 1.0 at row 15
        if row % 2 == 0 {
            // Lowpass: cutoff dark -> open across the rows.
            let cutoff = 300.0 * (12_000.0_f32 / 300.0).powf(t);
            TrackEffect {
                kind: EffectKind::Lowpass,
                params: [cutoff, 0.15, 0.0, 0.0],
            }
        } else {
            // Bitcrush: heavily crushed -> nearly clean across the rows.
            let bits = 3.0 + t * 11.0; // 3..14 bits
            TrackEffect {
                kind: EffectKind::Bitcrush,
                params: [bits, 1.0, 0.0, 0.0],
            }
        }
    }
}

impl Default for TrackEffect {
    fn default() -> Self {
        Self::default_for_row(0)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p multosis --lib -E 'test(track_effect) + test(default_for_row)'`
Expected: PASS — 3 tests. Then `cargo build -p multosis` — compiles, no warnings.

- [ ] **Step 5: Commit**

```bash
git add multosis/src/effects.rs multosis/Cargo.toml
git commit -m "feat(multosis): add the TrackEffect per-track config"
```

(`Cargo.toml` only if `serde_json` had to be added.)

---

### Task 5: Remove the toolbar effect-bank control

**Files:**
- Modify: `multosis/src/editor/toolbar.rs`
- Modify: `multosis/src/editor.rs`

The editor's `Bank` toolbar control selects `effect_bank`, which Task 6 removes. Removing the control first keeps each task's build green. Verified by compilation + tests.

- [ ] **Step 1: Remove the `Bank` control from `toolbar.rs`**

In `multosis/src/editor/toolbar.rs`:
- Remove the `Bank` variant from the `ToolbarControl` enum.
- Remove `Bank` from `ToolbarControl::ALL` (now 5 controls, in order: `Speed, AutoRestart, Mix, Output, Reset`).
- In `logical_x_w`, replace the six match arms with five — the five remaining controls evenly divide the same logical content span the six used (origin `6.0`, right end `1050.0`, `6.0` inter-control gaps): five 204-wide slots.

  ```rust
          match self {
              ToolbarControl::Speed => (6.0, 204.0),
              ToolbarControl::AutoRestart => (216.0, 204.0),
              ToolbarControl::Mix => (426.0, 204.0),
              ToolbarControl::Output => (636.0, 204.0),
              ToolbarControl::Reset => (846.0, 204.0),
          }
  ```

  (`6 + 5×204 + 4×6 = 1050`, matching the old layout's right end; the grid-layout milestone's `remap` then maps this onto the window as before.) The existing `toolbar_rows_lie_within_the_window_margins` / `toolbar_controls_do_not_overlap` / `toolbar_hit_round_trips_each_item` tests iterate `ToolbarControl::ALL` and must still pass.
- Remove the `bank_label` helper function and any other `Bank`-only code (e.g. its arm in `draw_toolbar`).

- [ ] **Step 2: Remove the `Bank` handling from `editor.rs`**

In `multosis/src/editor.rs`, `handle_toolbar_button` has a `ToolbarControl::Bank` match arm that cycles `effect_bank` — remove that arm. Remove any other `ToolbarControl::Bank` reference. (`effect_bank` the *parameter* still exists after this task — it simply no longer has a toolbar control; Task 6 removes the parameter.)

- [ ] **Step 3: Verify**

Run: `cargo build -p multosis` — compiles with NO warnings.
Run: `cargo nextest run -p multosis` — PASS, 161 tests (145 + 16 from Tasks 1–4).
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 4: Commit**

```bash
git add multosis/src/editor/toolbar.rs multosis/src/editor.rs
git commit -m "feat(multosis): remove the effect-bank toolbar control"
```

---

### Task 6: Convert the audio engine to per-track effects

**Files:**
- Modify: `multosis/src/engine.rs`
- Modify: `multosis/src/lib.rs`
- Modify: `multosis/src/effects.rs`

The interlocked conversion: the engine holds `[EffectInstance; 16]`, the plugin persists `[TrackEffect; 16]`, and `EffectBank` / the old banks / the `effect_bank` parameter are removed.

- [ ] **Step 1: Write / update the failing tests**

In `multosis/src/engine.rs`'s `#[cfg(test)] mod tests` block:
- Every existing test that calls `engine.process(..., EffectBank::Lowpass, ...)` (six call sites) — remove the `EffectBank::Lowpass` argument (the `process` signature loses the `bank` parameter — see Step 3).
- Add this new test:

```rust
    #[test]
    fn engine_runs_per_track_effects() {
        let config: [crate::effects::TrackEffect; 16] =
            std::array::from_fn(crate::effects::TrackEffect::default_for_row);
        let mut engine = AudioEngine::new();
        engine.set_sample_rate(48_000.0);
        engine.set_effects(&config);
        let grid = Grid::default_routing(); // left column = start cells
        let mut left = [0.3_f32; 64];
        let mut right = [0.3_f32; 64];
        // One armed step at full wet: the start column lights, every row's
        // own effect runs — output is finite and not pure dry passthrough.
        engine.process(&mut left, &mut right, true, 10.0, 1.0, true, &grid);
        assert!(left.iter().all(|s| s.is_finite()));
        assert!(
            left.iter().any(|&s| (s - 0.3).abs() > 1e-6),
            "per-track effects should change the signal"
        );
    }
```

(The exact `process` argument list after the `bank` removal is in Step 3; match it in the updated and new tests.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p multosis --lib -E 'test(engine_runs_per_track_effects)'`
Expected: build failure — `no method named set_effects`.

- [ ] **Step 3: Convert `AudioEngine`** (`multosis/src/engine.rs`)

- Replace the `lowpass: LowpassBank` and `bitcrush: BitcrushBank` fields with `effects: [EffectInstance; 16]` and `sample_rate: f32`. Import `crate::effects::{Effect, EffectInstance, TrackEffect}` (drop the `EffectBank`/`LowpassBank`/`BitcrushBank` imports).
- `AudioEngine::new()` — build `effects` from the per-row defaults: `effects: std::array::from_fn(|r| { let cfg = TrackEffect::default_for_row(r); let mut e = EffectInstance::new(cfg.kind); for i in 0..e.parameters().len() { e.set_param(i, cfg.params[i]); } e })`, `sample_rate: 48_000.0`.
- Add a method `set_effects(&mut self, config: &[TrackEffect; 16])` — rebuild `self.effects` from `config` exactly as `new` does from the defaults (construct `EffectInstance::new(cfg.kind)`, apply `set_param` for each of the kind's parameters from `cfg.params`), then apply the stored sample rate: after rebuilding, call `set_sample_rate` on each instance with `self.sample_rate`.
- `set_sample_rate(&mut self, sample_rate: f32)` — store `self.sample_rate = sample_rate` and call `set_sample_rate(sample_rate)` on every instance in `self.effects`.
- `reset()` — keep the propagator/clock reset; replace `self.lowpass.reset()` with a loop calling `reset()` on every instance in `self.effects`.
- `process_sample` — drop the `bank: EffectBank` parameter; for each active row `r`, call `self.effects[r].process_sample(dry_l, dry_r)` and sum the `(l, r)` into the wet accumulators. (A per-row `amplitude` gain — constant `1.0` in 2a, the seam for 2b — may be applied as `wet += 1.0 * effect_out`; keep it as a literal `1.0` multiply or omit it and note the seam in a comment. Do not add an amplitude field in 2a.)
- `process` — drop the `bank: EffectBank` parameter from the signature; its body calls `process_sample` without `bank`. The new signature: `process(&mut self, left: &mut [f32], right: &mut [f32], playing: bool, samples_per_step: f64, mix: f32, auto_restart: bool, grid: &Grid)`. The `#[allow(clippy::too_many_arguments)]` stays.
- `active_rows`, `wavefront`, `sequence_state`, `step` — unchanged.

- [ ] **Step 4: Update `effects.rs`** — remove the superseded code

In `multosis/src/effects.rs`, delete `EffectBank`, `LowpassBank`, `BitcrushBank` and their `impl`s, `Default` impls, and any of their now-orphaned tests (`effect_bank_variants_distinct`, the `LowpassBank`/`BitcrushBank` DSP tests). The `nih_plug::prelude::Enum` import is now unused — remove it. The new `LowpassEffect`/`BitcrushEffect`/`EffectKind`/`EffectInstance`/`TrackEffect` and their tests stay.

- [ ] **Step 5: Update `lib.rs`**

In `multosis/src/lib.rs`:
- Remove the `effect_bank: EnumParam<EffectBank>` field from `MultosisParams` and its initialiser in `impl Default`. Drop the `EffectBank` import.
- Add a persisted per-track effect config field to `MultosisParams`:
  ```rust
      /// Per-track effect configuration — persisted plugin state.
      #[persist = "track-effects"]
      pub track_effects: Arc<Mutex<[crate::effects::TrackEffect; 16]>>,
  ```
  Initialise it in `impl Default`:
  ```rust
              track_effects: Arc::new(Mutex::new(std::array::from_fn(
                  crate::effects::TrackEffect::default_for_row,
              ))),
  ```
- In the plugin's init code that bridges the persisted grid into the engine (the `if let Ok(grid) = self.params.grid.lock() { ... }` block), also bridge the effect config: `if let Ok(cfg) = self.params.track_effects.lock() { self.engine.set_effects(&cfg); }`. Ensure this runs after `self.engine.set_sample_rate(...)` so `set_effects` applies the correct sample rate (or, if `set_sample_rate` runs later, that is fine — `set_effects` uses the engine's stored `sample_rate` and `set_sample_rate` re-applies to all instances; either order leaves every instance correctly configured).
- In `process()`, remove `let bank = self.params.effect_bank.value();` and drop `bank` from the `self.engine.process(...)` call — the new call is `self.engine.process(&mut *left, &mut *right, playing, sps, mix, auto_restart, &self.grid)`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo nextest run -p multosis --lib -E 'test(engine_runs_per_track_effects)'`
Expected: PASS.

Run: `cargo build -p multosis` — compiles, NO warnings.
Run: `cargo nextest run -p multosis` — PASS, 156 tests (161 after Task 5, minus the 6 removed `EffectBank` / `LowpassBank` / `BitcrushBank` tests, plus the 1 new `engine_runs_per_track_effects` test). Confirm and report the exact count.
Run: `cargo clippy -p multosis -- -D warnings` — clean.

- [ ] **Step 7: Commit**

```bash
git add multosis/src/engine.rs multosis/src/lib.rs multosis/src/effects.rs
git commit -m "feat(multosis): per-track trait-based effects in the audio engine"
```

---

### Task 7: Verification

**Files:** none — checks and a manual smoke test.

- [ ] **Step 1: Full suite, lint, format**

Run: `cargo nextest run -p multosis` — PASS (report the count).
Run: `cargo nextest run --workspace` — PASS, all green.
Run: `cargo clippy -p multosis -- -D warnings` — no warnings.
Run: `cargo fmt -p multosis -- --check` — clean (if a diff, run `cargo fmt -p multosis` and commit it in Step 4).

- [ ] **Step 2: Release build and bundle**

Run: `cargo build --bin multosis --release` — the standalone binary builds.
Run: `cargo nih-plug bundle multosis --release` — VST3 + CLAP bundle produced, no errors.

- [ ] **Step 3: Manual smoke test**

Run `cargo run --bin multosis` in a host (or the standalone). Confirm:
- The sequencer still plays; the wavefront propagates and is audible.
- With the default per-track config, different rows sound different (some rows lowpassed, some bitcrushed, character spread across rows) — per-track effects are working.
- The toolbar no longer shows the Effect (Bank) control; Speed / Auto-Restart / Mix / Output / Reset and the six grid operations all still work.
- Reset, drag-paint, region resize, the editor in general — unaffected.

Report the smoke-test observations.

- [ ] **Step 4: Commit (only if Step 1 required formatting edits)**

```bash
git add multosis/
git commit -m "style(multosis): apply rustfmt for the effect abstraction"
```

If Step 1 produced no edits, skip this commit.

---

## Definition of done

- Multosis runs trait-based, per-track effects: an `Effect` trait, an `EffectKind` registry, enum-dispatch `EffectInstance`, and a persisted `[TrackEffect; 16]` config. `EffectBank` and the hardwired banks are gone.
- `cargo nextest run -p multosis` is green; `cargo clippy -p multosis -- -D warnings` is clean; the plugin bundles and plays with per-track variety.

## Spec coverage check (self-review)

- §1 `Effect` trait + `ParamSpec` — Task 1 (per-sample `process_sample` + `set_sample_rate`, as the plan's stated refinement of the spec's block sketch; `reset`, `parameters`, `set_param`).
- §2 Registry + dispatch — `EffectKind` (`ALL`, `name`) and enum `EffectInstance` (Task 3); enum dispatch, no `dyn`, allocation-free.
- §3 The two ported effects — `LowpassEffect` (resonant TPT-SVF, cutoff/resonance) Task 1, `BitcrushEffect` (bit depth + rate reduction) Task 2.
- §4 Per-track config — `TrackEffect` + `MAX_EFFECT_PARAMS` + `default_for_row` (Task 4); persisted `[TrackEffect; 16]` via plain serde derive (Task 6 Step 5); `effect_bank` parameter + `EffectBank` removed (Tasks 5–6).
- §5 Audio-engine conversion — `[EffectInstance; 16]` built from config, `set_effects`, per-track `process_sample`, `process` loses the `bank` arg, state persists across steps (Task 6); the per-row `amplitude` seam noted (Task 6 Step 3).
- §6 The 2b/2c seam — `parameters()`/`set_param` (the modulation seam) and `EffectKind`/`ParamSpec`/`TrackEffect` (the UI seam) all present; no MSEG or UI code written.
- §7 Testing — per-effect DSP, trait surface, dispatch, `TrackEffect` serde, the converted engine (Tasks 1–6); smoke test (Task 7).
- §8 Out of scope — no modulation engine, no tabbed shell / assignment UI / config handoff, no new effects: none added.

## Note on task sequencing

Tasks 1–4 add new types additively — the crate builds and all prior tests pass throughout. Task 5 removes only the toolbar control (the `effect_bank` parameter still exists, just UI-less) — build stays green. Task 6 is the interlocked conversion: it removes `EffectBank`, the old banks, and the `effect_bank` parameter, and rewires the engine and plugin in one commit (they cannot be separated — every `EffectBank` use site falls together). Each task ends with a green build, green tests, and clean clippy.
