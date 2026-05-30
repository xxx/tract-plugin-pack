use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use nap::engine::ReverbChannel;
use nap::sequence::{
    default_decay_curve, default_tone_curve, default_width_curve, generate, GenParams,
    VelvetSequence,
};

/// One 512-sample block of audio at 48 kHz is the real-time budget yardstick:
/// 512 / 48000 ≈ 10.67 ms. Each bench processes exactly that many samples
/// through a single channel.
const BLK: usize = 512;

fn bench_engine(c: &mut Criterion) {
    let sr = 48_000.0;
    let mut group = c.benchmark_group("nap/engine");

    // Impulse input — exercises the convolution; the value pattern is irrelevant
    // to cost (every pulse tap is read regardless).
    let mut input = vec![0.0f32; BLK];
    input[0] = 1.0;

    // Includes the worst case (10 s × 4000/s = 40_000 pulses) where the
    // O(pulse-count) convolution cost ceiling actually bites.
    for &(size, density) in &[
        (1.0f32, 1000.0f32),
        (2.0, 1500.0),
        (4.0, 3000.0),
        (10.0, 4000.0),
    ] {
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

        // Production path: block + SIMD convolution.
        group.bench_function(format!("blk/{id}"), |b| {
            b.iter_batched(
                || (ReverbChannel::new(sr), vec![0.0f32; BLK]),
                |(mut ch, mut out)| {
                    ch.process_block(&input, &mut out, &seq, &seq.location);
                    std::hint::black_box(&out);
                },
                BatchSize::SmallInput,
            )
        });

        // Reference path: the old per-sample gather, kept for comparison.
        group.bench_function(format!("seq/{id}"), |b| {
            b.iter_batched(
                || ReverbChannel::new(sr),
                |mut ch| {
                    for &x in input.iter() {
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
