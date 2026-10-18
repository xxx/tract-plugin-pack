//! Spectral Lofi: bin decimation with optional randomisation.
//!
//! A bitmask of which bins to keep is refreshed every `Slow` hops. At
//! Randomise=0 we keep every Nth bin (regular decimation, N = 1/(1-Factor)).
//! At Randomise=100 we keep each bin with independent probability (1-Factor).
//! In between we lerp the keep probability between the two rules. Bins not
//! kept are zeroed.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

/// Maximum half-spectrum entries we'll ever need to mask -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// xorshift32 -- audio-thread-safe (no allocation, no transcendentals).
/// Returns the next state; the caller threads it through.
fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralLofiChannel {
    keep_mask: Vec<bool>,
    hop_counter: u32,
    rng_state: u32,
}

impl SpectralLofiChannel {
    fn new(seed: u32) -> Self {
        Self {
            // Start with all-true so a fresh effect at Factor=0 is passthrough.
            keep_mask: vec![true; MAX_HALF_PLUS_ONE],
            hop_counter: 0,
            // Seed must be non-zero for xorshift to do anything useful.
            rng_state: seed.max(1),
        }
    }

    fn reset(&mut self) {
        self.keep_mask.fill(true);
        self.hop_counter = 0;
        // Don't reset rng_state -- otherwise reset would produce
        // identical masks every time.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    factor_pct: f32,
    randomise_pct: f32,
    slow: i32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            factor_pct: 0.0,
            randomise_pct: 0.0,
            slow: 1,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralLofiChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let factor = (self.params.factor_pct * 0.01).clamp(0.0, 1.0);
        if factor <= 1e-6 {
            return; // Factor=0 -> keep everything
        }
        let randomise = (self.params.randomise_pct * 0.01).clamp(0.0, 1.0);
        let slow = self.params.slow.clamp(1, 100) as u32;

        // Refresh keep_mask every `slow` hops.
        self.chan.hop_counter = self.chan.hop_counter.wrapping_add(1);
        if self.chan.hop_counter >= slow {
            self.chan.hop_counter = 0;
            // Regular vote spacing: keep iff (k mod step) < 0.5.
            // step >= 1.0; e.g. factor=0.5 -> step=2 (keep every 2nd bin).
            let step = (1.0 / (1.0 - factor)).max(1.0);
            for k in 0..=half {
                let regular = ((k as f32) % step) < 0.5;
                self.chan.rng_state = xorshift(self.chan.rng_state);
                let r = (self.chan.rng_state as f32) / (u32::MAX as f32);
                let random_vote = r > factor;
                // Lerp the two binary votes into a probability and threshold at 0.5.
                let p = (1.0 - randomise) * (regular as i32 as f32)
                    + randomise * (random_vote as i32 as f32);
                self.chan.keep_mask[k] = p > 0.5;
            }
        }
        // Apply the mask.
        for k in 0..=half {
            if !self.chan.keep_mask[k] {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

pub struct SpectralLofiEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralLofiChannel,
    chan_r: SpectralLofiChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralLofiEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Factor",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Random",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Slow",
            min: 1.0,
            max: 100.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " hops",
            },
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

impl Default for SpectralLofiEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        // Seed L and R with different non-zero values so their masks
        // are independent.
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralLofiChannel::new(0xC001_C0DE),
            chan_r: SpectralLofiChannel::new(0xDEAD_BEEF),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralLofiEffect {
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
            0 => self.params.factor_pct = value.clamp(0.0, 100.0),
            1 => self.params.randomise_pct = value.clamp(0.0, 100.0),
            2 => self.params.slow = value.round().clamp(1.0, 100.0) as i32,
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

    fn spectrum(size: usize) -> Vec<Complex<f32>> {
        let mut bins = vec![Complex::default(); size];
        bins[0] = Complex::new(-0.75, 0.0);
        bins[size / 2] = Complex::new(0.25, 0.0);
        for bin in 1..size / 2 {
            bins[bin] = Complex::new((bin + 1) as f32 * 0.125, -0.5);
            bins[size - bin] = bins[bin].conj();
        }
        bins
    }

    fn masked(source: &[Complex<f32>], keep: impl Fn(usize) -> bool) -> Vec<Complex<f32>> {
        let size = source.len();
        source
            .iter()
            .enumerate()
            .map(|(bin, &value)| {
                if keep(bin.min(size - bin)) {
                    value
                } else {
                    Complex::default()
                }
            })
            .collect()
    }

    fn transform(
        channel: &mut SpectralLofiChannel,
        bins: &mut [Complex<f32>],
        factor: f32,
        random: f32,
        slow: i32,
    ) {
        let size = bins.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                factor_pct: factor,
                randomise_pct: random,
                slow,
                ..ParamsCache::default()
            },
        }
        .transform(bins, size, 48_000.0);
    }

    fn configured_lofi(fft_index: f32) -> SpectralLofiEffect {
        let mut effect = SpectralLofiEffect::default();
        for (index, value) in [50.0, 0.0, 3.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn regular_decimation_preserves_known_bin_patterns_at_every_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            let original = spectrum(size);
            for (factor, period) in [(50.0, 2), (75.0, 4), (87.5, 8), (93.75, 16)] {
                let mut channel = SpectralLofiChannel::new(1);
                let mut actual = original.clone();
                transform(&mut channel, &mut actual, factor, 0.0, 1);
                assert_eq!(actual, masked(&original, |bin| bin % period == 0));
            }
            let mut channel = SpectralLofiChannel::new(1);
            let mut actual = original.clone();
            // Step=1.25: only residues 0 and 4 have a remainder below 0.5.
            transform(&mut channel, &mut actual, 20.0, 0.0, 1);
            assert_eq!(actual, masked(&original, |bin| matches!(bin % 5, 0 | 4)));
            transform(&mut channel, &mut actual, 100.0, 0.0, 1);
            assert_eq!(actual, masked(&original, |bin| bin == 0));
        }
    }

    #[test]
    fn factor_bypass_includes_the_threshold_and_freezes_mask_clock_and_rng() {
        let original = spectrum(512);
        let mut channel = SpectralLofiChannel::new(1);
        channel.keep_mask.fill(false);
        channel.hop_counter = 2;
        for factor in [0.0, 0.0000999, 0.0001] {
            let mut actual = original.clone();
            transform(&mut channel, &mut actual, factor, 100.0, 3);
            assert_eq!(actual, original);
            assert_eq!(channel.hop_counter, 2);
            assert_eq!(channel.rng_state, 1);
            assert!(channel.keep_mask.iter().all(|keep| !keep));
        }
        let mut actual = original.clone();
        transform(&mut channel, &mut actual, 0.00010000001, 0.0, 3);
        assert_eq!(channel.hop_counter, 0);
        assert_ne!(channel.rng_state, 1);
        assert_eq!(actual[0], original[0]);
        assert_eq!(actual[1], Complex::default());
    }

    #[test]
    fn slow_refreshes_on_the_nth_active_hop_and_holds_the_previous_mask() {
        let original = spectrum(512);
        for slow in [1, 4, 100] {
            let mut channel = SpectralLofiChannel::new(1);
            for hop in 1..=3 * slow {
                let factor = if hop <= slow { 50.0 } else { 75.0 };
                let previous_rng = channel.rng_state;
                let mut actual = original.clone();
                transform(&mut channel, &mut actual, factor, 0.0, slow);
                let expected = if hop < slow {
                    original.clone()
                } else if hop < 2 * slow {
                    masked(&original, |bin| bin % 2 == 0)
                } else {
                    masked(&original, |bin| bin % 4 == 0)
                };
                assert_eq!(actual, expected, "Slow {slow}, hop {hop}");
                assert_eq!(channel.hop_counter, (hop % slow) as u32);
                if hop % slow != 0 {
                    assert_eq!(channel.rng_state, previous_rng);
                } else {
                    assert_ne!(channel.rng_state, previous_rng);
                }
            }
        }
    }

    #[test]
    fn xorshift_matches_known_vectors_and_zero_seed_is_repaired() {
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
        assert_eq!(SpectralLofiChannel::new(0).rng_state, 1);
    }

    #[test]
    fn random_masks_match_known_votes_and_the_midpoint_requires_both_votes() {
        let random_head = [
            false, false, true, false, true, false, false, false, false, true, true, false,
        ];
        // Fixed xorshift seed 1: totals and final state include DC and Nyquist.
        for (size, random_kept, both_kept, final_state) in [
            (512, 125, 60, 0xF980_7A79),
            (1_024, 248, 125, 0x4DF7_F391),
            (2_048, 494, 245, 0x61AE_4B59),
            (4_096, 994, 502, 0x1E43_CF97),
        ] {
            let original = spectrum(size);
            for random in [0.0, 49.999, 50.0, 50.001, 100.0] {
                let mut channel = SpectralLofiChannel::new(1);
                let mut actual = original.clone();
                transform(&mut channel, &mut actual, 50.0, random, 1);
                let kept = actual[..=size / 2]
                    .iter()
                    .filter(|value| **value != Complex::default())
                    .count();
                let expected = if random < 50.0 {
                    size / 4 + 1
                } else if random == 50.0 {
                    both_kept
                } else {
                    random_kept
                };
                assert_eq!(kept, expected, "FFT {size}, Random {random}");
                assert_eq!(channel.rng_state, final_state);
                for (bin, &vote) in random_head.iter().enumerate() {
                    let regular = bin % 2 == 0;
                    let keep = if random < 50.0 {
                        regular
                    } else if random == 50.0 {
                        regular && vote
                    } else {
                        vote
                    };
                    assert_eq!(
                        actual[bin],
                        if keep {
                            original[bin]
                        } else {
                            Complex::default()
                        }
                    );
                }
                for bin in 1..size / 2 {
                    assert_eq!(actual[size - bin], actual[bin].conj());
                }
            }
        }
    }

    #[test]
    fn random_votes_drop_bins_at_exact_factor_thresholds() {
        for (seed, factor, next_state) in [
            (0x8800_4000, 50.0, 0x8000_0000),
            (0x5E6C_FCE7, 100.0, 0xFFFF_FFFF),
        ] {
            assert_eq!(xorshift(seed), next_state);
            let mut channel = SpectralLofiChannel::new(seed);
            let mut actual = spectrum(512);
            transform(&mut channel, &mut actual, factor, 100.0, 1);
            assert_eq!(actual[0], Complex::default());
            if factor == 100.0 {
                assert!(actual.iter().all(|value| *value == Complex::default()));
            }
        }
    }

    #[test]
    fn reset_restores_initial_mask_and_clock_without_rewinding_rng_or_storage() {
        let mut effect = SpectralLofiEffect::default();
        assert_ne!(effect.chan_l.rng_state, effect.chan_r.rng_state);
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert!(channel.keep_mask.len() >= 2_049);
            let storage = channel.keep_mask.as_ptr();
            let length = channel.keep_mask.len();
            let capacity = channel.keep_mask.capacity();
            for size in [512, 4_096, 1_024, 2_048, 4_096] {
                let mut bins = spectrum(size);
                transform(channel, &mut bins, 50.0, 100.0, 1);
                assert!(channel.keep_mask[..=size / 2].iter().any(|keep| !keep));
                let rng = channel.rng_state;
                channel.hop_counter = 2;
                channel.reset();
                assert_eq!(channel.hop_counter, 0);
                assert_eq!(channel.rng_state, rng);
                let mut restored = spectrum(size);
                transform(channel, &mut restored, 50.0, 100.0, 3);
                assert_eq!(restored, spectrum(size));
                assert_eq!(channel.rng_state, rng);
                assert_eq!(channel.keep_mask.as_ptr(), storage);
                assert_eq!(channel.keep_mask.len(), length);
                assert_eq!(channel.keep_mask.capacity(), capacity);
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralLofiEffect::default();
        assert_eq!(
            (
                effect.params.factor_pct,
                effect.params.randomise_pct,
                effect.params.slow,
                effect.params.fft_param
            ),
            (0.0, 0.0, 1, 2.0)
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
                ("Factor", 0.0, 100.0, 0.0),
                ("Random", 0.0, 100.0, 0.0),
                ("Slow", 1.0, 100.0, 1.0),
                ("FFT", 0.0, 3.0, 2.0)
            ]
        );
        assert!(
            specs
                .iter()
                .all(|s| matches!(s.scaling, ParamScaling::Linear))
        );
        for spec in &specs[..2] {
            assert!(matches!(
                spec.format,
                ParamFormat::Number {
                    decimals: 0,
                    unit: " %"
                }
            ));
        }
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number {
                decimals: 0,
                unit: " hops"
            }
        ));
        assert!(
            matches!(specs[3].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for slot in [0, 1] {
            for (value, expected) in [(-999.0, 0.0), (37.5, 37.5), (999.0, 100.0)] {
                effect.set_param(slot, value);
                assert_eq!(
                    [effect.params.factor_pct, effect.params.randomise_pct][slot],
                    expected
                );
            }
        }
        for (value, expected) in [(-99.0, 1), (1.49, 1), (1.5, 2), (3.49, 3), (999.0, 100)] {
            effect.set_param(2, value);
            assert_eq!(effect.params.slow, expected);
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
                effect.params.factor_pct,
                effect.params.randomise_pct,
                effect.params.slow
            ),
            (100.0, 100.0, 100)
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_even_bin_mask_reference() {
        struct EvenBins;
        impl SpectralTransform for EvenBins {
            fn transform(&mut self, bins: &mut [Complex<f32>], size: usize, _: f32) {
                for bin in (1..size).step_by(2) {
                    bins[bin] = Complex::default();
                }
            }
        }
        let mut effect = configured_lofi(0.0);
        effect.set_param(2, 1.0);
        effect.set_sample_rate(32_000.0);
        let mut left = SpectralEngine::new(32_000.0);
        let mut right = SpectralEngine::new(32_000.0);
        left.set_fft_size(512);
        right.set_fft_size(512);
        let mut energy = [0.0; 2];
        for sample in 0..8_192 {
            let input = (
                (sample as f32 * 0.17).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            let expected = (
                left.process_sample(input.0, &mut EvenBins),
                right.process_sample(input.1, &mut EvenBins),
            );
            let actual = effect.process_sample(input.0, input.1);
            assert_eq!(actual, expected, "sample {sample}");
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
    }

    #[test]
    fn host_reset_clears_stereo_history_and_preserves_pending_fft() {
        let mut used = configured_lofi(0.0);
        let mut energy = [0.0; 2];
        for sample in 0..4_096 {
            let output = used.process_sample(
                (sample as f32 * 0.23).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            energy[0] += output.0 * output.0;
            energy[1] += output.1 * output.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
        used.set_param(3, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_lofi(0.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(3, 1.0);
        fresh.reset();
        for sample in 0..8_192 {
            let input = (
                (sample as f32 * 0.11).cos() * 0.125,
                (sample as f32 * 0.27).sin() * 0.25,
            );
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn sample_rate_configuration_and_host_reset_replay_a_fresh_selected_fft() {
        for index in 0..4 {
            let mut used = configured_lofi(index as f32);
            let mut energy = [0.0; 2];
            for sample in 0..12_288 {
                let output = used.process_sample(
                    (sample as f32 * 0.19).sin() * 0.25,
                    (sample as f32 * 0.27).cos() * 0.125,
                );
                energy[0] += output.0 * output.0;
                energy[1] += output.1 * output.1;
            }
            assert!(energy.into_iter().all(|e| e > 0.01));
            used.set_sample_rate(32_000.0);
            used.reset();
            let mut fresh = configured_lofi(index as f32);
            fresh.set_sample_rate(32_000.0);
            fresh.reset();
            assert_eq!(used.sample_rate, 32_000.0);
            assert_eq!(used.latency_samples(), [512, 1_024, 2_048, 4_096][index]);
            assert_eq!(used.engine_r.latency_samples(), used.latency_samples());
            for sample in 0..12_288 {
                let input = (
                    (sample as f32 * 0.29).cos() * 0.25,
                    (sample as f32 * 0.13).sin() * 0.125,
                );
                assert_eq!(
                    used.process_sample(input.0, input.1),
                    fresh.process_sample(input.0, input.1)
                );
            }
        }
    }

    fn drive(e: &mut SpectralLofiEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralLofiEffect::default().parameters().len(), 4);
    }

    #[test]
    fn factor_zero_is_passthrough() {
        let mut e = SpectralLofiEffect::default();
        e.set_param(3, 1.0); // FFT = 1024 (slot 3)
        e.set_param(0, 0.0); // Factor = 0 -> return early (slot 0)
        let f = 1000.0;
        let sr = 48_000.0;
        let n = 4096;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralLofiEffect::default();
        e.set_param(0, 50.0); // arbitrary Factor (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn xorshift_never_returns_zero() {
        // Sanity check on the RNG -- a zero state would lock xorshift forever.
        let mut s = 1u32;
        for _ in 0..1000 {
            s = xorshift(s);
            assert!(s != 0);
        }
    }
}
