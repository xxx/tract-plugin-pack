//! Profiling harness for `tract-dsp`.
//!
//! Drives the crate's audio-thread hot paths directly — the ITU-R BS.1770-4
//! true-peak detector and the lock-free SPSC ring — with no plugin or GUI
//! plumbing. Measures the same code `gs-meter`, `tinylimit`, and `imagine`
//! run in a host.
//!
//! Build (with target-cpu auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example tract_dsp_profile -p tract-dsp
//!
//! Profile a single scenario with perf (comment out the others in `main`):
//!     perf record -F 999 -g -- target/profiling/examples/tract_dsp_profile
//!     perf report --no-children -g graph,0.5
//!
//! Each per-sample scenario reports a real-time factor (audio seconds
//! processed per wall-clock second) and an approximate CPU-per-instance
//! figure. A summary table makes baseline-vs-optimization comparison easy.
//!
//! Scenarios exercise the crate's cost-relevant paths:
//!  - `TruePeakDetector` at 48 kHz (4× oversampling — all 4 polyphase phases),
//!    96 kHz (2× — 2 phases), and 192 kHz (bypass — early return). This is the
//!    single most expensive piece of shared DSP.
//!  - `process_sample` (running-max only) vs `process_sample_peak` (also
//!    returns the per-sample peak) — the latter is what tinylimit's ISP path
//!    calls.
//!  - SPSC ring `push` — the per-sample audio-thread cost imagine's
//!    vectorscope pays. The consumer-side `snapshot` is timed separately
//!    (GUI-thread, not a per-sample cost) and reported as ns/call.

use std::hint::black_box;
use std::time::Instant;

use tract_dsp::spsc;
use tract_dsp::true_peak::TruePeakDetector;

const BLOCK: usize = 1024;
const N_BLOCKS: usize = 50_000; // 51.2 M samples — ~1067 s of audio at 48 kHz

/// Which tract-dsp path a scenario drives.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Path {
    /// `TruePeakDetector::process_sample` — updates the running max only.
    TruePeak,
    /// `TruePeakDetector::process_sample_peak` — also returns the per-sample peak.
    TruePeakPeak,
    /// `spsc::Producer::push` — one (L, R) pair per sample.
    SpscPush,
}

#[derive(Copy, Clone, Debug)]
struct Scenario {
    name: &'static str,
    path: Path,
    sample_rate: f32,
}

struct RunResult {
    scenario: Scenario,
    rt_factor: f64,
    cpu_pct_per_instance: f64,
}

/// One sample of an exponential 100 Hz → 10 kHz chirp that loops every 4 s.
/// Gives the FIR real, varied input without constant-folding.
fn chirp_sample(n: usize, sr: f32) -> f32 {
    let t = n as f32 / sr;
    let cycle = (t % 4.0) / 4.0;
    let f = 100.0 * 100.0_f32.powf(cycle);
    (2.0 * std::f32::consts::PI * f * t).sin() * 0.5
}

fn run(scenario: Scenario) -> RunResult {
    eprintln!("── {} ({:.0} Hz) ──", scenario.name, scenario.sample_rate);
    let sr = scenario.sample_rate;
    let total_samples: usize = N_BLOCKS * BLOCK;

    // Pre-generate one full 4 s chirp period BEFORE timing starts, so the
    // timed loop measures only the DSP under test — not the sin/powf of
    // signal generation. The DSP is fed this buffer cyclically.
    let signal: Vec<f32> = (0..(4.0 * sr) as usize)
        .map(|n| chirp_sample(n, sr))
        .collect();

    let start = Instant::now();
    match scenario.path {
        Path::TruePeak => {
            let mut det = TruePeakDetector::new();
            det.set_sample_rate(sr);
            det.reset();
            for n in 0..total_samples {
                det.process_sample(black_box(signal[n % signal.len()]));
            }
            black_box(det.true_peak_max());
        }
        Path::TruePeakPeak => {
            let mut det = TruePeakDetector::new();
            det.set_sample_rate(sr);
            det.reset();
            let mut acc = 0.0_f32;
            for n in 0..total_samples {
                acc += det.process_sample_peak(black_box(signal[n % signal.len()]));
            }
            black_box(acc);
        }
        Path::SpscPush => {
            let (prod, _cons) = spsc::channel(65_536);
            for n in 0..total_samples {
                let s = signal[n % signal.len()];
                prod.push(black_box(s), black_box(-s));
            }
        }
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

/// Time the consumer-side `snapshot`. This is a GUI-thread call (~60 Hz in the
/// real plugin), not a per-sample cost, so it is reported as ns/call rather
/// than folded into the real-time-factor table.
fn time_snapshot() {
    const CAP: usize = 65_536;
    const COPY: usize = 4_096; // a typical vectorscope decimation window
    const ITERS: usize = 100_000;

    let (prod, cons) = spsc::channel(CAP);
    for i in 0..CAP {
        prod.push(i as f32, -(i as f32));
    }
    let mut a = vec![0.0_f32; COPY];
    let mut b = vec![0.0_f32; COPY];

    let start = Instant::now();
    let mut total = 0_usize;
    for _ in 0..ITERS {
        total += cons.snapshot_oldest_first(COPY, &mut a, &mut b);
    }
    let elapsed = start.elapsed();
    black_box(total);

    let ns_per_call = elapsed.as_nanos() as f64 / ITERS as f64;
    eprintln!("── SPSC snapshot ({COPY}-sample copy) ──");
    eprintln!(
        "  {ITERS} calls  wall={:.3}s  {ns_per_call:.1} ns/call",
        elapsed.as_secs_f64(),
    );
    eprintln!();
}

fn main() {
    let scenarios = [
        Scenario {
            name: "TruePeak process_sample / 48k (4×)",
            path: Path::TruePeak,
            sample_rate: 48_000.0,
        },
        Scenario {
            name: "TruePeak process_sample / 96k (2×)",
            path: Path::TruePeak,
            sample_rate: 96_000.0,
        },
        Scenario {
            name: "TruePeak process_sample / 192k (bypass)",
            path: Path::TruePeak,
            sample_rate: 192_000.0,
        },
        Scenario {
            name: "TruePeak process_sample_peak / 48k (4×)",
            path: Path::TruePeakPeak,
            sample_rate: 48_000.0,
        },
        Scenario {
            name: "SPSC push / 48k",
            path: Path::SpscPush,
            sample_rate: 48_000.0,
        },
    ];

    let results: Vec<RunResult> = scenarios.iter().map(|s| run(*s)).collect();
    time_snapshot();

    eprintln!("─────── summary ───────");
    eprintln!("{:<42}  {:>9}  {:>9}", "scenario", "RT-factor", "cpu/inst");
    for r in &results {
        eprintln!(
            "{:<42}  {:>8.1}×  {:>8.3}%",
            r.scenario.name, r.rt_factor, r.cpu_pct_per_instance
        );
    }
}
