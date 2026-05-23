use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use multosis::effects::{default_params_for_kind, EffectKind, TrackEffect};
use multosis::engine::AudioEngine;
use multosis::grid::Grid;
use multosis::modulation::TrackModulation;

/// Build an engine with the default modulation on every row and `kind`
/// installed on every row (or `None` for the all-silence baseline).
fn engine_uniform(kind: Option<EffectKind>) -> AudioEngine {
    let mut engine = AudioEngine::new();
    engine.set_sample_rate(48_000.0);
    let mut effects: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
    if let Some(k) = kind {
        for te in effects.iter_mut() {
            te.kind = k;
            te.params = default_params_for_kind(k);
            te.mix = 1.0;
        }
    }
    engine.set_effects(&effects);
    let modulation: [TrackModulation; 16] = std::array::from_fn(TrackModulation::default_for_row);
    engine.set_modulation(&modulation);
    engine
}

/// Engine with half Lowpass / half Bitcrush — the "loaded" workload that
/// covers two distinct DSP shapes in one bench.
fn engine_mixed() -> AudioEngine {
    let mut engine = AudioEngine::new();
    engine.set_sample_rate(48_000.0);
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
    let modulation: [TrackModulation; 16] = std::array::from_fn(TrackModulation::default_for_row);
    engine.set_modulation(&modulation);
    engine
}

/// Engine with every row running FM — the heaviest per-effect workload.
/// Even-numbered rows are Carrier mode (delay-line vibrato), odd rows are
/// Modulator mode (per-channel sine carrier modulated by the input). The
/// series chain runs all 16 stages on every sample, exercising both FM
/// code paths plus the delay-line + carrier-phase stacking.
fn engine_fm_loaded() -> AudioEngine {
    let mut engine = AudioEngine::new();
    engine.set_sample_rate(48_000.0);
    let mut effects: [TrackEffect; 16] = std::array::from_fn(TrackEffect::default_for_row);
    for (i, te) in effects.iter_mut().enumerate() {
        te.kind = EffectKind::Fm;
        te.params = default_params_for_kind(EffectKind::Fm);
        // Mode is at param index 3 (0 = Carrier, 1 = Modulator).
        te.params[3] = if i % 2 == 0 { 0.0 } else { 1.0 };
        te.mix = 1.0;
    }
    engine.set_effects(&effects);
    let modulation: [TrackModulation; 16] = std::array::from_fn(TrackModulation::default_for_row);
    engine.set_modulation(&modulation);
    engine
}

/// One `process` iteration: refill the buffers (so feedback/filtering
/// doesn't accumulate across iterations) then call `process`.
fn run_process(
    engine: &mut AudioEngine,
    left: &mut [f32],
    right: &mut [f32],
    playing: bool,
    samples_per_step: f64,
    grid: &Grid,
) {
    left.fill(0.3);
    right.fill(0.3);
    engine.process(
        black_box(left),
        black_box(right),
        black_box(playing),
        black_box(samples_per_step),
        black_box(120.0),
        black_box(1.0),
        black_box(grid),
    );
}

fn bench_process(c: &mut Criterion) {
    let grid = Grid::default();
    let mut group = c.benchmark_group("audio_engine");

    // Headline workload: every row active, half Lowpass / half Bitcrush,
    // modulation running, transport playing, 512-sample block.
    {
        let mut engine = engine_mixed();
        let mut left = vec![0.3_f32; 512];
        let mut right = vec![0.3_f32; 512];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_512samp_mixed", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 1000.0, &grid));
        });
    }

    // Structural baseline: every row's effect is `None` (silence). The
    // delta from `_mixed` is the actual Lowpass+Bitcrush DSP cost; what
    // remains is the per-row infrastructure (active loop, modulation,
    // compressor, dry/wet mix).
    {
        let mut engine = engine_uniform(None);
        let mut left = vec![0.3_f32; 512];
        let mut right = vec![0.3_f32; 512];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_512samp_silence", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 1000.0, &grid));
        });
    }

    // Idle: loaded with effects but transport stopped — measures the
    // "plugin sitting in the chain doing nothing visible" cost.
    {
        let mut engine = engine_mixed();
        let mut left = vec![0.3_f32; 512];
        let mut right = vec![0.3_f32; 512];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_512samp_idle", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, false, 1000.0, &grid));
        });
    }

    // Many step boundaries per block: small `samples_per_step` → ~50 ticks
    // in a 512-sample block, exercising the per-segment modulation update
    // and the cell-light fire path.
    {
        let mut engine = engine_mixed();
        let mut left = vec![0.3_f32; 512];
        let mut right = vec![0.3_f32; 512];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_512samp_many_boundaries", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 10.0, &grid));
        });
    }

    // Small buffer — low-latency hosts. Per-block overhead amortizes over
    // fewer samples so the per-sample cost is highest here.
    {
        let mut engine = engine_mixed();
        let mut left = vec![0.3_f32; 64];
        let mut right = vec![0.3_f32; 64];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_64samp_mixed", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 1000.0, &grid));
        });
    }

    // Large buffer — common modern DAW setting.
    {
        let mut engine = engine_mixed();
        let mut left = vec![0.3_f32; 1024];
        let mut right = vec![0.3_f32; 1024];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_1024samp_mixed", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 1000.0, &grid));
        });
    }

    // FM-heavy workload: every row runs FM, half Carrier mode + half
    // Modulator mode. FM is the heaviest per-sample effect (~4× Lowpass);
    // chaining 16 of them in series stresses the worst case the engine
    // can produce today.
    {
        let mut engine = engine_fm_loaded();
        let mut left = vec![0.3_f32; 512];
        let mut right = vec![0.3_f32; 512];
        group.throughput(Throughput::Elements(left.len() as u64));
        group.bench_function("process_512samp_fm", |b| {
            b.iter(|| run_process(&mut engine, &mut left, &mut right, true, 1000.0, &grid));
        });
    }

    group.finish();
}

criterion_group!(benches, bench_process);
criterion_main!(benches);
