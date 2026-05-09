//! Profiling harness for Imagine.
//!
//! Drives Imagine's per-sample DSP loop directly (encode → crossover → bands
//! → recover-sides → decode → display sinks), bypassing nih-plug's `Buffer`
//! plumbing and parameter smoothers. Measures the same hot path the real
//! plugin runs in a host. Stereo white noise input exercises the full
//! spectral surface.
//!
//! Build (with haswell auto-detected via xtask):
//!     cargo xtask native build --profile profiling --example profile -p imagine
//!
//! Profile a single scenario with perf:
//!     perf record -F 999 -g -- target/profiling/examples/profile
//!     perf report --no-children -g graph,0.5
//!
//! Each scenario reports a real-time factor (audio seconds per wall-clock
//! second). A summary table at the end makes it easy to compare scenarios
//! side-by-side.
//!
//! Scenarios (toggle individually if perf-recording one):
//!  - IIR / Linear at 48 kHz with static params (the common case)
//!  - Linear at 48 kHz with continuous crossover-frequency automation
//!    (worst case — both process_current AND process_pending run per
//!    sample on every LP, since the snap-promoted crossfade fires every
//!    block before the previous one completes)
//!  - Linear at 96 kHz (sample-rate scaling)
//!  - IIR at 48 kHz with band 0 soloed (different short-circuit path
//!    through Imagine::process)

use imagine::bands::{Band, StereoizeMode};
use imagine::crossover::{CrossoverFir, CrossoverIir};
use imagine::hilbert::HilbertFir;
use imagine::midside;
use imagine::spectrum::{Analyzer, SpectrumDisplay};
use imagine::vectorscope::{ring_pair, VectorProducer};
use imagine::{
    Quality, FIR_CROSSFADE_DEFAULT, FIR_CROSSOVER_LENGTH, FIR_HILBERT_LENGTH,
    HAAS_BUFFER_MAX_MS, HAAS_DEFAULT_MS, MAX_SAMPLE_RATE, NUM_BANDS,
};

use std::hint::black_box;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

const BLOCK: usize = 1024;
const N_BLOCKS: usize = 50_000;

const BAND_WIDTH: [f32; 4] = [-30.0, 20.0, 50.0, 80.0];
const BAND_STZ_MS: [f32; 4] = [HAAS_DEFAULT_MS; 4];
const BAND_STZ_ON: [bool; 4] = [false, true, true, false];
const BAND_MODE: [StereoizeMode; 4] = [
    StereoizeMode::ModeI,
    StereoizeMode::ModeI,
    StereoizeMode::ModeII,
    StereoizeMode::ModeII,
];
const XOVER_FREQS: [f32; 3] = [120.0, 1000.0, 8000.0];
const RECOVER_AMOUNT: f32 = 0.5;

/// xorshift32 — deterministic stereo noise.
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

#[derive(Copy, Clone, Debug)]
struct Scenario {
    name: &'static str,
    quality: Quality,
    sample_rate: f32,
    /// If true, redesign FIR/IIR crossovers every block with slightly drifted
    /// frequencies. Forces continuous crossfade in Linear mode and shows
    /// worst-case CPU during user crossover automation.
    automate_freqs: bool,
    /// If Some(b), band `b` is soloed throughout the run — short-circuits the
    /// recover-sides path and uses only that band's outs.
    solo_band: Option<usize>,
    /// If true, force widths/stz/recover to zero and feed silence — matches
    /// "plugin dropped into chain, audio not playing" in a host. Closest
    /// scenario to what a user sees in their DAW's DSP graph at idle.
    at_rest: bool,
}

#[derive(Debug)]
struct Result {
    scenario: Scenario,
    rt_factor: f64,
    cpu_pct_per_instance: f64,
}

fn run(scenario: Scenario) -> Result {
    eprintln!(
        "── {} ({:?} @ {:.0} Hz, automate={}, solo={:?}, at_rest={}) ──",
        scenario.name,
        scenario.quality,
        scenario.sample_rate,
        scenario.automate_freqs,
        scenario.solo_band,
        scenario.at_rest,
    );

    let (widths, stz_ms, stz_on, recover) = if scenario.at_rest {
        ([0.0_f32; 4], [HAAS_DEFAULT_MS; 4], [false; 4], 0.0_f32)
    } else {
        (BAND_WIDTH, BAND_STZ_MS, BAND_STZ_ON, RECOVER_AMOUNT)
    };

    let sr = scenario.sample_rate;

    let mut crossover_iir_m = CrossoverIir::default();
    let mut crossover_iir_s = CrossoverIir::default();
    let mut crossover_fir_m = CrossoverFir::new(FIR_CROSSOVER_LENGTH);
    let mut crossover_fir_s = CrossoverFir::new(FIR_CROSSOVER_LENGTH);

    crossover_iir_m.redesign(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], sr);
    crossover_iir_s.redesign(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], sr);
    crossover_fir_m.initialize(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], sr);
    crossover_fir_s.initialize(XOVER_FREQS[0], XOVER_FREQS[1], XOVER_FREQS[2], sr);

    // Touch the crossfade-redesign path once so any cold-start cost lands
    // outside the timed loop (only for non-automated scenarios — automated
    // ones get plenty of redesign exercise inside the timed loop).
    if !scenario.automate_freqs {
        crossover_fir_m.redesign(
            XOVER_FREQS[0],
            XOVER_FREQS[1],
            XOVER_FREQS[2],
            sr,
            FIR_CROSSFADE_DEFAULT,
        );
        crossover_fir_s.redesign(
            XOVER_FREQS[0],
            XOVER_FREQS[1],
            XOVER_FREQS[2],
            sr,
            FIR_CROSSFADE_DEFAULT,
        );
    }

    let mut bands: [Band; 4] =
        std::array::from_fn(|_| Band::new(HAAS_BUFFER_MAX_MS, MAX_SAMPLE_RATE));
    for b in bands.iter_mut() {
        b.set_sample_rate(sr);
    }

    let mut hilbert = HilbertFir::new(FIR_HILBERT_LENGTH);
    let dry_n = hilbert.latency_samples();
    let mut dry_delay_m = vec![0.0_f32; dry_n];
    let mut dry_delay_s = vec![0.0_f32; dry_n];
    let mut dry_idx: usize = 0;

    let display = SpectrumDisplay::new();
    let mut spectrum = Analyzer::new(sr, display);
    let (vector_producer, _vector_consumer): (VectorProducer, _) = ring_pair();
    let correlation = AtomicU32::new(0);
    let balance = AtomicU32::new(0);
    let mut meter = MeterAccum::default();

    let mut l = vec![0.0_f32; BLOCK];
    let mut r = vec![0.0_f32; BLOCK];
    let mut rng_l = Rng(0xC0FFEE_u32);
    let mut rng_r = Rng(0xFEEDFACE_u32);

    let total_samples: u64 = N_BLOCKS as u64 * BLOCK as u64;
    let solo_idx = scenario.solo_band;
    let start = Instant::now();

    for block in 0..N_BLOCKS {
        // Generate a fresh stereo block — white noise normally, silence at rest.
        if scenario.at_rest {
            l.fill(0.0);
            r.fill(0.0);
        } else {
            for i in 0..BLOCK {
                l[i] = rng_l.next_f32() * 0.5;
                r[i] = rng_r.next_f32() * 0.5;
            }
        }

        // Optional per-block crossover automation. Slightly drift each freq
        // every block so the >0.5 Hz redesign gate fires every time.
        if scenario.automate_freqs {
            let phase = (block as f32) * 0.05;
            let f1 = XOVER_FREQS[0] + 5.0 * phase.sin();
            let f2 = XOVER_FREQS[1] + 30.0 * (phase * 0.7).sin();
            let f3 = XOVER_FREQS[2] + 200.0 * (phase * 0.3).sin();
            match scenario.quality {
                Quality::Iir => {
                    crossover_iir_m.redesign(f1, f2, f3, sr);
                    crossover_iir_s.redesign(f1, f2, f3, sr);
                }
                Quality::Linear => {
                    crossover_fir_m.redesign(f1, f2, f3, sr, FIR_CROSSFADE_DEFAULT);
                    crossover_fir_s.redesign(f1, f2, f3, sr, FIR_CROSSFADE_DEFAULT);
                }
            }
        }

        for i in 0..BLOCK {
            let (m_in, s_in) = midside::encode(l[i], r[i]);
            let (m_bands, s_bands) = match scenario.quality {
                Quality::Iir => (
                    crossover_iir_m.process(m_in),
                    crossover_iir_s.process(s_in),
                ),
                Quality::Linear => (
                    crossover_fir_m.process(m_in),
                    crossover_fir_s.process(s_in),
                ),
            };

            let mut m_outs = [0.0_f32; NUM_BANDS];
            let mut s_outs = [0.0_f32; NUM_BANDS];
            let mut m_sum = 0.0_f32;
            let mut s_sum = 0.0_f32;
            let mut s_removed_total = 0.0_f32;
            for b in 0..NUM_BANDS {
                let (m_o, s_o, s_rem) = bands[b].process(
                    m_bands[b],
                    s_bands[b],
                    widths[b],
                    stz_ms[b],
                    stz_on[b],
                    BAND_MODE[b],
                );
                m_outs[b] = m_o;
                s_outs[b] = s_o;
                m_sum += m_o;
                s_sum += s_o;
                s_removed_total += s_rem;
            }

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

            let recover_inject = hilbert.process(s_removed_total) * recover;

            let (m_final, s_final) = if let Some(idx) = solo_idx {
                (m_outs[idx], s_outs[idx])
            } else {
                (m_d + recover_inject, s_d)
            };

            let (l_out, r_out) = midside::decode(m_final, s_final);
            l[i] = l_out;
            r[i] = r_out;

            vector_producer.push(l_out, r_out);
            spectrum.push(m_final, s_final);
            if let Some((corr, bal)) = meter.push(l_out, r_out) {
                correlation.store(corr.to_bits(), Ordering::Relaxed);
                balance.store(bal.to_bits(), Ordering::Relaxed);
            }
        }

        black_box(&l);
        black_box(&r);
    }

    let elapsed = start.elapsed();
    let audio_seconds = total_samples as f64 / sr as f64;
    let rt_factor = audio_seconds / elapsed.as_secs_f64();
    let cpu_pct = 100.0 / rt_factor;

    eprintln!(
        "  audio={:.1}s wall={:.3}s  RT={:>6.1}×  cpu/inst={:>5.2}%",
        audio_seconds,
        elapsed.as_secs_f64(),
        rt_factor,
        cpu_pct,
    );
    eprintln!();

    Result {
        scenario,
        rt_factor,
        cpu_pct_per_instance: cpu_pct,
    }
}

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
    let scenarios = [
        // "at rest" — silent input + default params. Mirrors what a user sees
        // in their DAW's DSP graph immediately after dropping the plugin into
        // a chain (audio not playing, nothing tweaked yet).
        Scenario {
            name: "Linear / 48kHz / AT REST (silent + defaults)",
            quality: Quality::Linear,
            sample_rate: 48_000.0,
            automate_freqs: false,
            solo_band: None,
            at_rest: true,
        },
        Scenario {
            name: "IIR / 48kHz / AT REST (silent + defaults)",
            quality: Quality::Iir,
            sample_rate: 48_000.0,
            automate_freqs: false,
            solo_band: None,
            at_rest: true,
        },
        Scenario {
            name: "IIR / 48kHz / static",
            quality: Quality::Iir,
            sample_rate: 48_000.0,
            automate_freqs: false,
            solo_band: None,
            at_rest: false,
        },
        Scenario {
            name: "Linear / 48kHz / static",
            quality: Quality::Linear,
            sample_rate: 48_000.0,
            automate_freqs: false,
            solo_band: None,
            at_rest: false,
        },
        Scenario {
            name: "Linear / 48kHz / CROSSOVER AUTOMATION",
            quality: Quality::Linear,
            sample_rate: 48_000.0,
            automate_freqs: true,
            solo_band: None,
            at_rest: false,
        },
        Scenario {
            name: "Linear / 96kHz / static",
            quality: Quality::Linear,
            sample_rate: 96_000.0,
            automate_freqs: false,
            solo_band: None,
            at_rest: false,
        },
        Scenario {
            name: "IIR / 48kHz / static / SOLO band 0",
            quality: Quality::Iir,
            sample_rate: 48_000.0,
            automate_freqs: false,
            solo_band: Some(0),
            at_rest: false,
        },
    ];

    let results: Vec<Result> = scenarios.iter().map(|s| run(*s)).collect();

    eprintln!("─────── summary ───────");
    eprintln!(
        "{:<48}  {:>8}  {:>8}",
        "scenario", "RT-factor", "cpu/inst"
    );
    for r in &results {
        eprintln!(
            "{:<48}  {:>7.1}×  {:>7.2}%",
            r.scenario.name, r.rt_factor, r.cpu_pct_per_instance
        );
    }
}
