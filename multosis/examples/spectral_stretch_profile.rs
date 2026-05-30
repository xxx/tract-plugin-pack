//! Profiling harness for `SpectralStretch`.
//!
//! Drives the effect's per-sample hot path in a tight loop -- no plugin or
//! GUI plumbing. The bench reports a real-time factor (audio seconds
//! processed per wall-clock second) so callers can see whether an
//! optimisation moves the needle on the actual workload.
//!
//! Build (target-cpu auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example spectral_stretch_profile -p multosis
//!
//! Profile with perf:
//!     perf record -F 999 -g --call-graph dwarf -- target/profiling/examples/spectral_stretch_profile
//!     perf report --no-children -g graph,0.5

use std::hint::black_box;
use std::time::Instant;

use multosis::effects::{default_params_for_kind, Effect, EffectInstance, EffectKind};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 512;
const SECONDS: f32 = 5.0;

fn run_one(label: &str, fft_param: f32, speed: f32, tempo_pct: f32, chaos_pct: f32) {
    let mut effect = EffectInstance::new(EffectKind::SpectralStretch);
    effect.set_sample_rate(SAMPLE_RATE);
    let defaults = default_params_for_kind(EffectKind::SpectralStretch);
    let n_params = effect.parameters().len();
    for (i, &v) in defaults.iter().enumerate().take(n_params) {
        effect.set_param(i, v);
    }
    // Override to the scenario's params: Speed (0), Tempo (1), Chaos (2), FFT (3).
    effect.set_param(0, speed);
    effect.set_param(1, tempo_pct);
    effect.set_param(2, chaos_pct);
    effect.set_param(3, fft_param);

    // 440 Hz sine -- representative tonal input.
    let input: Vec<f32> = (0..BLOCK)
        .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / SAMPLE_RATE).sin() * 0.5)
        .collect();

    let target_samples = (SECONDS * SAMPLE_RATE) as usize;
    let n_blocks = target_samples / BLOCK;
    let real_samples = n_blocks * BLOCK;

    let start = Instant::now();
    let mut acc = 0.0_f32;
    for _ in 0..n_blocks {
        for &s in &input {
            let (l, r) = effect.process_sample(black_box(s), black_box(s));
            acc += l + r;
        }
    }
    black_box(acc);
    let elapsed = start.elapsed();

    let audio_secs = real_samples as f32 / SAMPLE_RATE;
    let wall_secs = elapsed.as_secs_f32();
    let rtf = audio_secs / wall_secs;
    let per_sample_ns = elapsed.as_nanos() as f64 / real_samples as f64;
    let core_pct = 100.0 / rtf;
    println!("{label:<40} rtf={rtf:>7.1}x  {per_sample_ns:>7.1} ns/sample  ~{core_pct:.2}% core",);
}

fn main() {
    println!("SpectralStretch profile -- {SECONDS}s of audio per scenario at {SAMPLE_RATE} Hz");
    println!();
    // Defaults: FFT=2048 (param=2.0), Speed=1.0, Tempo=100%, Chaos=0%.
    run_one("default (2048, speed=1, tempo=100)", 2.0, 1.0, 100.0, 0.0);
    run_one("speed=0.5 (downshift)", 2.0, 0.5, 100.0, 0.0);
    run_one("speed=2.0 (upshift)", 2.0, 2.0, 100.0, 0.0);
    run_one("chaos=50% adds rng + extra trig", 2.0, 1.0, 100.0, 50.0);
    run_one("tempo=10% throttles analyze", 2.0, 1.0, 10.0, 0.0);
    run_one("FFT=512 (smallest)", 0.0, 1.0, 100.0, 0.0);
    run_one("FFT=1024", 1.0, 1.0, 100.0, 0.0);
    run_one("FFT=4096 (largest)", 3.0, 1.0, 100.0, 0.0);
}
