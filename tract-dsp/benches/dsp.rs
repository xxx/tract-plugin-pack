//! Criterion micro-benchmarks for the shared `tract-dsp` primitives.
//!
//! These complement `examples/tract_dsp_profile.rs`: the profiling harness
//! reports whole-path real-time factors for a perf/flamegraph workflow, while
//! these give per-call/per-block wall-clock numbers with statistical bounds
//! for regression tracking. Every primitive that runs on a per-sample audio
//! hot path is covered here, because this crate has to be fast in all ways.
//!
//! The default-feature build benches only the dependency-free primitives
//! (boxcar, db, fast_math, fir, hilbert, window, spsc, true_peak). The
//! FFT-backed paths are feature-gated to match the library:
//!
//!     cargo xtask native bench -p tract-dsp --bench dsp \
//!         --features stft,spectral-engine
//!
//! (`spectral-engine` pulls in `stft-analysis`; add `stft` for the realfft
//! `StftConvolver`.) Without those features the gated groups simply don't run.

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

/// Per-sample benches process one block at a time so Criterion's throughput
/// figures read in samples/s — the unit that matters for an audio hot path.
const BLOCK: usize = 512;

/// One block of a 200 Hz sine at 48 kHz, ~0.7 peak. Real, varied,
/// non-constant-foldable input for the per-sample loops.
fn sine_block() -> Vec<f32> {
    (0..BLOCK)
        .map(|i| (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 48_000.0).sin() * 0.7)
        .collect()
}

// ────────────────────────── boxcar (RunningSumWindow) ──────────────────────

fn bench_boxcar(c: &mut Criterion) {
    use tract_dsp::boxcar::RunningSumWindow;

    let block = sine_block();
    let mut group = c.benchmark_group("boxcar");
    group.throughput(Throughput::Elements(BLOCK as u64));
    // The O(1) running-sum push gs-meter's momentary-RMS window pays per sample.
    group.bench_function("push_f64_window4096", |b| {
        let mut w = RunningSumWindow::<f64>::new(4096, 4096);
        b.iter(|| {
            for &s in &block {
                w.push(black_box(s as f64));
            }
            black_box(w.sum())
        });
    });
    group.finish();
}

// ──────────────────────────────── db ───────────────────────────────────────

fn bench_db(c: &mut Criterion) {
    use tract_dsp::db::{db_to_linear, db_to_linear_fast, linear_to_db};

    // A spread of dB values that won't constant-fold to one branch.
    let dbs: Vec<f32> = (0..BLOCK).map(|i| -60.0 + (i as f32) * 0.12).collect();
    let lins: Vec<f32> = (0..BLOCK).map(|i| 0.001 + (i as f32) * 0.002).collect();

    let mut group = c.benchmark_group("db");
    group.throughput(Throughput::Elements(BLOCK as u64));
    group.bench_function("db_to_linear_powf", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &d in &dbs {
                acc += db_to_linear(black_box(d));
            }
            acc
        });
    });
    // The exp()-based fast path is what the per-sample gain ramps call; this
    // bench is the evidence for "roughly twice as fast as powf()".
    group.bench_function("db_to_linear_fast_exp", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &d in &dbs {
                acc += db_to_linear_fast(black_box(d));
            }
            acc
        });
    });
    group.bench_function("linear_to_db", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &l in &lins {
                acc += linear_to_db(black_box(l));
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────────── fast_math (tanh) ─────────────────────────────

fn bench_fast_math(c: &mut Criterion) {
    use tract_dsp::fast_math::tanh_pade;

    // Span the full clamp range so neither branch is constant-folded.
    let xs: Vec<f32> = (0..BLOCK).map(|i| -5.0 + (i as f32) * (10.0 / BLOCK as f32)).collect();

    let mut group = c.benchmark_group("fast_math");
    group.throughput(Throughput::Elements(BLOCK as u64));
    group.bench_function("tanh_pade", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &x in &xs {
                acc += tanh_pade(black_box(x));
            }
            acc
        });
    });
    // Baseline for the speedup claim — libm tanhf over the same inputs.
    group.bench_function("tanh_libm", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &x in &xs {
                acc += black_box(x).tanh();
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────────── fir (FirRing) ────────────────────────────────

fn bench_fir(c: &mut Criterion) {
    use tract_dsp::fir::FirRing;

    let block = sine_block();
    let mut group = c.benchmark_group("fir");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // The miff / wavetable-filter Raw-mode hot path: push every sample, MAC
    // the most-recent window against a pre-reversed kernel. Two kernel lengths
    // bracket typical use; both are multiples of 16 (the f32x16 lane width).
    for &taps in &[64usize, 256] {
        let kernel: Vec<f32> = (0..taps)
            .map(|i| {
                // A windowed-sinc-ish kernel: real coefficients, sums to ~1.
                let t = i as f32 / taps as f32;
                (1.0 - (2.0 * t - 1.0).abs()) / (taps as f32 * 0.5)
            })
            .collect();
        let mut ring = FirRing::new(taps);
        group.bench_with_input(BenchmarkId::new("push_mac", taps), &taps, |b, _| {
            b.iter(|| {
                let mut acc = 0.0_f32;
                for &s in &block {
                    ring.push(black_box(s));
                    acc += ring.mac(&kernel);
                }
                acc
            });
        });
    }
    group.finish();
}

// ──────────────────────────── hilbert ──────────────────────────────────────

fn bench_hilbert(c: &mut Criterion) {
    use tract_dsp::hilbert::{AnalyticSignal, HilbertFir};

    let block = sine_block();
    let mut group = c.benchmark_group("hilbert");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // The 90° rotator imagine's decorrelator and multosis's FM path call.
    group.bench_function("HilbertFir_process_len65", |b| {
        let mut h = HilbertFir::new(65);
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                acc += h.process(black_box(s));
            }
            acc
        });
    });
    // Delay-matched (real, imag) pair — one rotation + one delay tap per sample.
    group.bench_function("AnalyticSignal_process_len65", |b| {
        let mut a = AnalyticSignal::new(65);
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                let (re, im) = a.process(black_box(s));
                acc += re + im;
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────────── window ───────────────────────────────────────

fn bench_window(c: &mut Criterion) {
    use tract_dsp::window::{hann_periodic, hann_symmetric};

    // One-shot construction cost (GUI/setup-thread, not per-sample) at a
    // typical analysis size.
    let mut group = c.benchmark_group("window");
    group.bench_function("hann_periodic_4096", |b| {
        b.iter(|| black_box(hann_periodic(black_box(4096))));
    });
    group.bench_function("hann_symmetric_4096", |b| {
        b.iter(|| black_box(hann_symmetric(black_box(4096))));
    });
    group.finish();
}

// ──────────────────────────── spsc ─────────────────────────────────────────

fn bench_spsc(c: &mut Criterion) {
    use tract_dsp::spsc;

    let block = sine_block();
    let mut group = c.benchmark_group("spsc");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // Producer-side per-sample push — imagine's vectorscope cost.
    group.bench_function("push", |b| {
        let (prod, _cons) = spsc::channel(65_536);
        b.iter(|| {
            for &s in &block {
                prod.push(black_box(s), black_box(-s));
            }
        });
    });
    group.finish();

    // Consumer-side snapshot is a GUI-thread (~60 Hz) call, not a per-sample
    // cost, so it gets its own group with no throughput annotation.
    let mut snap = c.benchmark_group("spsc_snapshot");
    let (prod, cons) = spsc::channel(65_536);
    for i in 0..65_536 {
        prod.push(i as f32, -(i as f32));
    }
    let mut a = vec![0.0_f32; 4096];
    let mut b_buf = vec![0.0_f32; 4096];
    snap.bench_function("snapshot_oldest_first_4096", |b| {
        b.iter(|| black_box(cons.snapshot_oldest_first(4096, &mut a, &mut b_buf)));
    });
    snap.finish();
}

// ──────────────────────────── true_peak ────────────────────────────────────

fn bench_true_peak(c: &mut Criterion) {
    use tract_dsp::true_peak::TruePeakDetector;

    let block = sine_block();
    let mut group = c.benchmark_group("true_peak");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // The single most expensive piece of shared DSP. Sample-rate determines
    // the oversampling factor: 4× <96 k, 2× 96–192 k, bypass ≥192 k.
    for &sr in &[48_000.0_f32, 96_000.0, 192_000.0] {
        let mut det = TruePeakDetector::new();
        det.set_sample_rate(sr);
        det.reset();
        group.bench_with_input(
            BenchmarkId::new("process_sample", sr as u32),
            &sr,
            |b, _| {
                b.iter(|| {
                    for &s in &block {
                        det.process_sample(black_box(s));
                    }
                    black_box(det.true_peak_max())
                });
            },
        );
    }

    // The variant tinylimit's ISP path calls (returns the per-sample peak).
    let mut det = TruePeakDetector::new();
    det.set_sample_rate(48_000.0);
    det.reset();
    group.bench_function("process_sample_peak_48k", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                acc += det.process_sample_peak(black_box(s));
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────── stft_analysis (gated) ────────────────────────────

#[cfg(feature = "stft-analysis")]
fn bench_stft_analysis(c: &mut Criterion) {
    use tract_dsp::stft_analysis::StftAnalyzer;

    let block = sine_block();
    let mut group = c.benchmark_group("stft_analysis");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // satch's 2048/512 and warp-zone's 4096/1024 analysis front-end: write
    // every sample, analyze once per hop. Forward-FFT cost both plugins pay.
    for &(fft_size, hop) in &[(2048usize, 512usize), (4096, 1024)] {
        let mut analyzer = StftAnalyzer::new(fft_size, hop);
        let mut hop_counter = 0usize;
        group.bench_with_input(
            BenchmarkId::new("write_analyze", format!("{fft_size}-{hop}")),
            &(fft_size, hop),
            |b, _| {
                b.iter(|| {
                    let mut acc = 0.0_f32;
                    for &s in &block {
                        analyzer.write(black_box(s));
                        hop_counter += 1;
                        if hop_counter >= hop {
                            hop_counter = 0;
                            acc += analyzer.analyze().spectrum[1].re;
                        }
                    }
                    acc
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────── spectral_clipper (gated) ─────────────────────────

#[cfg(feature = "stft-analysis")]
fn bench_spectral_clipper(c: &mut Criterion) {
    use tract_dsp::spectral_clipper::{saturate_td, saturate_td_with_tanh_fast, SpectralClipper};

    let xs: Vec<f32> = (0..BLOCK)
        .map(|i| (2.0 * std::f32::consts::PI * 200.0 * i as f32 / 48_000.0).sin() * 1.8)
        .collect();

    let mut group = c.benchmark_group("spectral_clipper");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // Scalar waveshaper — the time-domain clip applied to every sample.
    group.bench_function("saturate_td", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &x in &xs {
                acc += saturate_td(black_box(x), 2.0, 0.7, 0.5);
            }
            acc
        });
    });
    // The fast variant reusing a precomputed 1/threshold and returning tanh.
    group.bench_function("saturate_td_with_tanh_fast", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &x in &xs {
                acc += saturate_td_with_tanh_fast(black_box(x), 2.0, 0.7, 1.0 / 0.7, 0.5).0;
            }
            acc
        });
    });

    // Full spectral-clipper per-sample path (FFT + dual-ring OLA every hop).
    let block = sine_block();
    let mut clip = SpectralClipper::new(2048, 512);
    group.bench_function("process_sample_2048-512", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                acc += clip.process_sample(black_box(s), 2.0, 0.7, 0.5);
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────── spectral_shifter (gated) ─────────────────────────

#[cfg(feature = "stft-analysis")]
fn bench_spectral_shifter(c: &mut Criterion) {
    use tract_dsp::spectral_shifter::SpectralShifter;

    let block = sine_block();
    let mut group = c.benchmark_group("spectral_shifter");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // warp-zone's phase-vocoder step: shift up an octave, no stretch.
    let mut shifter = SpectralShifter::new(4096, 1024);
    group.bench_function("process_sample_4096-1024_shift_octave", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                acc += shifter.process_sample(black_box(s), 12.0, 1.0, false, 0, 2048);
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────── spectral_engine (gated) ──────────────────────────

#[cfg(feature = "spectral-engine")]
fn bench_spectral_engine(c: &mut Criterion) {
    use rustfft::num_complex::Complex;
    use tract_dsp::spectral_engine::{SpectralEngine, SpectralTransform};

    /// Identity transform — isolates the engine's analysis/synthesis OLA cost
    /// from any per-effect spectral work, the floor every Spectral effect pays.
    struct Identity;
    impl SpectralTransform for Identity {
        fn transform(&mut self, _s: &mut [Complex<f32>], _n: usize, _sr: f32) {}
    }

    let block = sine_block();
    let mut group = c.benchmark_group("spectral_engine");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // All four switchable FFT sizes — the multosis Spectral family runs any.
    for &fft_size in &[512usize, 1024, 2048, 4096] {
        let mut engine = SpectralEngine::new(48_000.0);
        engine.set_fft_size(fft_size);
        let mut id = Identity;
        group.bench_with_input(
            BenchmarkId::new("process_sample_identity", fft_size),
            &fft_size,
            |b, _| {
                b.iter(|| {
                    let mut acc = 0.0_f32;
                    for &s in &block {
                        acc += engine.process_sample(black_box(s), &mut id);
                    }
                    acc
                });
            },
        );
    }
    group.finish();
}

// ──────────────────────── stft / StftConvolver (gated) ─────────────────────

#[cfg(feature = "stft")]
fn bench_stft(c: &mut Criterion) {
    use tract_dsp::stft::StftConvolver;

    let block = sine_block();
    let mut group = c.benchmark_group("stft");
    group.throughput(Throughput::Elements(BLOCK as u64));

    // miff Phaseless / wavetable-filter magnitude-multiply OLA convolver.
    let frame = 4096;
    let mags = vec![1.0_f32; frame / 2 + 1];
    let mut conv = StftConvolver::new(frame);
    group.bench_function("process_4096_apply", |b| {
        b.iter(|| {
            let mut acc = 0.0_f32;
            for &s in &block {
                acc += conv.process(black_box(s), &mags, true);
            }
            acc
        });
    });
    group.finish();
}

// ──────────────────────────── orchestration ────────────────────────────────

fn all(c: &mut Criterion) {
    // Always-on, dependency-free primitives.
    bench_boxcar(c);
    bench_db(c);
    bench_fast_math(c);
    bench_fir(c);
    bench_hilbert(c);
    bench_window(c);
    bench_spsc(c);
    bench_true_peak(c);

    // FFT-backed paths, gated to match the library's feature flags.
    #[cfg(feature = "stft-analysis")]
    {
        bench_stft_analysis(c);
        bench_spectral_clipper(c);
        bench_spectral_shifter(c);
    }
    #[cfg(feature = "spectral-engine")]
    bench_spectral_engine(c);
    #[cfg(feature = "stft")]
    bench_stft(c);
}

criterion_group!(benches, all);
criterion_main!(benches);
