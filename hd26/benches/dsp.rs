use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use hd26::dimension::{DimMode, DimParams, Dimension};
use hd26::hyper::{Hyper, HyperParams};
use hd26::transient::TransientDetector;

/// 512 samples @ 48 kHz ≈ 10.67 ms — the real-time budget yardstick.
const BLK: usize = 512;
const SR: f32 = 48_000.0;

fn input_block() -> Vec<f32> {
    (0..BLK).map(|n| (0.05 * n as f32).sin() * 0.5).collect()
}

fn bench_hyper(c: &mut Criterion) {
    let mut group = c.benchmark_group("hd26/hyper");
    let input = input_block();
    for voices in [1usize, 3, 7] {
        let p = HyperParams {
            voices,
            detune: 0.6,
            rate_hz: 1.0,
            width: 0.5,
            mix: 1.0,
        };
        group.bench_function(format!("voices_{voices}"), |b| {
            b.iter_batched(
                || Hyper::new(SR),
                |mut h| {
                    for &x in &input {
                        std::hint::black_box(h.process_sample(x, x, &p));
                    }
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn bench_dimension(c: &mut Criterion) {
    let mut group = c.benchmark_group("hd26/dimension");
    let input = input_block();
    for (label, mode) in [("am", DimMode::Am), ("pitch", DimMode::Pitch)] {
        let p = DimParams {
            size: 0.5,
            mode,
            hpf_hz: 120.0,
            mix: 0.7,
        };
        group.bench_function(label, |b| {
            b.iter_batched(
                || Dimension::new(SR),
                |mut d| {
                    for &x in &input {
                        std::hint::black_box(d.process_sample(x, x * 0.8, &p));
                    }
                },
                BatchSize::SmallInput,
            )
        });
    }
    group.finish();
}

fn bench_transient(c: &mut Criterion) {
    let mut group = c.benchmark_group("hd26/transient");
    let input = input_block();
    group.bench_function("detect", |b| {
        b.iter_batched(
            || TransientDetector::new(SR),
            |mut t| {
                for &x in &input {
                    std::hint::black_box(t.process_sample(x));
                }
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_full_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("hd26/full");
    let input = input_block();
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
    group.bench_function("hyper+dimension_7v", |b| {
        b.iter_batched(
            || (Hyper::new(SR), Dimension::new(SR)),
            |(mut h, mut d)| {
                for &x in &input {
                    let (hl, hr) = h.process_sample(x, x, &hp);
                    std::hint::black_box(d.process_sample(hl, hr, &dp));
                }
            },
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_hyper,
    bench_dimension,
    bench_transient,
    bench_full_chain
);
criterion_main!(benches);
