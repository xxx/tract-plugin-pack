//! Profiling harness for miff.
//!
//! Drives miff's per-block DSP directly — the convolution engine
//! (`RawChannel` / `PhaselessChannel`) plus the per-sample input-spectrum
//! ring write — bypassing nih-plug's `Buffer` plumbing and the parameter
//! smoothers. Measures the same hot path the real plugin runs in a host.
//!
//! Build (with target-cpu auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example miff_profile -p miff
//!
//! Profile a single scenario with perf (comment out the others in `main`):
//!     perf record -F 999 -g -- target/profiling/examples/miff_profile
//!     perf report --no-children -g graph,0.5
//!
//! Each scenario reports a real-time factor (audio seconds per wall-clock
//! second) and an approximate CPU-per-instance figure. A summary table at the
//! end makes baseline-vs-optimization comparison easy.
//!
//! Scenarios exercise miff's cost-relevant branches:
//!  - AT REST: silent input — once the convolution history flushes to zero,
//!    `RawChannel::process` takes its silence fast-path and skips the 2048-tap
//!    SIMD MAC loop. What a host's DSP graph shows at idle.
//!  - Raw / Length sweep: the SIMD f32x16 MAC cost scales linearly with the
//!    `Length` (tap count). 64 → 4096 spans the parameter's whole range.
//!  - Phaseless: a fixed 4096-point STFT with 50% overlap-add. Its cost is
//!    independent of `Length`, so a single active Phaseless scenario suffices.
//!
//! The editor-only input-spectrum FFT (throttled to ~30 Hz, gated on the
//! editor being open) is deliberately NOT mirrored — it is a fixed overhead
//! that does not vary by scenario. The always-on per-sample input-ring write
//! IS mirrored, since `process()` runs it every sample regardless.

use std::hint::black_box;
use std::time::Instant;

use miff::convolution::{PhaselessChannel, RawChannel};
use miff::kernel::{self, Kernel};
use tiny_skia_widgets::mseg::MsegData;

const SR: f32 = 48_000.0;
const BLOCK: usize = 1024;
const N_BLOCKS: usize = 20_000; // ~427 s of audio at SR

/// Mirrors the private `ISPECTRUM_FFT` in `miff/src/lib.rs` — the input-ring
/// length. Must stay a power of two (the ring index masks with `LEN - 1`).
const ISPECTRUM_FFT: usize = 2048;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Mode {
    Raw,
    Phaseless,
}

#[derive(Copy, Clone, Debug)]
struct Scenario {
    name: &'static str,
    mode: Mode,
    /// Kernel tap count (Raw). Phaseless uses a fixed 4096-point STFT, so for
    /// Phaseless this only affects the baked kernel, not the per-sample cost.
    length: usize,
    /// Silent input — exercises the Raw silence fast-path / host-idle case.
    at_rest: bool,
}

struct RunResult {
    scenario: Scenario,
    rt_factor: f64,
    cpu_pct_per_instance: f64,
}

/// Bake a non-trivial kernel: `MsegData::default()` is a 0→1 ramp, which the
/// bipolar tap map turns into a genuine (non-zero) FIR — so the convolution
/// actually runs instead of short-circuiting on `Kernel::is_zero`.
fn active_kernel(length: usize) -> Kernel {
    let k = kernel::bake(&MsegData::default(), length);
    assert!(!k.is_zero, "ramp curve must bake to a non-zero kernel");
    k
}

fn run(scenario: Scenario) -> RunResult {
    eprintln!(
        "── {} (mode={:?}, length={}, at_rest={}) ──",
        scenario.name, scenario.mode, scenario.length, scenario.at_rest,
    );

    let kernel = active_kernel(scenario.length);

    let mut raw: [RawChannel; 2] = [RawChannel::new(), RawChannel::new()];
    let mut phaseless: [PhaselessChannel; 2] = [PhaselessChannel::new(), PhaselessChannel::new()];
    for ch in raw.iter_mut() {
        ch.reset();
    }
    for ch in phaseless.iter_mut() {
        ch.reset();
    }

    // Fixed mix/gain — the real `process()` applies `(dry + (wet-dry)*mix) *
    // gain` per sample; the smoothers themselves are negligible and skipped.
    const MIX: f32 = 1.0;
    const GAIN: f32 = 1.0;

    let mut l = vec![0.0_f32; BLOCK];
    let mut r = vec![0.0_f32; BLOCK];

    // Always-on per-sample input-spectrum ring write, mirrored from process().
    let mut input_ring = vec![0.0_f32; ISPECTRUM_FFT];
    let mut ring_pos = 0_usize;

    let total_samples: u64 = N_BLOCKS as u64 * BLOCK as u64;
    let start = Instant::now();

    for block in 0..N_BLOCKS {
        // Generate the input block. Silent at rest; otherwise a stereo chirp
        // sweeping 100 Hz → 10 kHz exponentially over 4 s, looping, with a
        // small L/R phase offset.
        if scenario.at_rest {
            l.fill(0.0);
            r.fill(0.0);
        } else {
            let phase_offset = (block * BLOCK) as f32 / SR;
            for i in 0..BLOCK {
                let t = phase_offset + i as f32 / SR;
                let cycle = (t % 4.0) / 4.0;
                let f = 100.0 * 100.0_f32.powf(cycle);
                let phase = 2.0 * std::f32::consts::PI * f * t;
                l[i] = phase.sin() * 0.3;
                r[i] = (phase + 0.05).cos() * 0.3;
            }
        }

        // Mirror process()'s per-sample loop: convolve each channel, apply the
        // dry/wet mix and output gain, accumulate the mono input ring.
        for i in 0..BLOCK {
            let dry_l = l[i];
            let dry_r = r[i];

            let (wet_l, wet_r) = match scenario.mode {
                Mode::Raw => (
                    raw[0].process(dry_l, &kernel),
                    raw[1].process(dry_r, &kernel),
                ),
                Mode::Phaseless => (
                    phaseless[0].process(dry_l, &kernel),
                    phaseless[1].process(dry_r, &kernel),
                ),
            };

            l[i] = (dry_l + (wet_l - dry_l) * MIX) * GAIN;
            r[i] = (dry_r + (wet_r - dry_r) * MIX) * GAIN;

            let mono = (dry_l + dry_r) * 0.5;
            input_ring[ring_pos] = mono;
            ring_pos = (ring_pos + 1) & (ISPECTRUM_FFT - 1);
        }

        // Prevent the optimizer from eliding the outputs or the ring write.
        black_box(&l);
        black_box(&r);
        black_box(&input_ring);
    }

    let elapsed = start.elapsed();
    let audio_seconds = total_samples as f64 / SR as f64;
    let rt_factor = audio_seconds / elapsed.as_secs_f64();
    let cpu_pct = 100.0 / rt_factor;

    eprintln!(
        "  audio={:.1}s wall={:.3}s  RT={:>7.1}×  cpu/inst={:>6.4}%",
        audio_seconds,
        elapsed.as_secs_f64(),
        rt_factor,
        cpu_pct,
    );
    eprintln!();

    RunResult {
        scenario,
        rt_factor,
        cpu_pct_per_instance: cpu_pct,
    }
}

fn main() {
    let scenarios = [
        Scenario {
            name: "AT REST (silent) / Raw",
            mode: Mode::Raw,
            length: 256,
            at_rest: true,
        },
        Scenario {
            name: "Raw / Length 64",
            mode: Mode::Raw,
            length: 64,
            at_rest: false,
        },
        Scenario {
            name: "Raw / Length 256 (default)",
            mode: Mode::Raw,
            length: 256,
            at_rest: false,
        },
        Scenario {
            name: "Raw / Length 1024",
            mode: Mode::Raw,
            length: 1024,
            at_rest: false,
        },
        Scenario {
            name: "Raw / Length 4096 (max)",
            mode: Mode::Raw,
            length: 4096,
            at_rest: false,
        },
        Scenario {
            name: "Phaseless / Length 256",
            mode: Mode::Phaseless,
            length: 256,
            at_rest: false,
        },
        Scenario {
            name: "AT REST (silent) / Phaseless",
            mode: Mode::Phaseless,
            length: 256,
            at_rest: true,
        },
    ];

    let results: Vec<RunResult> = scenarios.iter().map(|s| run(*s)).collect();

    eprintln!("─────── summary ───────");
    eprintln!("{:<32}  {:>9}  {:>9}", "scenario", "RT-factor", "cpu/inst");
    for r in &results {
        eprintln!(
            "{:<32}  {:>8.1}×  {:>8.4}%",
            r.scenario.name, r.rt_factor, r.cpu_pct_per_instance
        );
    }
}
