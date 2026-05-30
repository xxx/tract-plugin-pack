# Nap Dual-Engine (Zero Latency / Efficient FFT) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a user-selectable **Efficient** (uniformly-partitioned FFT) convolution engine to Nap alongside the existing **Zero Latency** (time-domain) engine — same sound, lower CPU at large/dense settings, ~512-sample latency.

**Architecture:** The whole wet path is LTI, so it equals convolution with one baked impulse response. A new reusable `tract_dsp::partitioned_conv::PartitionedConvolver` (UPOLS) runs that IR; `nap` bakes the IR analytically on the GUI thread, hands it to the audio thread, and routes `process()` by a non-automatable `mode` param. Both engines convolve the same IR, so a mode-equivalence test gates "same sound."

**Tech Stack:** Rust (nightly), nih-plug, `realfft` (already a workspace dep via `tract-dsp`'s `stft` feature). Spec: `docs/superpowers/specs/2026-05-29-nap-fft-mode-design.md`.

**Reference files:** `tract-dsp/src/stft.rs` (realfft `process_with_scratch` + 1/N scaling idiom), `miff/src/lib.rs` (`EnumParam` mode, `.non_automatable()`, click-safe mode reset), `miff/src/editor.rs` (`HitAction::ModeSelector` + mode region), `nap/src/handoff.rs` (`SequenceHandoff` to mirror), `nap/src/engine.rs` (`ReverbChannel`, the coloration `Dictionary`/`OnePole`, `BLOCK`, `SETTLE_SAMPLES`, `MAX_TAIL_SECONDS`).

**Commit rule:** Commits ARE authorized for this work (the user authorized commits for this development). End every commit message with:
`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. One commit per task; amend for review fixes. Do NOT push.

**Build/test:** `cargo nextest run -p nap` / `-p tract-dsp`; `cargo clippy -p <crate> -- -D warnings`; `cargo fmt --check -p <crate>`; profiling `cargo xtask native build --profile profiling --example nap_profile -p nap`.

---

## Task 1: `PartitionedConvolver` in tract-dsp (UPOLS)

**Files:**
- Create: `tract-dsp/src/partitioned_conv.rs`
- Modify: `tract-dsp/src/lib.rs` (add `#[cfg(feature = "partitioned-conv")] pub mod partitioned_conv;`)
- Modify: `tract-dsp/Cargo.toml` (add feature `partitioned-conv = ["dep:realfft"]`)

A uniformly-partitioned overlap-save real convolver. Owns input/output FIFOs so it accepts arbitrary-length blocks and has a fixed `P`-sample latency. `set_ir` installs partition spectra baked elsewhere (so it never does an FFT of the IR on the audio thread).

- [ ] **Step 1: Cargo feature.** In `tract-dsp/Cargo.toml` under `[features]` add:

```toml
# Gates the `partitioned_conv` module (uniformly-partitioned FFT convolution).
partitioned-conv = ["dep:realfft"]
```

- [ ] **Step 2: Write the module** `tract-dsp/src/partitioned_conv.rs`:

```rust
//! Uniformly-partitioned overlap-save (UPOLS) real convolution.
//!
//! Convolves a streaming input with a fixed impulse response that has been
//! pre-partitioned into `P`-sample blocks and forward-FFT'd elsewhere (the
//! audio thread never transforms the IR). Owns input/output FIFOs so it accepts
//! arbitrary block lengths; the I/O introduces exactly `P` samples of latency.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// Partition / latency size and FFT size (`N = 2*P` for linear convolution).
pub const P: usize = 512;
pub const N: usize = 2 * P;
/// Real-FFT bin count for an `N`-point transform.
pub const BINS: usize = N / 2 + 1;

pub struct PartitionedConvolver {
    fft: Arc<dyn RealToComplex<f32>>,
    ifft: Arc<dyn ComplexToReal<f32>>,
    /// IR partition spectra, `max_k` blocks of `BINS` (only `k` active). Newest
    /// input pairs with partition 0.
    ir: Vec<Complex<f32>>,
    k: usize,
    max_k: usize,
    /// Frequency-domain delay line: `max_k` past input-block spectra (ring).
    fdl: Vec<Complex<f32>>,
    fdl_pos: usize,
    /// Time window [prev P | current P] and FFT/accumulator scratch.
    time: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    acc: Vec<Complex<f32>>,
    scratch_fwd: Vec<Complex<f32>>,
    scratch_inv: Vec<Complex<f32>>,
    prev_in: Vec<f32>,
    /// P-sample input/output FIFOs (output primed with P zeros → P latency).
    in_fifo: Vec<f32>,
    in_len: usize,
    out_fifo: Vec<f32>, // ring of length P
    out_read: usize,
    out_fill: usize,
}

impl PartitionedConvolver {
    /// `max_ir_len` bounds the IR length; `max_k = ceil(max_ir_len / P)`.
    pub fn new(max_ir_len: usize) -> Self {
        let max_k = max_ir_len.div_ceil(P).max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let ifft = planner.plan_fft_inverse(N);
        let scratch_fwd = fft.make_scratch_vec();
        let scratch_inv = ifft.make_scratch_vec();
        Self {
            fft,
            ifft,
            ir: vec![Complex::new(0.0, 0.0); max_k * BINS],
            k: 0,
            max_k,
            fdl: vec![Complex::new(0.0, 0.0); max_k * BINS],
            fdl_pos: 0,
            time: vec![0.0; N],
            spectrum: vec![Complex::new(0.0, 0.0); BINS],
            acc: vec![Complex::new(0.0, 0.0); BINS],
            scratch_fwd,
            scratch_inv,
            prev_in: vec![0.0; P],
            in_fifo: vec![0.0; P],
            in_len: 0,
            out_fifo: vec![0.0; P],
            out_read: 0,
            out_fill: P, // prime with P zeros → P-sample latency
        }
    }

    pub fn latency(&self) -> usize {
        P
    }

    /// Install `k` IR partition spectra (`k * BINS` complex values, partition 0
    /// = first P taps). Clears the FDL so the new IR starts clean. GUI/setup or
    /// reset only.
    pub fn set_ir(&mut self, spectra: &[Complex<f32>], k: usize) {
        debug_assert!(k <= self.max_k);
        debug_assert_eq!(spectra.len(), k * BINS);
        self.ir[..k * BINS].copy_from_slice(spectra);
        for s in self.ir[k * BINS..].iter_mut() {
            *s = Complex::new(0.0, 0.0);
        }
        self.k = k;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.fdl.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
        self.fdl_pos = 0;
        self.prev_in.fill(0.0);
        self.in_len = 0;
        self.out_fifo.fill(0.0);
        self.out_read = 0;
        self.out_fill = P;
    }

    /// Convolve `input` → `output` (equal lengths, any size). Allocation-free.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        debug_assert_eq!(input.len(), output.len());
        for (inp, out) in input.iter().zip(output.iter_mut()) {
            self.in_fifo[self.in_len] = *inp;
            self.in_len += 1;
            if self.in_len == P {
                self.run_partition();
                self.in_len = 0;
            }
            // Emit one sample from the output FIFO (primed with P zeros).
            *out = self.out_fifo[self.out_read];
            self.out_read = (self.out_read + 1) % P;
        }
    }

    /// One P-sample UPOLS step: FFT the [prev|current] window, push to the FDL,
    /// multiply-accumulate against the IR partitions, inverse-FFT, and queue the
    /// last P samples (overlap-save discards the first P).
    fn run_partition(&mut self) {
        // window = [prev_in | in_fifo]
        self.time[..P].copy_from_slice(&self.prev_in);
        self.time[P..].copy_from_slice(&self.in_fifo);
        self.fft
            .process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch_fwd)
            .expect("forward FFT length matches planner");

        // Store newest spectrum into the FDL.
        let pos = self.fdl_pos;
        self.fdl[pos * BINS..pos * BINS + BINS].copy_from_slice(&self.spectrum);

        // acc = Σ_{j<k} ir[j] · fdl[(pos - j) mod k]
        for a in self.acc.iter_mut() {
            *a = Complex::new(0.0, 0.0);
        }
        for j in 0..self.k {
            let slot = (pos + self.k - j) % self.k;
            let ir = &self.ir[j * BINS..j * BINS + BINS];
            let x = &self.fdl[slot * BINS..slot * BINS + BINS];
            for (a, (h, xx)) in self.acc.iter_mut().zip(ir.iter().zip(x.iter())) {
                *a += *h * *xx;
            }
        }

        // inverse FFT → time (realfft mutates the spectrum input).
        self.ifft
            .process_with_scratch(&mut self.acc, &mut self.time, &mut self.scratch_inv)
            .expect("inverse FFT length matches planner");
        let scale = 1.0 / N as f32;
        for (slot, &y) in self.out_fifo.iter_mut().zip(self.time[P..].iter()) {
            *slot = y * scale; // last P samples (overlap-save)
        }
        // The output FIFO is a simple P-ring read in lockstep with writes here;
        // out_read continues from where it was, out_fifo now holds this block.

        self.prev_in.copy_from_slice(&self.in_fifo);
        if self.k > 0 {
            self.fdl_pos = (self.fdl_pos + 1) % self.k;
        }
    }
}
```

Note on the output FIFO: because `set_ir`/`reset` prime it with `P` zeros and `run_partition` overwrites all `P` slots exactly when `in_len` wraps, the per-sample read in `process` lags writes by exactly one partition → `P` latency, contiguous. (The golden test in Step 3 verifies the alignment; if an off-by-one appears, fix the read/write phasing there — the test is authoritative.)

- [ ] **Step 3: Tests** (append to `partitioned_conv.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Direct time-domain convolution reference: out[n] = Σ_k ir[k]·in[n-k].
    fn direct_conv(input: &[f32], ir: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; input.len()];
        for n in 0..input.len() {
            let mut acc = 0.0f32;
            for (k, &h) in ir.iter().enumerate() {
                if n >= k {
                    acc += h * input[n - k];
                }
            }
            out[n] = acc;
        }
        out
    }

    /// Partition `ir` into k blocks of P, zero-pad each to N, forward-FFT.
    fn bake_spectra(ir: &[f32]) -> (Vec<Complex<f32>>, usize) {
        let k = ir.len().div_ceil(P).max(1);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let mut spectra = vec![Complex::new(0.0, 0.0); k * BINS];
        let mut block = vec![0.0f32; N];
        let mut sp = fft.make_output_vec();
        for j in 0..k {
            block.iter_mut().for_each(|x| *x = 0.0);
            let start = j * P;
            let end = (start + P).min(ir.len());
            block[..end - start].copy_from_slice(&ir[start..end]);
            fft.process(&mut block, &mut sp).unwrap();
            spectra[j * BINS..j * BINS + BINS].copy_from_slice(&sp);
        }
        (spectra, k)
    }

    #[test]
    fn upols_matches_direct_convolution_delayed_by_p() {
        use std::f32::consts::PI;
        // IR spanning several partitions; input a tone.
        let ir: Vec<f32> = (0..1500).map(|n| (-(n as f32) / 400.0).exp() * (0.13 * n as f32).sin()).collect();
        let total = 6000;
        let input: Vec<f32> = (0..total).map(|n| (2.0 * PI * 300.0 * n as f32 / 48000.0).sin()).collect();
        let reference = direct_conv(&input, &ir);

        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let mut out = vec![0.0f32; total];
        // Feed in odd-sized chunks to exercise the FIFO with arbitrary blocks.
        let mut off = 0;
        for &chunk in [100usize, 512, 333, 1024, 7].iter().cycle() {
            if off >= total { break; }
            let b = chunk.min(total - off);
            let (i, o) = (&input[off..off + b], &mut out[off..off + b]);
            conv.process(i, o);
            off += b;
        }
        // UPOLS output is the convolution delayed by the convolver's latency.
        let lat = conv.latency();
        for n in lat..total - 10 {
            let got = out[n];
            let want = reference[n - lat];
            assert!((got - want).abs() <= 1e-3 + 1e-3 * want.abs(), "n={n}: {got} vs {want}");
        }
    }

    #[test]
    fn impulse_in_yields_the_ir() {
        let ir: Vec<f32> = (0..900).map(|n| if n % 7 == 0 { 0.5 } else { -0.2 } * (-(n as f32) / 300.0).exp()).collect();
        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let total = 2048;
        let mut input = vec![0.0f32; total];
        input[0] = 1.0;
        let mut out = vec![0.0f32; total];
        conv.process(&input, &mut out);
        let lat = conv.latency();
        for n in 0..ir.len() {
            assert!((out[n + lat] - ir[n]).abs() <= 1e-3, "tap {n}: {} vs {}", out[n + lat], ir[n]);
        }
    }

    #[test]
    fn reset_clears_state() {
        let ir = vec![1.0f32; 600];
        let (spectra, k) = bake_spectra(&ir);
        let mut conv = PartitionedConvolver::new(ir.len());
        conv.set_ir(&spectra, k);
        let mut out = vec![0.0f32; 2000];
        let mut input = vec![0.0f32; 2000];
        input[0] = 1.0;
        conv.process(&input, &mut out);
        conv.reset();
        let mut z = vec![0.0f32; 2000];
        conv.process(&vec![0.0f32; 2000], &mut z);
        assert!(z.iter().all(|&v| v.abs() <= 1e-6), "reset should silence");
    }
}
```

- [ ] **Step 4: Run + lint.** `cargo nextest run -p tract-dsp --features partitioned-conv partitioned_conv` (3 pass) — if `upols_matches_direct_convolution_delayed_by_p` reveals the latency isn't exactly `P`, adjust the FIFO read/write phasing in `run_partition`/`process` and update `latency()` to the measured value (the test is authoritative). Then `cargo clippy -p tract-dsp --features partitioned-conv -- -D warnings`, `cargo fmt -p tract-dsp`.

- [ ] **Step 5: Commit.** (if authorized) `git add tract-dsp/src/partitioned_conv.rs tract-dsp/src/lib.rs tract-dsp/Cargo.toml Cargo.lock && git commit` — `feat(tract-dsp): uniformly-partitioned overlap-save convolver`.

---

## Task 2: Analytic IR bake (`nap/src/ir.rs`)

**Files:**
- Create: `nap/src/ir.rs`
- Modify: `nap/src/lib.rs` (`pub mod ir;`)
- Modify: `nap/Cargo.toml` (enable `tract-dsp`'s `partitioned-conv` feature)

Builds the dense IR `h[0..L]` of the wet path (exactly matching `ReverbChannel`'s filter math), then its partition spectra, per channel.

- [ ] **Step 1: Enable the feature** in `nap/Cargo.toml`: change the `tract-dsp` dep to `tract-dsp = { path = "../tract-dsp", features = ["partitioned-conv"] }`.

- [ ] **Step 2: Write `nap/src/ir.rs`.** It reuses `crate::coloration::{Dictionary, OnePole, Q}`, `crate::sequence::VelvetSequence`, `crate::engine::{BLOCK, MAX_TAIL_SECONDS, SETTLE_SAMPLES}` (re-export `SETTLE_SAMPLES`/`DC_BLOCKER_R` as `pub(crate)` from engine.rs if not already), and `tract_dsp::partitioned_conv::{P, N, BINS}`.

```rust
//! Analytic bake of the wet-path impulse response, and its UPOLS partition
//! spectra, for the Efficient (FFT) engine. GUI/setup thread only.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use crate::coloration::{Dictionary, OnePole, Q};
use crate::engine::{DC_BLOCKER_R, MAX_TAIL_SECONDS, SETTLE_SAMPLES};
use crate::sequence::VelvetSequence;
use tract_dsp::partitioned_conv::{BINS, N, P};

/// Max IR length: max tail + filter settle, rounded up to a whole partition.
pub fn max_ir_len(sample_rate: f32) -> usize {
    (((MAX_TAIL_SECONDS + 0.1) * sample_rate) as usize + SETTLE_SAMPLES)
        .div_ceil(P)
        * P
}

/// A baked per-channel IR as UPOLS partition spectra. Pre-allocated to the max.
pub struct IrSpectra {
    pub spectra: Vec<Complex<f32>>, // max_k * BINS, only k*BINS valid
    pub k: usize,
}

impl IrSpectra {
    pub fn new(sample_rate: f32) -> Self {
        let max_k = max_ir_len(sample_rate).div_ceil(P).max(1);
        Self {
            spectra: vec![Complex::new(0.0, 0.0); max_k * BINS],
            k: 0,
        }
    }
    pub fn copy_from(&mut self, other: &IrSpectra) {
        self.k = other.k;
        self.spectra[..self.k * BINS].copy_from_slice(&other.spectra[..self.k * BINS]);
    }
}

/// Reusable GUI-thread baker (owns the IR scratch + FFT planner).
pub struct IrBaker {
    sample_rate: f32,
    dict: Dictionary,
    post: OnePole,
    /// Per-filter sparse excitation buffers + the summed/filtered IR, length max_ir_len.
    band: Vec<f32>,
    h: Vec<f32>,
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    fft_block: Vec<f32>,
    fft_out: Vec<Complex<f32>>,
}

impl IrBaker {
    pub fn new(sample_rate: f32) -> Self {
        let l = max_ir_len(sample_rate);
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N);
        let fft_out = fft.make_output_vec();
        Self {
            sample_rate,
            dict: Dictionary::new(sample_rate),
            post: OnePole::new(12_000.0, sample_rate),
            band: vec![0.0; l],
            h: vec![0.0; l],
            fft,
            fft_block: vec![0.0; N],
            fft_out,
        }
    }

    /// Bake the IR for the channel whose taps are `location` (use `seq.location`
    /// for L, `seq.location_r` for R) into `out`. `coeff`/`filter_idx` shared.
    pub fn bake(&mut self, seq: &VelvetSequence, location: &[u32], out: &mut IrSpectra) {
        let l = (seq.tail_len + SETTLE_SAMPLES).min(self.h.len());
        self.h[..l].iter_mut().for_each(|x| *x = 0.0);

        // For each coloration filter: place its pulses, run the one-pole, add to h.
        for q in 0..Q {
            self.band[..l].iter_mut().for_each(|x| *x = 0.0);
            for m in 0..seq.count {
                if seq.filter_idx[m] as usize == q {
                    let loc = location[m] as usize;
                    if loc < l {
                        self.band[loc] += seq.coeff[m];
                    }
                }
            }
            let mut f = self.dict.filters[q];
            f.reset();
            for i in 0..l {
                self.h[i] += f.process(self.band[i]);
            }
        }

        // Post-LP then DC blocker over h, exactly matching ReverbChannel.
        let mut post = self.post;
        post.reset();
        let (mut dc_x1, mut dc_y1) = (0.0f32, 0.0f32);
        for i in 0..l {
            let p = post.process(self.h[i]);
            let y = p - dc_x1 + DC_BLOCKER_R * dc_y1;
            dc_x1 = p;
            dc_y1 = y;
            self.h[i] = y;
        }

        // Partition h into k blocks of P, zero-pad to N, forward-FFT.
        let k = l.div_ceil(P).max(1);
        for j in 0..k {
            self.fft_block.iter_mut().for_each(|x| *x = 0.0);
            let start = j * P;
            let end = (start + P).min(l);
            self.fft_block[..end - start].copy_from_slice(&self.h[start..end]);
            self.fft.process(&mut self.fft_block, &mut self.fft_out).unwrap();
            out.spectra[j * BINS..j * BINS + BINS].copy_from_slice(&self.fft_out);
        }
        out.k = k;
        let _ = self.sample_rate;
    }
}
```

(`OnePole` must be `Copy` or expose `reset()`; it already has `reset()`. If `self.dict.filters[q]` can't be copied, take `&mut` and `reset()` it instead — adjust to the real `OnePole` API. Make `DC_BLOCKER_R` and `SETTLE_SAMPLES` `pub(crate)` in `engine.rs`.)

- [ ] **Step 3: Test** — the analytic IR equals the impulse response of the real `ReverbChannel` at a small setting:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ReverbChannel;
    use crate::rng::Rng;

    #[test]
    fn analytic_ir_matches_engine_impulse_response() {
        // Small seq so the engine impulse response is cheap to capture.
        let mut seq = VelvetSequence::new();
        let n = 50;
        seq.count = n;
        let mut rng = Rng::new(8);
        for m in 0..n {
            let loc = (rng.next_u64() % 300) as u32;
            seq.location[m] = loc;
            seq.location_r[m] = loc;
            seq.coeff[m] = (rng.next_f32() * 2.0 - 1.0) * 0.2;
            seq.filter_idx[m] = (rng.next_u64() % Q as u64) as u8;
        }
        seq.tail_len = 300;
        let l = seq.tail_len + SETTLE_SAMPLES;

        // Engine impulse response.
        let mut ch = ReverbChannel::new(48_000.0);
        let mut ir_engine = vec![0.0f32; l];
        ir_engine[0] = ch.process(1.0, &seq, &seq.location);
        for s in ir_engine.iter_mut().skip(1) {
            *s = ch.process(0.0, &seq, &seq.location);
        }

        // Analytic IR (inverse-FFT the baked spectra back, or compare h directly):
        // bake, then reconstruct h from partitions and compare.
        let mut baker = IrBaker::new(48_000.0);
        let mut spec = IrSpectra::new(48_000.0);
        baker.bake(&seq, &seq.location, &mut spec);
        // Reconstruct time-domain IR from the partition spectra via inverse FFT.
        let mut planner = RealFftPlanner::<f32>::new();
        let ifft = planner.plan_fft_inverse(N);
        let mut recon = vec![0.0f32; l];
        let mut tmp = ifft.make_output_vec();
        for j in 0..spec.k {
            let mut sp: Vec<Complex<f32>> = spec.spectra[j * BINS..j * BINS + BINS].to_vec();
            ifft.process(&mut sp, &mut tmp).unwrap();
            let scale = 1.0 / N as f32;
            for i in 0..P {
                let idx = j * P + i;
                if idx < l {
                    recon[idx] = tmp[i] * scale;
                }
            }
        }
        for n in 0..l.min(1000) {
            assert!((recon[n] - ir_engine[n]).abs() <= 1e-3 + 1e-3 * ir_engine[n].abs(),
                "ir tap {n}: analytic {} vs engine {}", recon[n], ir_engine[n]);
        }
    }
}
```

- [ ] **Step 4: Run + lint + commit** (`cargo nextest run -p nap ir`; clippy; fmt). `feat(nap): analytic IR bake for the FFT engine`.

---

## Task 3: `IrHandoff` (RT-safe GUI→audio publish)

**Files:** Modify: `nap/src/handoff.rs`

- [ ] **Step 1:** Mirror `SequenceHandoff` for `IrSpectra` (one per channel — store an L+R pair, or two handoffs). Use `Mutex<(IrSpectra, IrSpectra)>` + `AtomicU64` generation; `publish(&l, &r)` copies in + bumps generation; `try_read_into(&mut l, &mut r, &mut gen) -> bool` copies only when the generation changed. Same shape/tests as `SequenceHandoff` (first-read-picks-up, unchanged-skips, newest-wins). Code mirrors the existing `SequenceHandoff` exactly with `IrSpectra::copy_from`.

- [ ] **Step 2: Tests** mirroring the three `SequenceHandoff` tests. **Step 3:** run/lint/commit — `feat(nap): IrHandoff for baked IR spectra`.

---

## Task 4: `mode` param + `process()` routing + mode-equivalence test

**Files:** Modify: `nap/src/lib.rs`

- [ ] **Step 1: Add the mode enum + param** (mirror miff's `MiffMode`):

```rust
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum NapMode {
    #[id = "zero-latency"]
    #[name = "Zero Latency"]
    ZeroLatency,
    #[id = "efficient"]
    #[name = "Efficient"]
    Efficient,
}
```
Add to `NapParams`: `#[id = "mode"] pub mode: EnumParam<NapMode>` initialized `EnumParam::new("Engine", NapMode::ZeroLatency).non_automatable()`.

- [ ] **Step 2: Add audio-thread state** to `Nap`: per-channel `PartitionedConvolver` (`left_conv`, `right_conv`, sized `ir::max_ir_len(sr)`), an `IrHandoff` (`Arc`), the audio-thread `IrSpectra` pair + `ir_gen`, a per-channel `P`-sample dry-delay ring for Efficient alignment, and `last_mode: NapMode` for click-safe reset. Build convolvers in `initialize` at the real sample rate; install the baked IR via `set_ir`.

- [ ] **Step 3: `Nap::bake_ir`** (GUI/setup helper, sibling of `regenerate`): lock the curves, build the `VelvetSequence` (reuse `regenerate`'s generated sequence), run `IrBaker::bake` for L (`seq.location`) and R (`seq.location_r`), publish via the `IrHandoff`. Called from `initialize` and the editor (Efficient mode).

- [ ] **Step 4: Route `process()`:**
  - Read `mode`. On a change vs `last_mode`: `reset()` both convolvers + dry-delay, set `context.set_latency_samples(if Efficient { P } else { 0 })`, update `last_mode`.
  - `try_read_into` the sequence (always) and the IR (always; cheap when unchanged).
  - **ZeroLatency:** unchanged existing path; reported latency 0.
  - **Efficient:** for each sub-block, gain the dry → feed `left_conv.process`/`right_conv.process` → wet; delay the dry by `P` (per-channel ring) so it aligns with the `P`-late wet; then the SAME per-sample pre-delay + dry/wet mix + output gain as now.
  - Keep the idle/silence handling sensible (Efficient's convolver naturally outputs ~0 on silent input; the existing input-silence `ProcessStatus::Tail` logic still applies).

- [ ] **Step 5: Mode-equivalence test** (the headline gate). Build a non-trivial seq, bake its IR, run an input stream through (a) the ZeroLatency path and (b) the Efficient path, and assert Efficient output (shifted back by `P`) matches ZeroLatency within tolerance. Since constructing a full `Buffer` is awkward, test at the engine+convolver level: feed the same input through `ReverbChannel::process` (per sample) and through a `PartitionedConvolver` loaded with the baked IR, compare `efficient[n+P] ≈ zerolat[n]` within `1e-3`. This proves the two engines are the same system.

```rust
#[test]
fn efficient_matches_zero_latency_within_tolerance() {
    use crate::engine::ReverbChannel;
    use crate::ir::{IrBaker, IrSpectra};
    use crate::rng::Rng;
    use tract_dsp::partitioned_conv::{PartitionedConvolver, P};

    let mut seq = VelvetSequence::new();
    let n = 200;
    seq.count = n;
    let mut rng = Rng::new(21);
    for m in 0..n {
        let loc = (rng.next_u64() % 1500) as u32;
        seq.location[m] = loc;
        seq.location_r[m] = loc;
        seq.coeff[m] = (rng.next_f32() * 2.0 - 1.0) * 0.05;
        seq.filter_idx[m] = (rng.next_u64() % crate::coloration::Q as u64) as u8;
    }
    seq.tail_len = 1500;

    let total = 8000;
    let mut rng = Rng::new(2);
    let input: Vec<f32> = (0..total).map(|_| rng.next_f32() * 2.0 - 1.0).collect();

    // Zero-Latency reference.
    let mut ch = ReverbChannel::new(48_000.0);
    let zl: Vec<f32> = input.iter().map(|&x| ch.process(x, &seq, &seq.location)).collect();

    // Efficient: baked IR through the convolver.
    let mut baker = IrBaker::new(48_000.0);
    let mut spec = IrSpectra::new(48_000.0);
    baker.bake(&seq, &seq.location, &mut spec);
    let mut conv = PartitionedConvolver::new(crate::ir::max_ir_len(48_000.0));
    conv.set_ir(&spec.spectra[..spec.k * tract_dsp::partitioned_conv::BINS], spec.k);
    let mut eff = vec![0.0f32; total];
    conv.process(&input, &mut eff);

    for nn in P..total - 10 {
        assert!((eff[nn] - zl[nn - P]).abs() <= 1e-3 + 1e-3 * zl[nn - P].abs(),
            "n={nn}: efficient {} vs zero-latency {}", eff[nn], zl[nn - P]);
    }
}
```

- [ ] **Step 6: Run + lint + commit.** `cargo nextest run -p nap` (all pass incl. equivalence); clippy `-p nap`; fmt. `feat(nap): Engine mode param + Efficient FFT routing`.

---

## Task 5: Editor mode selector + drag-deferred IR bake

**Files:** Modify: `nap/src/editor.rs`

- [ ] **Step 1:** Add a `HitAction::ModeSelector` and a stepped selector (Zero Latency / Efficient) in the bottom strip — mirror miff's `HitAction::ModeSelector` region + click handling (cycles/toggles the 2-value `mode` param via the param setter). Use the shared `tiny_skia_widgets` stepped-selector/controls.
- [ ] **Step 2: Drag-deferred IR bake.** The editor already calls `regenerate()` (sequence) on edits. Add: when `mode == Efficient`, call `bake_ir()` — but DEFER it during a continuous MSEG node drag: bake on drag-release (mouse-up) and on discrete edits (node add/delete, dial change, mode→Efficient switch), NOT on every `on_mouse_move`. Track drag state (the editor/`MsegEditState` already knows). The sequence + tail-viz still update live every move.
- [ ] **Step 3:** Selecting Efficient triggers a `bake_ir()` if the IR is stale.
- [ ] **Step 4: Build the standalone, manual check** (`cargo build --bin nap`): switching modes works, Efficient sounds the same as Zero Latency (just latency-shifted), large-Size curve drags stay smooth (audio updates on release). Run/lint/fmt/commit — `feat(nap): engine-mode selector + drag-deferred IR bake`.

---

## Task 6: Docs + workspace verification

**Files:** Modify: `docs/nap/nap-manual.md`, `CLAUDE.md`, `README.md` (if it lists features), `nap/examples/nap_profile.rs` (add an Efficient-mode scenario for comparison).

- [ ] **Step 1:** Document the two engines in the manual (the latency tradeoff, when to use each). Add a `**nap** … ir.rs / partitioned_conv` note to CLAUDE.md, and a `tract_dsp::partitioned_conv` line to its tract-dsp section.
- [ ] **Step 2:** Extend `nap_profile.rs` to also measure the Efficient path (build a baked IR + `PartitionedConvolver`, run the same matrix) so the CPU win is quantified alongside Zero Latency.
- [ ] **Step 3: Workspace gates.** `cargo nextest run --workspace`; `cargo clippy --workspace -- -D warnings`; `cargo fmt --check` (nap-scoped); `cargo nih-plug bundle nap --release`. Commit docs — `docs(nap): document the dual engine + Efficient bench scenario`.

---

## Self-review notes (for the planner)
- **Spec coverage:** PartitionedConvolver/UPOLS (Task 1), analytic IR bake (Task 2), IrHandoff (Task 3), mode param + routing + dry-delay + latency + equivalence (Task 4), editor selector + drag-deferred bake (Task 5), docs/bench/verify (Task 6). All spec sections mapped.
- **Latency:** the convolver declares `latency() == P`; lib.rs reports it + dry-delays by `P`; the equivalence test asserts `efficient[n+P] == zero_latency[n]`. If Task 1's golden test shows the true latency differs from `P`, that value propagates (convolver `latency()`, the dry-delay, the equivalence shift) — fix consistently.
- **No sound change:** the mode-equivalence test (Task 4) is the gate; the IR-bake-matches-engine test (Task 2) backs it.
- **Known verify-against-reality:** `OnePole` Copy/reset (Task 2 note), `realfft::num_complex` re-export (else add `rustfft` to the feature), the stepped-selector widget API (mirror miff). Each flagged inline.
- **Adversarial review:** after Task 4 and Task 1, the UPOLS convolver + the equivalence claim should get an adversarial correctness review (RT-safety, FFT alignment/scaling, the FIFO latency phasing) before final.
