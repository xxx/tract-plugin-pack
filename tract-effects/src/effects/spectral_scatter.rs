//! Spectral Scatter: per-bin random delays, refreshed at Rate Hz.
//!
//! Each bin gets an independent random delay in [0, Length_hops). The delays
//! are reassigned every `1 / Rate` seconds. Feedback recycles delayed values
//! back into the ring so taps re-fire (with the new random delays each cycle).
//!
//! Implementation mirrors SpectralCascade's per-bin delay ring -- depth
//! capped at MAX_DELAY_HOPS hops so memory is bounded. Unlike Cascade's
//! linear-ramp delay map, Scatter assigns each bin an INDEPENDENT random
//! delay drawn from [0, Length_hops). Same dk=0 special-case applies:
//! when a bin's random delay rounds to zero we read the just-written
//! slot (i.e. the input), not the stale ring contents.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

/// Per-channel max delay depth in HOPS. Matches SpectralCascade.
const MAX_DELAY_HOPS: usize = 128;
/// Largest half-spectrum size we ever need (4096 / 2 + 1).
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralScatterChannel {
    /// Flattened 2D ring: `ring[hop_index * MAX_HALF_PLUS_ONE + bin_k]`.
    ring: Vec<Complex<f32>>,
    write_pos: usize,
    /// Per-bin random delay assignment in [0, Length_hops).
    delay_per_bin: Vec<u16>,
    hop_counter: u32,
    rng_state: u32,
}

impl SpectralScatterChannel {
    fn new(seed: u32) -> Self {
        Self {
            ring: vec![Complex::default(); MAX_DELAY_HOPS * MAX_HALF_PLUS_ONE],
            write_pos: 0,
            // Default all delays to 0 -- passthrough until the first refresh.
            delay_per_bin: vec![0u16; MAX_HALF_PLUS_ONE],
            hop_counter: 0,
            rng_state: seed.max(1),
        }
    }
    fn reset(&mut self) {
        self.ring.fill(Complex::default());
        self.write_pos = 0;
        self.delay_per_bin.fill(0);
        self.hop_counter = 0;
        // Keep rng_state across reset so successive presses don't give
        // identical delay sequences.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    length_ms: f32,
    feedback_pct: f32,
    rate_hz: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            length_ms: 200.0,
            feedback_pct: 0.0,
            rate_hz: 1.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralScatterChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_hops = (((self.params.length_ms * 0.001 * sample_rate) / hop_samples).round()
            as usize)
            .clamp(1, MAX_DELAY_HOPS - 1);
        let feedback = (self.params.feedback_pct * 0.01).clamp(0.0, 0.95);
        let rate = self.params.rate_hz.max(0.001);
        let hop_rate_hz = sample_rate / hop_samples;
        let hops_per_refresh = (hop_rate_hz / rate).round().max(1.0) as u32;

        // Refresh delay_per_bin every `hops_per_refresh` hops.
        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= hops_per_refresh {
            self.chan.hop_counter = 0;
            for k in 1..=half {
                self.chan.rng_state = xorshift(self.chan.rng_state);
                self.chan.delay_per_bin[k] = (self.chan.rng_state as usize % length_hops) as u16;
            }
        }

        let h_now = self.chan.write_pos;
        for k in 1..=half {
            let dk = self.chan.delay_per_bin[k] as i32;
            // Compute the output value FIRST so we can choose between
            // historical-tap (dk > 0) and just-written (dk == 0).
            let delayed = if dk == 0 {
                // dk == 0 reads "now". The ring at slot h_now hasn't been
                // written for this frame yet; the right answer is the input
                // value (no historical recycling for a zero-delay bin).
                spectrum[k]
            } else {
                let read_h = ((h_now as i32 - dk).rem_euclid(MAX_DELAY_HOPS as i32)) as usize;
                self.chan.ring[read_h * MAX_HALF_PLUS_ONE + k]
            };
            // Write input + feedback * delayed into the ring at h_now.
            let written = Complex::new(
                spectrum[k].re + feedback * delayed.re,
                spectrum[k].im + feedback * delayed.im,
            );
            self.chan.ring[h_now * MAX_HALF_PLUS_ONE + k] = written;
            spectrum[k] = delayed;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        self.chan.write_pos = (h_now + 1) % MAX_DELAY_HOPS;
    }
}

pub struct SpectralScatterEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralScatterChannel,
    chan_r: SpectralScatterChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralScatterEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Length",
            min: 10.0,
            max: 2000.0,
            default: 200.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " ms",
            },
        },
        ParamSpec {
            name: "Feedback",
            min: 0.0,
            max: 95.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Rate",
            min: 0.1,
            max: 10.0,
            default: 1.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        // FFT in the LAST slot so it isn't the first dial users grab to modulate.
        ParamSpec {
            name: "FFT",
            min: 0.0,
            max: 3.0,
            default: 2.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Enum {
                labels: &["512", "1024", "2048", "4096"],
            },
        },
    ];
}

impl Default for SpectralScatterEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralScatterChannel::new(0x5CA1_7E01),
            chan_r: SpectralScatterChannel::new(0x5CA1_7E02),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralScatterEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            chan: &mut self.chan_l,
            params: self.params,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            chan: &mut self.chan_r,
            params: self.params,
        };
        let ro = self.engine_r.process_sample(right, &mut ctx_r);
        (lo, ro)
    }
    fn parameters(&self) -> &'static [ParamSpec] {
        &Self::PARAMS
    }
    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.engine_l = SpectralEngine::new(sample_rate);
        self.engine_r = SpectralEngine::new(sample_rate);
        let fft_size = FFT_SIZES[self.params.fft_param.round().clamp(0.0, 3.0) as usize];
        self.engine_l.set_fft_size(fft_size);
        self.engine_r.set_fft_size(fft_size);
    }
    fn reset(&mut self) {
        self.engine_l.reset();
        self.engine_r.reset();
        self.chan_l.reset();
        self.chan_r.reset();
    }
    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.params.length_ms = value.clamp(10.0, 2000.0),
            1 => self.params.feedback_pct = value.clamp(0.0, 95.0),
            2 => self.params.rate_hz = value.clamp(0.1, 10.0),
            3 => {
                self.params.fft_param = value;
                let fft_size = FFT_SIZES[value.round().clamp(0.0, 3.0) as usize];
                self.engine_l.set_fft_size(fft_size);
                self.engine_r.set_fft_size(fft_size);
            }
            _ => {}
        }
    }
    fn latency_samples(&self) -> usize {
        self.engine_l.latency_samples()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn tagged_spectrum(size: usize, hop: usize) -> Vec<Complex<f32>> {
        let mut bins = vec![Complex::default(); size];
        bins[0] = Complex::new(-0.25 * (hop + 1) as f32, 0.0);
        for bin in 1..=size / 2 {
            bins[bin] = Complex::new(
                bin as f32 * 0.0078125 + hop as f32 * 0.0625,
                if bin == size / 2 {
                    0.0
                } else {
                    -0.5 - hop as f32 * 0.015625
                },
            );
            if bin < size / 2 {
                bins[size - bin] = bins[bin].conj();
            }
        }
        bins
    }

    fn transform(
        channel: &mut SpectralScatterChannel,
        bins: &mut [Complex<f32>],
        length: f32,
        feedback: f32,
        rate: f32,
        sample_rate: f32,
    ) {
        let size = bins.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                length_ms: length,
                feedback_pct: feedback,
                rate_hz: rate,
                ..ParamsCache::default()
            },
        }
        .transform(bins, size, sample_rate);
    }

    fn configured_scatter(fft_index: f32) -> SpectralScatterEffect {
        let mut effect = SpectralScatterEffect::default();
        for (index, value) in [200.0, 37.5, 10.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn xorshift_matches_known_vectors_and_repairs_a_zero_seed() {
        let mut state = 1;
        for expected in [
            0x0004_2021,
            0x0408_0601,
            0x9DCC_A8C5,
            0x1255_994F,
            0x8EF9_17D1,
            0x2C6F_5BD0,
            0x25B2_331A,
            0x19F9_1CB2,
            0x7787_7125,
            0xADD0_2374,
            0x9E60_02CB,
            0x591C_9737,
        ] {
            state = xorshift(state);
            assert_eq!(state, expected);
        }
        assert_eq!(xorshift(0), 1);
        assert_eq!(SpectralScatterChannel::new(0).rng_state, 1);
    }

    #[test]
    fn seeded_delay_assignments_follow_length_rounding_and_the_depth_cap() {
        for (size, sr, length, depth, total, head, last) in [
            (512, 32_000.0, 11.99, 1, 0, [0, 0, 0, 0, 0, 0, 0, 0], 0),
            (512, 32_000.0, 12.0, 2, 125, [1, 1, 1, 1, 1, 0, 0, 0], 0),
            (512, 48_000.0, 16.0, 3, 241, [0, 1, 0, 2, 2, 1, 1, 0], 2),
            (512, 48_000.0, 20.0, 4, 369, [1, 1, 1, 3, 1, 0, 2, 2], 0),
            (1_024, 32_000.0, 56.0, 4, 762, [1, 1, 1, 3, 1, 0, 2, 2], 2),
            (
                2_048,
                48_000.0,
                200.0,
                9,
                4_260,
                [0, 4, 6, 8, 8, 7, 1, 6],
                7,
            ),
            (
                4_096,
                48_000.0,
                200.0,
                5,
                4_072,
                [4, 4, 1, 0, 3, 4, 2, 0],
                2,
            ),
            (4_096, 48_000.0, 10.0, 1, 0, [0, 0, 0, 0, 0, 0, 0, 0], 0),
            (
                512,
                48_000.0,
                2_000.0,
                127,
                16_061,
                [113, 77, 65, 107, 101, 43, 120, 33],
                97,
            ),
        ] {
            let mut channel = SpectralScatterChannel::new(1);
            channel.hop_counter = 1_000_000;
            let original = tagged_spectrum(size, 0);
            let mut actual = original.clone();
            transform(&mut channel, &mut actual, length, 0.0, 10.0, sr);
            let delays = &channel.delay_per_bin[1..=size / 2];
            assert_eq!(&delays[..8], &head, "FFT {size}, Length {length}");
            assert_eq!(
                delays.iter().map(|&delay| delay as usize).sum::<usize>(),
                total
            );
            assert_eq!(*delays.last().unwrap(), last);
            assert!(delays.iter().all(|&delay| (delay as usize) < depth));
            assert_eq!(channel.delay_per_bin[0], 0);
            assert_eq!(actual[0], original[0]);
            for bin in 1..=size / 2 {
                let expected = if channel.delay_per_bin[bin] == 0 {
                    original[bin]
                } else {
                    Complex::default()
                };
                assert_eq!(actual[bin], expected);
                if bin < size / 2 {
                    assert_eq!(actual[size - bin], expected.conj());
                }
            }
        }
    }

    #[test]
    fn refresh_occurs_on_the_rounded_rate_boundary_and_continues_the_rng_stream() {
        for (size, sr, rate, interval, first_state, second_state) in [
            (512, 32_000.0, 10.0, 13, 0x54ED_A13C, 0x9648_C2BE),
            (1_024, 48_000.0, 10.0, 9, 0x9648_C2BE, 0x570E_C3C2),
            (2_048, 48_000.0, 10.0, 5, 0x570E_C3C2, 0x770A_F5A8),
            (4_096, 32_000.0, 10.0, 2, 0x770A_F5A8, 0x9D2A_B30A),
            (4_096, 8_000.0, 10.0, 1, 0x770A_F5A8, 0x9D2A_B30A),
        ] {
            let mut channel = SpectralScatterChannel::new(1);
            let mut held = channel.delay_per_bin.clone();
            for hop in 1..=2 * interval {
                let mut bins = tagged_spectrum(size, hop);
                let original = bins.clone();
                transform(&mut channel, &mut bins, 2_000.0, 0.0, rate, sr);
                if hop < interval {
                    assert_eq!(bins, original);
                    assert_eq!(channel.rng_state, 1);
                } else if hop < 2 * interval {
                    assert_eq!(channel.rng_state, first_state);
                } else {
                    assert_eq!(channel.rng_state, second_state);
                }
                assert_eq!(channel.hop_counter, (hop % interval) as u32);
                if hop % interval == 0 {
                    assert_ne!(channel.delay_per_bin, held);
                    held.clone_from(&channel.delay_per_bin);
                } else {
                    assert_eq!(channel.delay_per_bin, held);
                }
            }
        }
    }

    #[test]
    fn complex_delays_and_feedback_match_independent_queues_over_two_ring_wraps() {
        for size in [512, 1_024, 2_048, 4_096] {
            for (feedback_pct, feedback) in [(0.0, 0.0), (50.0, 0.5)] {
                let mut channel = SpectralScatterChannel::new(1);
                let mut queues = Vec::new();
                for bin in 0..=size / 2 {
                    let delay = [0, 1, 2, 7, 63, 126][bin % 6];
                    channel.delay_per_bin[bin] = delay as u16;
                    queues.push(VecDeque::from(vec![Complex::default(); delay]));
                }
                for hop in 0..259 {
                    let input = tagged_spectrum(size, hop);
                    let mut expected = input.clone();
                    for bin in 1..=size / 2 {
                        if let Some(delayed) = queues[bin].pop_front() {
                            queues[bin].push_back(input[bin] + delayed * feedback);
                            expected[bin] = delayed;
                        }
                        if bin < size / 2 {
                            expected[size - bin] = expected[bin].conj();
                        }
                    }
                    let mut actual = input;
                    transform(
                        &mut channel,
                        &mut actual,
                        2_000.0,
                        feedback_pct,
                        0.1,
                        192_000.0,
                    );
                    assert_eq!(
                        actual, expected,
                        "FFT {size}, hop {hop}, feedback {feedback_pct}"
                    );
                    assert_eq!(channel.write_pos, (hop + 1) % 128);
                }
            }
        }
    }

    #[test]
    fn zero_delay_outputs_input_and_a_later_tap_reads_the_feedback_write() {
        for (feedback, stored_gain) in [(0.0, 1.0), (50.0, 1.5), (95.0, 1.95)] {
            let mut channel = SpectralScatterChannel::new(1);
            channel.ring.fill(Complex::new(99.0, -9.0));
            let original = tagged_spectrum(512, 0);
            let mut bins = original.clone();
            transform(&mut channel, &mut bins, 200.0, feedback, 0.1, 48_000.0);
            assert_eq!(bins, original);
            channel.delay_per_bin[1] = 1;
            let mut next = tagged_spectrum(512, 1);
            transform(&mut channel, &mut next, 200.0, feedback, 0.1, 48_000.0);
            let expected = original[1] * stored_gain;
            assert!((next[1] - expected).norm() < 1e-6);
            assert_eq!(next[511], next[1].conj());
        }
    }

    #[test]
    fn reset_clears_history_and_assignments_but_preserves_rng_and_allocations() {
        let mut effect = SpectralScatterEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert!(channel.ring.len() >= 128 * 2_049 && channel.delay_per_bin.len() >= 2_049);
            let storage = (channel.ring.as_ptr(), channel.delay_per_bin.as_ptr());
            let lengths = (channel.ring.len(), channel.delay_per_bin.len());
            let capacities = (channel.ring.capacity(), channel.delay_per_bin.capacity());
            for size in [512, 4_096, 1_024, 2_048, 4_096] {
                let mut bins = tagged_spectrum(size, 0);
                channel.hop_counter = 1_000_000;
                transform(channel, &mut bins, 200.0, 50.0, 10.0, 48_000.0);
                let rng = channel.rng_state;
                channel.ring.fill(Complex::new(0.5, -0.25));
                channel.write_pos = 37;
                channel.hop_counter = 3;
                channel.reset();
                assert_eq!(
                    (channel.write_pos, channel.hop_counter, channel.rng_state),
                    (0, 0, rng)
                );
                assert!(
                    channel.delay_per_bin[..=2_048]
                        .iter()
                        .all(|&delay| delay == 0)
                );
                let original = tagged_spectrum(size, 1);
                let mut restored = original.clone();
                transform(channel, &mut restored, 200.0, 50.0, 0.1, 48_000.0);
                assert_eq!(restored, original);
                channel.delay_per_bin[1] = 126;
                let mut silence = vec![Complex::default(); size];
                transform(channel, &mut silence, 200.0, 50.0, 0.1, 48_000.0);
                assert!(silence.iter().all(|value| *value == Complex::default()));
                assert_eq!(channel.rng_state, rng);
                assert_eq!(
                    (channel.ring.as_ptr(), channel.delay_per_bin.as_ptr()),
                    storage
                );
                assert_eq!((channel.ring.len(), channel.delay_per_bin.len()), lengths);
                assert_eq!(
                    (channel.ring.capacity(), channel.delay_per_bin.capacity()),
                    capacities
                );
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralScatterEffect::default();
        assert_eq!(
            (
                effect.params.length_ms,
                effect.params.feedback_pct,
                effect.params.rate_hz,
                effect.params.fft_param
            ),
            (200.0, 0.0, 1.0, 2.0)
        );
        assert_eq!(effect.sample_rate, 48_000.0);
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|s| (s.name, s.min, s.max, s.default))
                .collect::<Vec<_>>(),
            [
                ("Length", 10.0, 2_000.0, 200.0),
                ("Feedback", 0.0, 95.0, 0.0),
                ("Rate", 0.1, 10.0, 1.0),
                ("FFT", 0.0, 3.0, 2.0)
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[2].scaling, ParamScaling::Log));
        assert!(matches!(specs[3].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 0,
                unit: " ms"
            }
        ));
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number {
                decimals: 0,
                unit: " %"
            }
        ));
        assert!(matches!(specs[2].format, ParamFormat::Hertz));
        assert!(
            matches!(specs[3].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, cases) in [
            (0, [(-999.0, 10.0), (37.5, 37.5), (9_999.0, 2_000.0)]),
            (1, [(-999.0, 0.0), (37.5, 37.5), (9_999.0, 95.0)]),
            (2, [(-999.0, 0.1), (3.75, 3.75), (9_999.0, 10.0)]),
        ] {
            for (value, expected) in cases {
                effect.set_param(slot, value);
                assert_eq!(
                    [
                        effect.params.length_ms,
                        effect.params.feedback_pct,
                        effect.params.rate_hz
                    ][slot],
                    expected
                );
            }
        }
        for (value, size) in [
            (-99.0, 512),
            (0.49, 512),
            (0.5, 1_024),
            (1.5, 2_048),
            (99.0, 4_096),
        ] {
            effect.set_param(3, value);
            assert_eq!(effect.latency_samples(), size);
            assert_eq!(effect.engine_r.latency_samples(), size);
        }
        effect.set_param(99, f32::INFINITY);
        assert_eq!(
            (
                effect.params.length_ms,
                effect.params.feedback_pct,
                effect.params.rate_hz
            ),
            (2_000.0, 95.0, 10.0)
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn one_hop_length_matches_an_independent_stereo_identity_reference() {
        struct Identity;
        impl SpectralTransform for Identity {
            fn transform(&mut self, _: &mut [Complex<f32>], _: usize, _: f32) {}
        }
        let mut effect = configured_scatter(3.0);
        effect.set_param(0, 10.0);
        let mut left = SpectralEngine::new(48_000.0);
        let mut right = SpectralEngine::new(48_000.0);
        left.set_fft_size(4_096);
        right.set_fft_size(4_096);
        let mut energy = [0.0; 2];
        for sample in 0..16_384 {
            let input = (
                (sample as f32 * 0.17).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            let expected = (
                left.process_sample(input.0, &mut Identity),
                right.process_sample(input.1, &mut Identity),
            );
            let actual = effect.process_sample(input.0, input.1);
            assert!(
                (actual.0 - expected.0).abs() < 1e-6 && (actual.1 - expected.1).abs() < 1e-6,
                "sample {sample}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
    }

    #[test]
    fn identical_stereo_input_produces_independent_random_delay_tails() {
        let mut effect = configured_scatter(0.0);
        let mut energy = [0.0; 2];
        let mut difference = 0.0;
        for sample in 0..24_576 {
            let input = if sample < 12_288 {
                (sample as f32 * 0.13).sin() * 0.25
            } else {
                0.0
            };
            let output = effect.process_sample(input, input);
            if sample >= 12_288 {
                energy[0] += output.0 * output.0;
                energy[1] += output.1 * output.1;
                difference += (output.0 - output.1).powi(2);
            }
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
        assert!(difference > 0.01);
        assert_ne!(effect.chan_l.rng_state, effect.chan_r.rng_state);
        assert_ne!(
            &effect.chan_l.delay_per_bin[..=256],
            &effect.chan_r.delay_per_bin[..=256]
        );
    }

    #[test]
    fn host_reset_clears_stereo_history_and_preserves_pending_fft_and_rng() {
        let mut used = configured_scatter(0.0);
        for sample in 0..16_384 {
            used.process_sample(
                (sample as f32 * 0.23).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
        }
        let rng = (used.chan_l.rng_state, used.chan_r.rng_state);
        used.set_param(3, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        assert_eq!((used.chan_l.rng_state, used.chan_r.rng_state), rng);
        let mut fresh = configured_scatter(0.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.chan_l = SpectralScatterChannel::new(rng.0);
        fresh.chan_r = SpectralScatterChannel::new(rng.1);
        fresh.set_param(3, 1.0);
        fresh.reset();
        let mut energy = [0.0; 2];
        for sample in 0..16_384 {
            let input = (
                (sample as f32 * 0.11).cos() * 0.125,
                (sample as f32 * 0.27).sin() * 0.25,
            );
            let actual = used.process_sample(input.0, input.1);
            assert_eq!(actual, fresh.process_sample(input.0, input.1));
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
    }

    #[test]
    fn sample_rate_configuration_and_reset_replay_a_fresh_selected_fft() {
        for index in 0..4 {
            let mut used = configured_scatter(index as f32);
            for sample in 0..16_384 {
                used.process_sample(
                    (sample as f32 * 0.19).sin() * 0.25,
                    (sample as f32 * 0.27).cos() * 0.125,
                );
            }
            let rng = (used.chan_l.rng_state, used.chan_r.rng_state);
            used.set_sample_rate(32_000.0);
            used.reset();
            let mut fresh = configured_scatter(index as f32);
            fresh.chan_l = SpectralScatterChannel::new(rng.0);
            fresh.chan_r = SpectralScatterChannel::new(rng.1);
            fresh.set_sample_rate(32_000.0);
            fresh.reset();
            assert_eq!(used.sample_rate, 32_000.0);
            assert_eq!(used.latency_samples(), [512, 1_024, 2_048, 4_096][index]);
            assert_eq!(used.engine_r.latency_samples(), used.latency_samples());
            let mut energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = (
                    (sample as f32 * 0.29).cos() * 0.25,
                    (sample as f32 * 0.13).sin() * 0.125,
                );
                let actual = used.process_sample(input.0, input.1);
                assert_eq!(actual, fresh.process_sample(input.0, input.1));
                energy[0] += actual.0 * actual.0;
                energy[1] += actual.1 * actual.1;
            }
            assert!(energy.into_iter().all(|e| e > 0.01));
        }
    }

    fn drive(e: &mut SpectralScatterEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralScatterEffect::default().parameters().len(), 4);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralScatterEffect::default();
        e.set_param(1, 50.0); // arbitrary Feedback < 95 (slot 1)
        let out = drive(&mut e, 8192, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn xorshift_never_returns_zero() {
        let mut s = 1u32;
        for _ in 0..1000 {
            s = xorshift(s);
            assert!(s != 0);
        }
    }
}
