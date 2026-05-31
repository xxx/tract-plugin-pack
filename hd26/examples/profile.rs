//! Profiling harness for the HD26 DSP chain. Runs a long steady-state pass of
//! the full Hyper -> Dimension chain (plus the transient detector) so a
//! sampling profiler (perf / cargo-flamegraph) can attribute per-function cost.
//!
//! Input is a pre-baked block cycled with a power-of-two mask, so no
//! transcendental from input generation pollutes the hot loop.
//!
//! Build + run (target-cpu native):
//!   RUSTFLAGS="-C target-cpu=native" cargo run --release -p hd26 --example profile
//! Profile:
//!   RUSTFLAGS="-C target-cpu=native" cargo build --release -p hd26 --example profile
//!   perf record -g --call-graph dwarf -- ./target/release/examples/profile
//!   perf report --stdio | head -60

use hd26::dimension::{DimMode, DimParams, Dimension};
use hd26::hyper::{Hyper, HyperParams};
use hd26::transient::TransientDetector;
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 512; // power of two for masked indexing

fn main() {
    let mut hyper = Hyper::new(SR);
    let mut dim = Dimension::new(SR);
    let mut trans = TransientDetector::new(SR);
    trans.set_sensitivity(0.5);

    let hp = HyperParams {
        voices: 7,
        detune: 0.6,
        rate_hz: 1.0,
        width: 0.5,
        mix: 1.0,
    };
    let dp = DimParams {
        size: 0.5,
        mode: DimMode::Am,
        hpf_hz: 120.0,
        mix: 0.7,
    };

    // Pre-baked input block (sin used here only, outside the timed loop).
    let block: Vec<f32> = (0..BLOCK)
        .map(|i| (std::f32::consts::TAU * 220.0 / SR * i as f32).sin() * 0.5)
        .collect();

    // 240 s of audio at 48 kHz -> ~11.5M samples; plenty for perf sampling.
    let total: usize = (SR as usize) * 240;

    let start = Instant::now();
    let mut acc = 0.0f32;
    for n in 0..total {
        let x = block[n & (BLOCK - 1)];
        let _ = trans.process_sample(x);
        let (hl, hr) = hyper.process_sample(x, x, &hp);
        let (dl, dr) = dim.process_sample(hl, hr, &dp);
        acc += dl + dr;
    }
    let elapsed = start.elapsed();

    let ns_per_sample = elapsed.as_nanos() as f64 / total as f64;
    let audio_seconds = total as f64 / SR as f64;
    let realtime_factor = audio_seconds / elapsed.as_secs_f64();
    // Per 512-sample block at 48 kHz the budget is 512/48000 = 10.667 ms.
    let us_per_block = ns_per_sample * BLOCK as f64 / 1000.0;
    println!("HD26 full chain, 7 voices, AM dimension:");
    println!("  samples processed : {total}");
    println!("  wall time         : {:.3} s", elapsed.as_secs_f64());
    println!("  ns / sample       : {ns_per_sample:.2}");
    println!("  us / 512-block    : {us_per_block:.2}  (budget 10667 us)");
    println!("  realtime factor   : {realtime_factor:.1}x");
    println!("  (acc sink         : {acc})");
}
