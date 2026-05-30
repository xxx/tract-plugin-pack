//! Profiling harness for the `Diode` ladder effect.
//!
//! Build:
//!     cargo xtask native build --profile profiling --example diode_profile -p multosis
//!
//! Profile:
//!     perf record -F 999 -g --call-graph dwarf -- target/profiling/examples/diode_profile
//!     perf report --no-children -g graph,0.5

use std::hint::black_box;
use std::time::Instant;

use multosis::effects::{default_params_for_kind, Effect, EffectInstance, EffectKind};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 512;
const SECONDS: f32 = 5.0;

fn run_one(label: &str, cutoff: f32, resonance: f32, drive_db: f32) {
    let mut effect = EffectInstance::new(EffectKind::Diode);
    effect.set_sample_rate(SAMPLE_RATE);
    let defaults = default_params_for_kind(EffectKind::Diode);
    let n_params = effect.parameters().len();
    for (i, &v) in defaults.iter().enumerate().take(n_params) {
        effect.set_param(i, v);
    }
    effect.set_param(0, cutoff);
    effect.set_param(1, resonance);
    effect.set_param(2, drive_db);

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
    println!("Diode profile -- {SECONDS}s of audio per scenario at {SAMPLE_RATE} Hz");
    println!();
    run_one("default (800Hz, res=0.5, drive=0dB)", 800.0, 0.5, 0.0);
    run_one("low cutoff (100Hz)", 100.0, 0.5, 0.0);
    run_one("high cutoff (8kHz)", 8_000.0, 0.5, 0.0);
    run_one("high resonance (res=0.95)", 800.0, 0.95, 0.0);
    run_one("max drive (24 dB)", 800.0, 0.5, 24.0);
    run_one("self-osc + max drive", 800.0, 1.0, 24.0);
}
