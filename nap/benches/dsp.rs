use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use nap::engine::ReverbChannel;
use nap::sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

fn bench_engine(c: &mut Criterion) {
    let sr = 48_000.0;
    let mut group = c.benchmark_group("nap/engine");
    for &(size, density) in &[(1.0f32, 1000.0f32), (2.0, 1500.0), (4.0, 3000.0)] {
        let p = GenParams {
            sample_rate: sr,
            size_s: size,
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
        let id = format!("size{size}s_density{density}");
        group.bench_function(&id, |b| {
            b.iter_batched(
                || ReverbChannel::new(sr),
                |mut ch| {
                    for n in 0..512 {
                        let x = if n == 0 { 1.0 } else { 0.0 };
                        std::hint::black_box(ch.process(x, &seq, &seq.location));
                    }
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

criterion_group!(benches, bench_engine);
criterion_main!(benches);
