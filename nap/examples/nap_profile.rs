//! Profiling harness for nap's reverb engine (the block + SIMD convolution).
//!
//! Drives the production `process_block` path for both channels with no plugin
//! or GUI plumbing, and reports a real-time factor + per-sample ns + stereo
//! %-of-a-core so optimisation passes stay comparable.
//!
//! Build:
//!     cargo xtask native build --profile profiling --example nap_profile -p nap
//!
//! Profile with perf (line-level attribution of the hot loop):
//!     perf record -F 1999 -g --call-graph dwarf -- \
//!         target/profiling/examples/nap_profile
//!     perf report --no-children -g graph,0.5

use std::hint::black_box;
use std::time::Instant;

use nap::engine::{ReverbChannel, BLOCK};
use nap::sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

const SAMPLE_RATE: f32 = 48_000.0;
const SECONDS: f32 = 5.0;

fn run_one(label: &str, size_s: f32, density: f32) {
    let p = GenParams {
        sample_rate: SAMPLE_RATE,
        size_s,
        density,
        width_ms: 8.0,
        seed: 1,
    };
    let mut seq = VelvetSequence::new();
    generate(
        &mut seq,
        &p,
        &default_decay_curve(),
        &default_width_curve(),
        &default_tone_curve(),
    );

    let mut left = ReverbChannel::new(SAMPLE_RATE);
    let mut right = ReverbChannel::new(SAMPLE_RATE);

    // 440 Hz tone block — representative tonal excitation.
    let input: Vec<f32> = (0..BLOCK)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE).sin() * 0.5)
        .collect();
    let mut wet_l = vec![0.0f32; BLOCK];
    let mut wet_r = vec![0.0f32; BLOCK];

    let target_samples = (SECONDS * SAMPLE_RATE) as usize;
    let n_blocks = target_samples / BLOCK;
    let real_samples = n_blocks * BLOCK;

    let start = Instant::now();
    let mut acc = 0.0f32;
    for _ in 0..n_blocks {
        left.process_block(black_box(&input), &mut wet_l, &seq, &seq.location);
        right.process_block(black_box(&input), &mut wet_r, &seq, &seq.location_r);
        acc += wet_l[0] + wet_r[0];
    }
    black_box(acc);
    let elapsed = start.elapsed();

    // Stereo: both channels processed per block above.
    let audio_secs = real_samples as f32 / SAMPLE_RATE;
    let wall_secs = elapsed.as_secs_f32();
    let rtf = audio_secs / wall_secs;
    let per_sample_ns = elapsed.as_nanos() as f64 / real_samples as f64;
    let core_pct = 100.0 / rtf;
    println!(
        "{label:<26} {:>6} pulses  rtf={rtf:>7.1}x  {per_sample_ns:>6.1} ns/stereo-sample  ~{core_pct:.2}% core",
        seq.count
    );
}

fn main() {
    println!(
        "nap engine profile — stereo process_block, {SECONDS}s of audio at {SAMPLE_RATE} Hz\n"
    );
    run_one("default (1.5s x 1500)", 1.5, 1500.0);
    run_one("medium (2s x 1500)", 2.0, 1500.0);
    run_one("large (4s x 3000)", 4.0, 3000.0);
    run_one("max (10s x 4000)", 10.0, 4000.0);
}
