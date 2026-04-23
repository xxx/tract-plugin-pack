use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use tiny_skia::{Color, Pixmap};
use tiny_skia_widgets::{draw_rect, fill_column_opaque, fill_pixmap_opaque};

const W: u32 = 800;
const H: u32 = 600;

fn bench_draw_rect_opaque(c: &mut Criterion) {
    // Opaque draw_rect should take tiny-skia-widgets' Source-blend fast path.
    let mut group = c.benchmark_group("draw_rect_opaque");
    group.throughput(Throughput::Bytes((400 * 300 * 4) as u64));
    group.bench_function("400x300_into_800x600", |b| {
        let mut pixmap = Pixmap::new(W, H).unwrap();
        let color = Color::from_rgba(0.4, 0.6, 0.9, 1.0).unwrap();
        b.iter(|| {
            draw_rect(
                black_box(&mut pixmap),
                black_box(100.0),
                black_box(100.0),
                black_box(400.0),
                black_box(300.0),
                black_box(color),
            );
        });
    });
    group.finish();
}

fn bench_draw_rect_translucent(c: &mut Criterion) {
    // Translucent draw_rect goes through tiny-skia's full raster+blend pipeline,
    // which is where tiny-skia's AVX2-gated paths actually kick in.
    let mut group = c.benchmark_group("draw_rect_translucent");
    group.throughput(Throughput::Bytes((400 * 300 * 4) as u64));
    group.bench_function("400x300_alpha_half", |b| {
        let mut pixmap = Pixmap::new(W, H).unwrap();
        let color = Color::from_rgba(0.4, 0.6, 0.9, 0.5).unwrap();
        b.iter(|| {
            draw_rect(
                black_box(&mut pixmap),
                black_box(100.0),
                black_box(100.0),
                black_box(400.0),
                black_box(300.0),
                black_box(color),
            );
        });
    });
    group.finish();
}

fn bench_fill_pixmap_opaque(c: &mut Criterion) {
    // fill_pixmap_opaque uses chunks_exact_mut + slice fill (direct pixel writes,
    // no tiny-skia raster pipeline). Should be largely invariant to target-cpu.
    let mut group = c.benchmark_group("fill_pixmap_opaque");
    group.throughput(Throughput::Bytes((W * H * 4) as u64));
    group.bench_function("800x600_clear", |b| {
        let mut pixmap = Pixmap::new(W, H).unwrap();
        let color = Color::from_rgba(0.05, 0.05, 0.1, 1.0).unwrap();
        b.iter(|| {
            fill_pixmap_opaque(black_box(&mut pixmap), black_box(color));
        });
    });
    group.finish();
}

fn bench_fill_column_opaque(c: &mut Criterion) {
    // pope-scope's waveform fast path: 800 direct-pixel-write columns per frame.
    // Small per-column work but called many times per frame.
    let mut group = c.benchmark_group("fill_column_opaque");
    group.throughput(Throughput::Elements(W as u64));
    group.bench_function("800_columns_200h", |b| {
        let mut pixmap = Pixmap::new(W, H).unwrap();
        let color = Color::from_rgba(1.0, 0.7, 0.2, 1.0).unwrap();
        b.iter(|| {
            for col in 0..W {
                fill_column_opaque(
                    black_box(&mut pixmap),
                    black_box(col as f32),
                    black_box(200.0),
                    black_box(400.0),
                    black_box(color),
                );
            }
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_draw_rect_opaque,
    bench_draw_rect_translucent,
    bench_fill_pixmap_opaque,
    bench_fill_column_opaque
);
criterion_main!(benches);
