use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use warp_zone::spectral::SpectralShifter;

fn bench_spectral_shifter(c: &mut Criterion) {
    // Full phase-vocoder step across a 512-sample block. Rustfft has its own SIMD
    // dispatch so most visible gain comes from the surrounding bin-remap + phase loops.
    let mut shifter = SpectralShifter::new(4096, 1024);
    let block: Vec<f32> = (0..512).map(|i| (i as f32 * 0.02).sin() * 0.7).collect();
    let fft_size = 4096;
    let low_bin = 0usize;
    let high_bin = fft_size / 2;

    let mut group = c.benchmark_group("spectral_shifter");
    group.throughput(Throughput::Elements(block.len() as u64));
    group.bench_function("512samp_shift_up_octave", |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for &s in block.iter() {
                acc += shifter.process_sample(
                    black_box(s),
                    black_box(12.0),
                    black_box(1.0),
                    false,
                    low_bin,
                    high_bin,
                );
            }
            acc
        });
    });
    group.finish();
}

criterion_group!(benches, bench_spectral_shifter);
criterion_main!(benches);
