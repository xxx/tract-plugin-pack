use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use pope_scope::ring_buffer::RingBuffer;

fn bench_ring_buffer_push(c: &mut Criterion) {
    // Representative: 512-sample block pushed into a 192000-sample ring. Exercises
    // the f32x16 mipmap downsample fast path in RingBuffer::push.
    let block: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();

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

fn bench_ring_buffer_read_range(c: &mut Criterion) {
    const CAPACITY: usize = 1_536_000;
    const WINDOW: usize = 96_000;
    let mut ring = RingBuffer::new(CAPACITY);
    ring.push(&vec![0.5; CAPACITY]);
    let mut out = vec![0.0; WINDOW];

    let mut group = c.benchmark_group("ring_buffer_read_range");
    group.throughput(Throughput::Elements(WINDOW as u64));
    for (name, start) in [
        ("contiguous", CAPACITY / 2),
        ("wrapped", CAPACITY - WINDOW / 2),
    ] {
        group.bench_function(name, |b| {
            b.iter(|| {
                ring.read_range(black_box(start), &mut out);
                black_box(&out);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_ring_buffer_push,
    bench_ring_buffer_read_range
);
criterion_main!(benches);
