//! Profiling harness for Imagine.
//!
//! Drives Imagine's per-sample DSP loop directly (encode → crossover → bands
//! → recover-sides → decode → display sinks), bypassing nih-plug's `Buffer`
//! plumbing and parameter smoothers. Measures the same hot path the real
//! plugin runs in a host. Stereo white noise input exercises the full
//! spectral surface so the FFT analyzer and per-band crossover don't get a
//! free ride on quiet inputs.
//!
//! Build (with haswell auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example profile -p imagine
//!
//! Profile:
//!     perf record -F 999 -g -- target/profiling/examples/profile
//!     perf report --no-children -g graph,0.5
//!
//! The harness prints a real-time factor at the end (audio seconds per
//! wall-clock second). Use it to compare baseline vs. post-optimization
//! runs on the same machine. Switch QUALITY between Linear and Iir to
//! profile the two different crossover paths.
//!
//! Profiles BOTH crossover qualities sequentially by default; comment out
//! one of the run() calls in main() if you only need one.

use imagine::bands::{Band, StereoizeMode};
use imagine::crossover::{CrossoverFir, CrossoverIir};
use imagine::hilbert::HilbertFir;
use imagine::midside;
use imagine::spectrum::{Analyzer, SpectrumDisplay};
use imagine::vectorscope::{ring_pair, VectorProducer};
use imagine::{
    Quality, FIR_CROSSFADE_DEFAULT, FIR_CROSSOVER_LENGTH, FIR_HILBERT_LENGTH, HAAS_DEFAULT_MS,
    HAAS_MAX_MS, MAX_SAMPLE_RATE, NUM_BANDS,
};

use std::hint::black_box;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 1024;
const N_BLOCKS: usize = 50_000; // ~1067 s of audio at SR

// Realistic per-band settings. Spread Width / Stereoize amount / Mode so
// every audio path is exercised in the same run:
//   Band 0 (low):  Width=-30 (slight narrowing), Stz=0,  Mode I  — exercises S_removed gating + Haas advance
//   Band 1 (lo-mid): Width=+20 (slight widening), Stz=20, Mode I — exercises Haas inject
//   Band 2 (hi-mid): Width=+50, Stz=40, Mode II — exercises decorrelator (6-stage all-pass)
//   Band 3 (high):   Width=+80, Stz=10, Mode II — exercises decorrelator with low amount
const BAND_WIDTH: [f32; 4] = [-30.0, 20.0, 50.0, 80.0];
const BAND_STZ: [f32; 4] = [0.0, 20.0, 40.0, 10.0];
const BAND_MODE: [StereoizeMode; 4] = [
    StereoizeMode::ModeI,
    StereoizeMode::ModeI,
    StereoizeMode::ModeII,
    StereoizeMode::ModeII,
];

const XOVER_FREQS: [f32; 3] = [120.0, 1000.0, 8000.0];
const RECOVER_AMOUNT: f32 = 0.5; // 50% — keeps Hilbert FIR on the hot path

/// xorshift32 — deterministic stereo noise (different seeds for L/R so
/// the Side channel has real content).
struct Rng(u32);
impl Rng {
    fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        ((x as f32) / (u32::MAX as f32)) * 2.0 - 1.0
    }
}

fn run(quality: Quality) {
    eprintln!(
        "── Imagine profile harness  ({:?} quality, {} bands, {}-sample blocks) ──",
        quality, NUM_BANDS, BLOCK
    );

    // ── Per-block / per-sample DSP state ─────────────────────────────
    let mut crossover_iir_m = CrossoverIir::default();
    let mut crossover_iir_s = CrossoverIir::default();
    let mut crossover_fir_m = CrossoverFir::new(FIR_CROSSOVER_LENGTH);
    let mut crossover_fir_s = CrossoverFir::new(FIR_CROSSOVER_LENGTH);

    crossover_iir_m.redesign(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], SR);
    crossover_iir_s.redesign(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], SR);
    crossover_fir_m.initialize(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], SR);
    crossover_fir_s.initialize(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], SR);
    // Touch the crossfade-redesign path once so any cold-start cost lands
    // outside the timed loop.
    crossover_fir_m.redesign(
        XOVER_FREQS[0],
        XOVER_FREQS[1],
        XOVER_FREQS[2],
        SR,
        FIR_CROSSFADE_DEFAULT,
    );
    crossover_fir_s.redesign(
        XOVER_FREQS[0],
        XOVER_FREQS[1],
        XOVER_FREQS[2],
        SR,
        FIR_CROSSFADE_DEFAULT,
    );

    let mut bands: [Band; 4] =
        std::array::from_fn(|_| Band::new(HAAS_MAX_MS, MAX_SAMPLE_RATE));
    for b in bands.iter_mut() {
        b.set_sample_rate(SR, HAAS_DEFAULT_MS);
    }

    let mut hilbert = HilbertFir::new(FIR_HILBERT_LENGTH);

    // Dry-delay aligns M_sum/S_sum with hilbert.process(s_removed_total) on
    // the recover injection path. Length = hilbert.latency_samples().
    let dry_n = hilbert.latency_samples();
    let mut dry_delay_m = vec![0.0_f32; dry_n];
    let mut dry_delay_s = vec![0.0_f32; dry_n];
    let mut dry_idx: usize = 0;

    // ── Display sinks (keep them on the hot path; the editor is real) ──
    let display = SpectrumDisplay::new();
    let mut spectrum = Analyzer::new(SR, display);
    let (vector_producer, _vector_consumer): (VectorProducer, _) = ring_pair();
    let correlation = AtomicU32::new(0);
    let balance = AtomicU32::new(0);
    let mut meter = MeterAccum::default();

    // ── I/O buffers ──────────────────────────────────────────────────
    let mut l = vec![0.0_f32; BLOCK];
    let mut r = vec![0.0_f32; BLOCK];
    let mut rng_l = Rng(0xC0FFEE_u32);
    let mut rng_r = Rng(0xFEEDFACE_u32);

    let total_samples: u64 = N_BLOCKS as u64 * BLOCK as u64;
    let start = Instant::now();

    for _block in 0..N_BLOCKS {
        // Stereo white noise. Different RNGs for L/R so S = (L-R)/2 has
        // real content and the side path isn't pessimistically silent.
        for i in 0..BLOCK {
            l[i] = rng_l.next_f32() * 0.5;
            r[i] = rng_r.next_f32() * 0.5;
        }

        for i in 0..BLOCK {
            // 1. Encode L/R → M, S.
            let (m_in, s_in) = midside::encode(l[i], r[i]);

            // 2. Crossover (one variant on the hot path; the other sits idle).
            let (m_bands, s_bands) = match quality {
                Quality::Iir => (
                    crossover_iir_m.process(m_in),
                    crossover_iir_s.process(s_in),
                ),
                Quality::Linear => (
                    crossover_fir_m.process(m_in),
                    crossover_fir_s.process(s_in),
                ),
            };

            // 3. Per-band processing: Width + Stereoize, accumulate S_removed.
            let mut m_sum = 0.0_f32;
            let mut s_sum = 0.0_f32;
            let mut s_removed_total = 0.0_f32;
            for b in 0..NUM_BANDS {
                let (m_o, s_o, s_rem) = bands[b].process(
                    m_bands[b],
                    s_bands[b],
                    BAND_WIDTH[b],
                    BAND_STZ[b],
                    BAND_MODE[b],
                );
                m_sum += m_o;
                s_sum += s_o;
                s_removed_total += s_rem;
            }

            // 4. Dry-delay so M_sum / S_sum line up with hilbert.process()'s
            //    group delay on the recover-sides injection.
            let (m_d, s_d) = if dry_n == 0 {
                (m_sum, s_sum)
            } else {
                let m_old = dry_delay_m[dry_idx];
                let s_old = dry_delay_s[dry_idx];
                dry_delay_m[dry_idx] = m_sum;
                dry_delay_s[dry_idx] = s_sum;
                dry_idx = if dry_idx + 1 == dry_n { 0 } else { dry_idx + 1 };
                (m_old, s_old)
            };

            let recover_inject = hilbert.process(s_removed_total) * RECOVER_AMOUNT;

            // 5. Decode M, S → L, R (no Solo override in this harness).
            let m_final = m_d + recover_inject;
            let s_final = s_d;
            let (l_out, r_out) = midside::decode(m_final, s_final);
            l[i] = l_out;
            r[i] = r_out;

            // 6. Display sinks — they cost real cycles in the plugin, so
            //    profile with them attached.
            vector_producer.push(l_out, r_out);
            spectrum.push(m_final, s_final);
            if let Some((corr, bal)) = meter.push(l_out, r_out) {
                correlation.store(corr.to_bits(), Ordering::Relaxed);
                balance.store(bal.to_bits(), Ordering::Relaxed);
            }
        }

        // Don't let the optimizer conclude the outputs are unobserved.
        black_box(&l);
        black_box(&r);
    }

    let elapsed = start.elapsed();
    let audio_seconds = total_samples as f64 / SR as f64;
    let realtime = audio_seconds / elapsed.as_secs_f64();

    eprintln!(
        "  Processed {} samples ({:.2} s of audio) in {:.3} s wall time",
        total_samples,
        audio_seconds,
        elapsed.as_secs_f64()
    );
    eprintln!("  Real-time factor: {:.1}× (higher is better)", realtime);
    eprintln!(
        "  Per-instance @ 48 kHz: ~{:.3}% of one core",
        100.0 / realtime
    );
    eprintln!();
}

// ── MeterAccum (mirrored from lib.rs::process so the harness exercises
//    the same publish cadence as the plugin). 1024-sample window, Pearson
//    correlation + L/R energy balance.
#[derive(Default)]
struct MeterAccum {
    sum_lr: f32,
    sum_ll: f32,
    sum_rr: f32,
    samples: usize,
}
impl MeterAccum {
    const WINDOW: usize = 1024;
    fn push(&mut self, l: f32, r: f32) -> Option<(f32, f32)> {
        self.sum_lr += l * r;
        self.sum_ll += l * l;
        self.sum_rr += r * r;
        self.samples += 1;
        if self.samples >= Self::WINDOW {
            let denom = (self.sum_ll * self.sum_rr).sqrt() + 1e-12;
            let correlation = (self.sum_lr / denom).clamp(-1.0, 1.0);
            let total = self.sum_ll + self.sum_rr + 1e-12;
            let balance = ((self.sum_rr - self.sum_ll) / total).clamp(-1.0, 1.0);
            *self = Self::default();
            Some((correlation, balance))
        } else {
            None
        }
    }
}

fn main() {
    run(Quality::Iir);
    run(Quality::Linear);
}
