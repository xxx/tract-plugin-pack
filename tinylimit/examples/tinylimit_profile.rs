//! Profiling harness for Tinylimit.
//!
//! Drives Tinylimit's per-block DSP directly — threshold boost → limiter
//! (`process_block`) → output-peak metering — bypassing nih-plug's `Buffer`
//! plumbing and the parameter smoothers. Measures the same hot path the real
//! plugin runs in a host.
//!
//! Build (with target-cpu auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example tinylimit_profile -p tinylimit
//!
//! Profile a single scenario with perf (comment out the others in `main`):
//!     perf record -F 999 -g -- target/profiling/examples/tinylimit_profile
//!     perf report --no-children -g graph,0.5
//!
//! Each scenario reports a real-time factor (audio seconds per wall-clock
//! second) and an approximate CPU-per-instance figure. A summary table at the
//! end makes baseline-vs-optimization comparison easy.
//!
//! Scenarios exercise Tinylimit's cost-relevant branches:
//!  - AT REST: silent input + benign params — what a host's DSP graph shows
//!    at idle. Two rows: ISP off, and ISP on (the true-peak FIR runs every
//!    sample regardless of level).
//!  - Hard knee (knee = 0): `process_block`'s fast path skips log10 for
//!    sub-threshold samples. Soft knee (knee > 0) always does log/exp.
//!  - ISP off vs on: the ITU-R BS.1770-4 polyphase true-peak detector is the
//!    single biggest optional cost.
//!  - 96 kHz: sample-rate scaling, and the true-peak detector switches from
//!    4× to 2× oversampling above 96 kHz.

use std::hint::black_box;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use tinylimit::limiter::Limiter;
use tinylimit::true_peak::TruePeakDetector;

const BLOCK: usize = 1024;
const N_BLOCKS: usize = 50_000; // ~1067 s of audio at 48 kHz

/// Mirrors the private `MAX_LOOKAHEAD_MS` in `tinylimit/src/lib.rs`.
const MAX_LOOKAHEAD_MS: f32 = 10.0;

// Fixed limiter character for the loud scenarios — a typical mastering setup.
const ATTACK_MS: f32 = 5.0;
const RELEASE_MS: f32 = 200.0;
const TRANSIENT_MIX: f32 = 0.5;
const STEREO_LINK: f32 = 1.0;
const THRESHOLD_DB: f32 = -12.0; // pushes the chirp well into limiting
const CEILING_DB: f32 = -0.3;

/// `ln(10) / 20` — converts dB to a linear gain via `(db * this).exp()`.
const LN10_OVER_20: f32 = std::f32::consts::LN_10 / 20.0;

#[derive(Copy, Clone, Debug)]
struct Scenario {
    name: &'static str,
    sample_rate: f32,
    /// Inter-sample (true) peak detection — runs the polyphase FIR per sample.
    isp: bool,
    /// 0.0 selects `process_block`'s hard-knee fast path; > 0 forces soft knee.
    knee_db: f32,
    /// Silent input + 0 dB threshold — the host-idle case.
    at_rest: bool,
}

struct RunResult {
    scenario: Scenario,
    rt_factor: f64,
    cpu_pct_per_instance: f64,
}

fn run(scenario: Scenario) -> RunResult {
    eprintln!(
        "── {} ({:.0} Hz, isp={}, knee={:.0} dB, at_rest={}) ──",
        scenario.name, scenario.sample_rate, scenario.isp, scenario.knee_db, scenario.at_rest,
    );

    let sr = scenario.sample_rate;

    let mut limiter = Limiter::new(sr, MAX_LOOKAHEAD_MS);
    limiter.set_params(ATTACK_MS, RELEASE_MS);
    limiter.set_max_block_size(BLOCK);
    limiter.reset();

    let mut true_peak: [TruePeakDetector; 2] = [TruePeakDetector::new(), TruePeakDetector::new()];
    for d in true_peak.iter_mut() {
        d.set_sample_rate(sr);
        d.reset();
    }

    let threshold_db = if scenario.at_rest { 0.0 } else { THRESHOLD_DB };
    // Threshold "boost": a lower threshold boosts the signal before limiting.
    let boost = (-threshold_db * LN10_OVER_20).exp();
    let ceiling_linear = (CEILING_DB * LN10_OVER_20).exp();

    let mut l = vec![0.0_f32; BLOCK];
    let mut r = vec![0.0_f32; BLOCK];

    // Stand-ins for the GUI meter atomics, so the optimizer can't elide the
    // limiter output or the output-peak scan.
    let out_peak = AtomicU32::new(0);
    let gr_meter = AtomicU32::new(0);

    let total_samples: u64 = N_BLOCKS as u64 * BLOCK as u64;
    let start = Instant::now();

    for block in 0..N_BLOCKS {
        // Generate the input block. Silent at rest; otherwise a stereo chirp
        // sweeping 100 Hz → 10 kHz exponentially over 4 s, looping, with a
        // small L/R phase offset. The threshold boost is folded into the
        // amplitude here (process() applies it as a separate per-sample
        // multiply — a negligible cost next to the limiter).
        if scenario.at_rest {
            l.fill(0.0);
            r.fill(0.0);
        } else {
            let phase_offset = (block * BLOCK) as f32 / sr;
            for i in 0..BLOCK {
                let t = phase_offset + i as f32 / sr;
                let cycle = (t % 4.0) / 4.0;
                let f = 100.0 * 100.0_f32.powf(cycle);
                let phase = 2.0 * std::f32::consts::PI * f * t;
                l[i] = phase.sin() * 0.5 * boost;
                r[i] = (phase + 0.05).cos() * 0.5 * boost;
            }
        }

        let true_peak_opt = if scenario.isp {
            Some(&mut true_peak)
        } else {
            None
        };
        let gr = limiter.process_block(
            &mut l,
            &mut r,
            scenario.knee_db,
            TRANSIENT_MIX,
            STEREO_LINK,
            ceiling_linear,
            true_peak_opt,
        );

        // Output-peak scan — mirrors process()'s post-limiter metering loop.
        let mut peak = 0.0_f32;
        for i in 0..BLOCK {
            peak = peak.max(l[i].abs()).max(r[i].abs());
        }
        out_peak.store(peak.to_bits(), Ordering::Relaxed);
        gr_meter.store(gr.to_bits(), Ordering::Relaxed);

        black_box(&l);
        black_box(&r);
    }

    let elapsed = start.elapsed();
    let audio_seconds = total_samples as f64 / sr as f64;
    let rt_factor = audio_seconds / elapsed.as_secs_f64();
    let cpu_pct = 100.0 / rt_factor;

    eprintln!(
        "  audio={:.1}s wall={:.3}s  RT={:>7.1}×  cpu/inst={:>5.3}%",
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
            name: "AT REST (silent) / ISP off",
            sample_rate: 48_000.0,
            isp: false,
            knee_db: 0.0,
            at_rest: true,
        },
        Scenario {
            name: "AT REST (silent) / ISP on",
            sample_rate: 48_000.0,
            isp: true,
            knee_db: 0.0,
            at_rest: true,
        },
        Scenario {
            name: "48kHz / hard knee / ISP off",
            sample_rate: 48_000.0,
            isp: false,
            knee_db: 0.0,
            at_rest: false,
        },
        Scenario {
            name: "48kHz / soft knee / ISP off",
            sample_rate: 48_000.0,
            isp: false,
            knee_db: 6.0,
            at_rest: false,
        },
        Scenario {
            name: "48kHz / hard knee / ISP on",
            sample_rate: 48_000.0,
            isp: true,
            knee_db: 0.0,
            at_rest: false,
        },
        Scenario {
            name: "48kHz / soft knee / ISP on",
            sample_rate: 48_000.0,
            isp: true,
            knee_db: 6.0,
            at_rest: false,
        },
        Scenario {
            name: "96kHz / hard knee / ISP on",
            sample_rate: 96_000.0,
            isp: true,
            knee_db: 0.0,
            at_rest: false,
        },
    ];

    let results: Vec<RunResult> = scenarios.iter().map(|s| run(*s)).collect();

    eprintln!("─────── summary ───────");
    eprintln!("{:<34}  {:>9}  {:>9}", "scenario", "RT-factor", "cpu/inst");
    for r in &results {
        eprintln!(
            "{:<34}  {:>8.1}×  {:>8.3}%",
            r.scenario.name, r.rt_factor, r.cpu_pct_per_instance
        );
    }
}
