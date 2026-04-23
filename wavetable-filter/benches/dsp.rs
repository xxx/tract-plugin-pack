use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use wavetable_filter::wavetable::Wavetable;

fn bench_wavetable_interp(c: &mut Criterion) {
    // 2048-sample frames, 256 frames. interpolate_frame_into does a linear blend
    // between two adjacent frames -- prime auto-vectorization territory.
    let frame_size = 2048;
    let num_frames = 256;
    let samples: Vec<f32> = (0..frame_size * num_frames)
        .map(|i| (i as f32 * 0.001).sin())
        .collect();
    let wt = Wavetable::new(samples, frame_size).expect("wavetable constructed");

    let mut group = c.benchmark_group("wavetable_interp");
    group.throughput(Throughput::Elements(frame_size as u64));
    group.bench_function("2048_frame_mid_position", |b| {
        let mut out = vec![0.0; frame_size];
        b.iter(|| {
            wt.interpolate_frame_into(black_box(128.37), black_box(&mut out));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_wavetable_interp);
criterion_main!(benches);
