//! Per-effect DSP bench. One bench is registered for every
//! [`multosis::effects::EffectKind`] variant via the `EffectKind::ALL`
//! registry — adding a new effect kind to the registry automatically
//! produces a new bench, no edits needed here.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use multosis::effects::{default_params_for_kind, Effect, EffectInstance, EffectKind};

/// Per-bench sample count. Matches the engine bench's block size so per-sample
/// numbers compare across files.
const N: usize = 512;
const SAMPLE_RATE: f32 = 48_000.0;

/// Hammers `process_sample` for every [`EffectKind`] variant in turn.
fn bench_effect_kinds(c: &mut Criterion) {
    // A representative 440 Hz sine input, computed once and reused so every
    // effect bench sees the identical signal. 0.5 amplitude leaves headroom
    // and avoids any clip behaviour on saturators we may add later.
    let input: Vec<f32> = (0..N)
        .map(|i| (i as f32 / SAMPLE_RATE * 440.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();

    let mut group = c.benchmark_group("effect");
    group.throughput(Throughput::Elements(N as u64));

    for &kind in &EffectKind::ALL {
        let mut effect = EffectInstance::new(kind);
        effect.set_sample_rate(SAMPLE_RATE);
        // Apply the kind's canonical defaults. `default_params_for_kind`
        // returns a fixed-size [f32; N]; skip entries past this effect's own
        // parameter count.
        let defaults = default_params_for_kind(kind);
        let param_count = effect.parameters().len();
        for (i, &v) in defaults.iter().enumerate().take(param_count) {
            effect.set_param(i, v);
        }

        let id = format!("process_sample_{:?}", kind);
        group.bench_function(&id, |b| {
            b.iter(|| {
                let mut acc = 0.0_f32;
                for &s in &input {
                    let (l, r) = effect.process_sample(black_box(s), black_box(s));
                    acc += l + r;
                }
                acc
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_effect_kinds);
criterion_main!(benches);
