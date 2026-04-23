use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use pope_scope::ring_buffer::RingBuffer;

fn bench_ring_buffer_push(c: &mut Criterion) {
    // Representative: 512-sample block pushed into a 192000-sample ring. Exercises
    // the f32x16 mipmap downsample fast path in RingBuffer::push.
    let block: Vec<f32> = (0..512)
        .map(|i| (i as f32 * 0.01).sin() * 0.5)
        .collect();

    let mut group = c.benchmark_group("ring_buffer_push");
    group.throughput(Throughput::Elements(block.len() as u64));
    group.bench_function("512samp_into_192k", |b| {
        let mut rb = RingBuffer::new(192_000);
        b.iter(|| {
            rb.push(black_box(&block));
        });
    });
    group.finish();
}

criterion_group!(benches, bench_ring_buffer_push);
criterion_main!(benches);
