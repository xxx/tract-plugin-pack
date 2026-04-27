//! Profiling harness for Six Pack.
//!
//! Runs an in-process simulation of the plugin's per-block DSP loop driven
//! by a chirping sine, so `perf record` on this binary measures the same
//! hot path the real plugin runs in a host. We bypass nih-plug's `Buffer`
//! plumbing (and the parameter smoothers) — the goal is to expose the DSP
//! cost: the SVFs, saturators, M/S routing, oversampling cascade, and the
//! two spectrum analyzers, processing realistic stereo audio with all six
//! bands active.
//!
//! Build:
//!     cargo build --profile profiling --example profile -p six-pack
//!
//! Profile:
//!     perf record -F 999 -g -- target/profiling/examples/profile
//!     perf report --no-children -g graph,0.5
//!
//! The harness prints a real-time factor at the end (audio seconds per
//! wall-clock second). Use it to compare baseline vs. post-optimization
//! runs on the same machine.

use six_pack::bands::{BandState, ChannelMode, FilterShape};
use six_pack::oversampling::StereoOversampler;
use six_pack::saturation::Algorithm;
use six_pack::spectrum::SpectrumAnalyzer;
use std::hint::black_box;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 1024;
const N_BLOCKS: usize = 50_000; // ~1067 s of audio at SR (~40 s wall time)

const BAND_SHAPES: [FilterShape; 6] = [
    FilterShape::LowShelf,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::Peak,
    FilterShape::HighShelf,
];

const BAND_FREQS: [f32; 6] = [60.0, 180.0, 540.0, 1_600.0, 4_800.0, 12_000.0];

// Realistic mix: every band at +6 dB so all are active, and a spread of
// algorithms / channel modes so we exercise all six saturation paths plus
// every M/S branch.
const BAND_GAIN_DB: f32 = 6.0;
const BAND_Q: f32 = 0.71;
const ALGOS: [Algorithm; 6] = [
    Algorithm::Tube,
    Algorithm::Tape,
    Algorithm::Diode,
    Algorithm::Digital,
    Algorithm::ClassB,
    Algorithm::Wavefold,
];
const MODES: [ChannelMode; 6] = [
    ChannelMode::Stereo,
    ChannelMode::Stereo,
    ChannelMode::Mid,
    ChannelMode::Stereo,
    ChannelMode::Side,
    ChannelMode::Stereo,
];

// Quality factor: 4× exercises the polyphase cascade without making the
// run absurdly long. Switch to 1, 8, or 16 to profile other quality tiers.
const OS_FACTOR: usize = 4;

const DRIVE_K: f32 = 1.0; // Color
const DEEMPH: bool = true;
const MIX: f32 = 0.5;

#[inline]
fn dry_amp(m: f32) -> f32 {
    (2.0 * (1.0 - m)).clamp(0.0, 1.0)
}
#[inline]
fn wet_amp(m: f32) -> f32 {
    (2.0 * m).clamp(0.0, 1.0)
}

fn main() {
    let effective_sr = SR * OS_FACTOR as f32;

    let mut bands: [BandState; 6] = std::array::from_fn(|i| {
        let mut b = BandState::new(BAND_SHAPES[i]);
        b.algo = ALGOS[i];
        b.mode = MODES[i];
        b.freq_hz = BAND_FREQS[i];
        b.q = BAND_Q;
        b.gain_db = BAND_GAIN_DB;
        b.enable = 1.0;
        b.recompute_coefs(effective_sr);
        b
    });

    let mut os = StereoOversampler::new();
    os.set_factor(OS_FACTOR, BLOCK);

    let mut spectrum = SpectrumAnalyzer::new(0xC0FFEE);
    let mut spectrum_wet = SpectrumAnalyzer::new(0xC0FFEE ^ 0x5A5A_5A5A);
    let band_activity: [AtomicU32; 6] = std::array::from_fn(|_| AtomicU32::new(0));

    let mut l = vec![0.0_f32; BLOCK];
    let mut r = vec![0.0_f32; BLOCK];

    let dry_amp_v = dry_amp(MIX);
    let wet_amp_v = wet_amp(MIX);

    let total_samples: u64 = N_BLOCKS as u64 * BLOCK as u64;
    let start = Instant::now();

    for block in 0..N_BLOCKS {
        // Generate a stereo chirp sweeping 100 Hz → 10 kHz exponentially over
        // 4 seconds, looping. Slight L/R phase difference so the Side band
        // sees actual side content.
        let phase_offset = (block * BLOCK) as f32 / SR;
        for i in 0..BLOCK {
            let t = phase_offset + i as f32 / SR;
            let cycle = (t % 4.0) / 4.0;
            let f = 100.0 * 100.0_f32.powf(cycle);
            let phase = 2.0 * std::f32::consts::PI * f * t;
            l[i] = phase.sin() * 0.3;
            r[i] = (phase + 0.05).cos() * 0.3;
        }

        // Mirror SixPack::process()'s structure, minus the nih-plug Buffer
        // overhead and the parameter smoothers.

        // Native-rate input spectrum push.
        for i in 0..BLOCK {
            let m = (l[i] + r[i]) * 0.5;
            spectrum.push_sample(m);
        }

        // Upsample.
        let (os_l, os_r) = os.upsample_block(&l, &r);
        let len_os = os_l.len();

        let mut band_sumsq = [0.0_f32; 6];
        for i in 0..len_os {
            let dry_l = os_l[i];
            let dry_r = os_r[i];

            let mut wet_l = 0.0_f32;
            let mut wet_r = 0.0_f32;
            let mut boost_l = 0.0_f32;
            let mut boost_r = 0.0_f32;
            for (b, band) in bands.iter_mut().enumerate() {
                let out = band.process_sample(dry_l, dry_r, DRIVE_K);
                wet_l += out.sat_l;
                wet_r += out.sat_r;
                boost_l += out.boost_l;
                boost_r += out.boost_r;
                band_sumsq[b] += out.sat_l * out.sat_l + out.sat_r * out.sat_r;
            }
            if DEEMPH {
                wet_l -= boost_l;
                wet_r -= boost_r;
            }

            if i % OS_FACTOR == 0 {
                spectrum_wet.push_sample((wet_l + wet_r) * 0.5);
            }

            os_l[i] = dry_amp_v * dry_l + wet_amp_v * wet_l;
            os_r[i] = dry_amp_v * dry_r + wet_amp_v * wet_r;
        }

        if len_os > 0 {
            let inv = 1.0 / (2.0 * len_os as f32);
            for (b, sumsq) in band_sumsq.iter().enumerate() {
                let rms = (sumsq * inv).sqrt();
                band_activity[b].store(rms.to_bits(), Ordering::Relaxed);
            }
        }

        // Downsample back to native rate.
        os.downsample_block(&mut l, &mut r);

        // Prevent the optimizer from concluding we don't need the outputs.
        black_box(&l);
        black_box(&r);
    }

    let elapsed = start.elapsed();
    let audio_seconds = total_samples as f64 / SR as f64;
    let realtime = audio_seconds / elapsed.as_secs_f64();

    eprintln!(
        "Six Pack profile harness  ({} bands, OS={}×, {}-sample blocks)",
        bands.len(),
        OS_FACTOR,
        BLOCK,
    );
    eprintln!(
        "Processed {} samples ({:.2} s of audio) in {:.3} s wall time",
        total_samples,
        audio_seconds,
        elapsed.as_secs_f64()
    );
    eprintln!("Real-time factor: {:.1}× (higher is better)", realtime);
}
