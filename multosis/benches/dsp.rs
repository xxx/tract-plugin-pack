use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use multosis::effects::{default_params_for_kind, EffectKind, TrackEffect};
use multosis::engine::AudioEngine;
use multosis::grid::Grid;
use multosis::modulation::TrackModulation;

fn bench_process_full_grid_mixed_effects(c: &mut Criterion) {
    // Worst-case-ish baseline: every cell enabled (every row active each
    // step), each row running a real effect with continuous modulation.
    // 512-sample blocks at 48 kHz — a typical DAW buffer.
    let mut engine = AudioEngine::new();
    engine.set_sample_rate(48_000.0);

    // Half Lowpass, half Bitcrush — covers two distinct DSP shapes so the
    // bench doesn't accidentally measure only one effect kind.
    let mut effects: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
    for (i, te) in effects.iter_mut().enumerate() {
        te.kind = if i % 2 == 0 {
            EffectKind::Lowpass
        } else {
            EffectKind::Bitcrush
        };
        te.params = default_params_for_kind(te.kind);
        te.mix = 1.0;
    }
    engine.set_effects(&effects);

    // Default modulation: every row has a cyclic triangle on assignable
    // MSEG 1, sweeping the effect's parameter 0 each block.
    let modulation: [TrackModulation; 16] = std::array::from_fn(TrackModulation::default_for_row);
    engine.set_modulation(&modulation);

    let grid = Grid::default();

    let mut left = vec![0.3_f32; 512];
    let mut right = vec![0.3_f32; 512];

    let mut group = c.benchmark_group("audio_engine");
    group.throughput(Throughput::Elements(left.len() as u64));
    group.bench_function("process_512samp_16rows_mixed", |b| {
        b.iter(|| {
            // Refill the buffers each iter so the input is the same every time
            // (effects with feedback would otherwise diverge as iterations
            // mutated the buffer in place).
            for x in left.iter_mut() {
                *x = 0.3;
            }
            for x in right.iter_mut() {
                *x = 0.3;
            }
            engine.process(
                black_box(&mut left),
                black_box(&mut right),
                black_box(true),
                black_box(1000.0),
                black_box(120.0),
                black_box(1.0),
                black_box(&grid),
            );
        });
    });
    group.finish();
}

criterion_group!(benches, bench_process_full_grid_mixed_effects);
criterion_main!(benches);
