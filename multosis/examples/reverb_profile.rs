//! Profiling harness for the Schroeder-Moorer `Reverb`.
//!
//! Mirrors `spectral_stretch_profile`: drives the per-sample hot path with
//! no plugin or GUI plumbing, reports a real-time factor and per-sample ns
//! so the same scenarios stay comparable across optimisation passes.
//!
//! Build:
//!     cargo xtask native build --profile profiling --example reverb_profile -p multosis
//!
//! Profile with perf:
//!     perf record -F 999 -g --call-graph dwarf -- target/profiling/examples/reverb_profile
//!     perf report --no-children -g graph,0.5

use std::hint::black_box;
use std::time::Instant;

use multosis::effects::{default_params_for_kind, Effect, EffectInstance, EffectKind};

const SAMPLE_RATE: f32 = 48_000.0;
const BLOCK: usize = 512;
const SECONDS: f32 = 5.0;

fn run_one(label: &str, decay: f32, damping: f32, mod_pct: f32, pre_ms: f32, width: f32) {
    let mut effect = EffectInstance::new(EffectKind::Reverb);
    effect.set_sample_rate(SAMPLE_RATE);
    let defaults = default_params_for_kind(EffectKind::Reverb);
    let n_params = effect.parameters().len();
    for (i, &v) in defaults.iter().enumerate().take(n_params) {
        effect.set_param(i, v);
    }
    // Order: Decay (0), Damping (1), Mod (2), Pre-delay (3), Width (4).
    effect.set_param(0, decay);
    effect.set_param(1, damping);
    effect.set_param(2, mod_pct);
    effect.set_param(3, pre_ms);
    effect.set_param(4, width);

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
    println!(
        "{label:<40} rtf={rtf:>7.1}x  {per_sample_ns:>7.1} ns/sample  ~{core_pct:.2}% core",
    );
}

fn main() {
    println!("Reverb profile -- {SECONDS}s of audio per scenario at {SAMPLE_RATE} Hz");
    println!();
    // Defaults: Decay=50, Damping=30, Mod=20, Pre=0, Width=100.
    run_one("default (decay=50, mod=20)", 50.0, 30.0, 20.0, 0.0, 100.0);
    run_one("mod=0 (no LFO sines)", 50.0, 30.0, 0.0, 0.0, 100.0);
    run_one("mod=100 (LFOs at full)", 50.0, 30.0, 100.0, 0.0, 100.0);
    run_one("long tail (decay=95)", 95.0, 30.0, 20.0, 0.0, 100.0);
    run_one("max damping", 50.0, 100.0, 20.0, 0.0, 100.0);
    run_one("pre-delay 50ms", 50.0, 30.0, 20.0, 50.0, 100.0);
    run_one("mono (width=0)", 50.0, 30.0, 20.0, 0.0, 0.0);
}
