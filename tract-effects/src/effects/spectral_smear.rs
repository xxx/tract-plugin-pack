//! Spectral Smear: per-bin magnitude envelope hold + Chaos phase randomisation.
//!
//! Each bin runs through a one-pole envelope follower: instant attack
//! (mag_out = max(mag_in, mag_held_decayed)) so transients punch through, but
//! a release tau equal to `Length` so the magnitude lingers after the source
//! drops. The decay coefficient is computed per hop -- one analysis hop is
//! `fft_size / 2` samples, so decay = exp(-hop_samples / length_samples). Chaos
//! adds a random rotation to each bin's phase, breaking inter-bin coherence
//! the longer the smear runs.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

/// Maximum half-spectrum entries we'll ever need to hold -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// xorshift32 -- audio-thread-safe (no allocation, no transcendentals).
/// Returns the next state; the caller threads it through.
fn xorshift(mut s: u32) -> u32 {
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    s.max(1)
}

struct SpectralSmearChannel {
    last_mag: Vec<f32>,
    rng_state: u32,
}

impl SpectralSmearChannel {
    fn new(seed: u32) -> Self {
        Self {
            last_mag: vec![0.0; MAX_HALF_PLUS_ONE],
            // Seed must be non-zero for xorshift to do anything useful.
            rng_state: seed.max(1),
        }
    }

    fn reset(&mut self) {
        self.last_mag.fill(0.0);
        // Don't reset rng_state -- otherwise repeated resets produce
        // identical phase rotations.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    length_ms: f32,
    chaos_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            length_ms: 200.0,
            chaos_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralSmearChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let hop_samples = (fft_size / 2) as f32;
        let length_samples = (self.params.length_ms * 0.001 * sample_rate).max(1.0);
        // Decay per hop = exp(-hop / length).
        let decay = (-hop_samples / length_samples).exp();
        let chaos = (self.params.chaos_pct * 0.01).clamp(0.0, 1.0);

        for k in 0..=half {
            let mag_in = spectrum[k].norm();
            let mag_held = self.chan.last_mag[k] * decay;
            let mag = mag_in.max(mag_held);
            self.chan.last_mag[k] = mag;

            // Reconstruct bin: original phase + Chaos-amount random rotation.
            let phase_in = spectrum[k].im.atan2(spectrum[k].re);
            self.chan.rng_state = xorshift(self.chan.rng_state);
            let r = (self.chan.rng_state as f32) / (u32::MAX as f32) - 0.5; // -0.5..0.5
            let phase = phase_in + chaos * std::f32::consts::TAU * r;
            spectrum[k] = Complex::new(mag * phase.cos(), mag * phase.sin());
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralSmearEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralSmearChannel,
    chan_r: SpectralSmearChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralSmearEffect {
    pub const PARAMS: [ParamSpec; 3] = [
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
            name: "Chaos",
            min: 0.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
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

impl Default for SpectralSmearEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        // Seed L and R with different non-zero values so their phase
        // rotations are independent.
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralSmearChannel::new(0xC0FF_EE01),
            chan_r: SpectralSmearChannel::new(0xC0FF_EE02),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralSmearEffect {
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
            1 => self.params.chaos_pct = value.clamp(0.0, 100.0),
            2 => {
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

    fn spectrum_with_magnitude(size: usize, magnitude: f32) -> Vec<Complex<f32>> {
        let half = size / 2;
        let mut spectrum = vec![Complex::default(); size];
        spectrum[0] = Complex::new(magnitude, 0.0);
        spectrum[half] = Complex::new(-magnitude, 0.0);
        for bin in 1..half {
            spectrum[bin] = Complex::new(0.6 * magnitude, -0.8 * magnitude);
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn transform_with(
        channel: &mut SpectralSmearChannel,
        spectrum: &mut [Complex<f32>],
        rate: f32,
        length: f32,
        chaos: f32,
    ) {
        let size = spectrum.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                fft_param: 2.0,
                length_ms: length,
                chaos_pct: chaos,
            },
        }
        .transform(spectrum, size, rate);
    }

    fn assert_close(actual: f32, expected: f64) {
        assert!(
            (actual as f64 - expected).abs() < 2e-6 + expected.abs() * 3e-5,
            "{actual} vs {expected}"
        );
    }

    fn configured_smear(fft_index: f32, chaos: f32) -> SpectralSmearEffect {
        let mut effect = SpectralSmearEffect::default();
        for (index, value) in [137.5, chaos, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn xorshift_matches_fixed_reference_words_and_zero_seed_recovery() {
        let mut state = 1;
        for expected in [
            0x0004_2021,
            0x0408_0601,
            0x9dcc_a8c5,
            0x1255_994f,
            0x8ef9_17d1,
            0x2c6f_5bd0,
            0x25b2_331a,
            0x19f9_1cb2,
        ] {
            state = xorshift(state);
            assert_eq!(state, expected);
        }
        assert_eq!(xorshift(0), 1);
        assert_eq!(SpectralSmearChannel::new(0).rng_state, 1);
    }

    #[test]
    fn instant_attack_and_held_release_match_closed_form_hop_decay() {
        for size in [512, 1_024, 2_048, 4_096] {
            for rate in [8_000.0, 48_000.0, 192_000.0] {
                for length in [10.0, 200.0, 2_000.0] {
                    let mut channel = SpectralSmearChannel::new(1);
                    let mut attack = spectrum_with_magnitude(size, 5.0);
                    transform_with(&mut channel, &mut attack, rate, length, 0.0);
                    let step = size as f64 / (2.0 * length as f64 * 0.001 * rate as f64);
                    let mut weaker = spectrum_with_magnitude(size, 1.0);
                    transform_with(&mut channel, &mut weaker, rate, length, 0.0);
                    let held = (5.0 * (-step).exp()).max(1.0);
                    for bin in [0, 1, size / 3, size / 2] {
                        assert_close(channel.last_mag[bin], held);
                        let expected = if bin == 0 {
                            (held, 0.0)
                        } else if bin == size / 2 {
                            (-held, 0.0)
                        } else {
                            (0.6 * held, -0.8 * held)
                        };
                        assert_close(weaker[bin].re, expected.0);
                        assert_close(weaker[bin].im, expected.1);
                    }
                    for hop in 1..=4 {
                        let mut silence = vec![Complex::default(); size];
                        transform_with(&mut channel, &mut silence, rate, length, 0.0);
                        let expected = held * (-(hop as f64) * step).exp();
                        for bin in [0, 1, size / 3, size / 2] {
                            assert_close(channel.last_mag[bin], expected);
                            // Silent input has phase zero, even when an earlier frame did not.
                            assert_close(silence[bin].re, expected);
                            assert_eq!(silence[bin].im, 0.0);
                        }
                    }
                    let mut stronger = spectrum_with_magnitude(size, 13.0);
                    transform_with(&mut channel, &mut stronger, rate, length, 0.0);
                    assert_close(channel.last_mag[1], 13.0);
                    assert_close(stronger[1].re, 7.8);
                    assert_close(stronger[1].im, -10.4);
                }
            }
        }
    }

    #[test]
    fn chaos_matches_golden_rng_rotations_and_preserves_magnitudes_and_conjugates() {
        for (seed, words, final_state) in [
            (
                0xc0ff_ee01,
                [
                    0x9ac5_c35e,
                    0x776c_5329,
                    0x5c28_b80d,
                    0x2e0e_8eb9,
                    0x04e3_fbf5,
                    0x0831_a15b,
                    0xbf41_3596,
                    0xa784_948f,
                ],
                0x1e0c_005a,
            ),
            (
                0xc0ff_ee02,
                [
                    0x9ac9_a33d,
                    0x7b64_d92a,
                    0xe87c_c84a,
                    0x5fc3_aef0,
                    0x69aa_c43e,
                    0x148e_89a8,
                    0x72c6_85d5,
                    0xedf2_e98b,
                ],
                0x562e_ecd5,
            ),
        ] {
            for chaos in [0.0, 25.0, 100.0] {
                let size = 512;
                let mut source = spectrum_with_magnitude(size, 5.0);
                for bin in (2..size / 2).step_by(2) {
                    source[bin] = Complex::new(-3.0, 4.0);
                    source[size - bin] = source[bin].conj();
                }
                let mut channel = SpectralSmearChannel::new(seed);
                let mut actual = source.clone();
                transform_with(&mut channel, &mut actual, 48_000.0, 200.0, chaos);
                for (bin, word) in words
                    .into_iter()
                    .enumerate()
                    .chain(std::iter::once((size / 2, final_state)))
                {
                    // Multiplying by a rotation is independent of atan2 phase reconstruction.
                    let angle = std::f64::consts::TAU * chaos as f64 / 100.0
                        * (word as f64 / 4_294_967_296.0 - 0.5);
                    let value = source[bin];
                    let expected = (
                        value.re as f64 * angle.cos() - value.im as f64 * angle.sin(),
                        value.re as f64 * angle.sin() + value.im as f64 * angle.cos(),
                    );
                    assert!(
                        (actual[bin].re as f64 - expected.0).abs() < 1e-5,
                        "seed {seed:x}, bin {bin}, Chaos {chaos}"
                    );
                    assert!((actual[bin].im as f64 - expected.1).abs() < 1e-5);
                    assert_close(channel.last_mag[bin], 5.0);
                }
                assert_eq!(channel.rng_state, final_state);
                for bin in 1..size / 2 {
                    assert_eq!(actual[size - bin], actual[bin].conj());
                }
            }
        }
    }

    #[test]
    fn random_clock_advances_for_every_nonnegative_bin_even_on_silence_and_zero_chaos() {
        for (size, expected) in [
            (512, 0xf980_7a79),
            (1_024, 0x4df7_f391),
            (2_048, 0x61ae_4b59),
            (4_096, 0x1e43_cf97),
        ] {
            let mut channel = SpectralSmearChannel::new(1);
            let mut silence = vec![Complex::default(); size];
            transform_with(&mut channel, &mut silence, 48_000.0, 200.0, 0.0);
            assert_eq!(channel.rng_state, expected);
            assert!(silence.iter().all(|value| *value == Complex::default()));
        }
    }

    #[test]
    fn reset_clears_all_held_bins_preserves_rng_and_reuses_preallocated_storage() {
        let mut effect = SpectralSmearEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert!(channel.last_mag.len() >= 2_049);
            let length = channel.last_mag.len();
            let capacity = channel.last_mag.capacity();
            let storage = channel.last_mag.as_ptr();
            for size in [512, 4_096, 1_024, 2_048] {
                channel.last_mag.fill(5.0);
                let mut charged = spectrum_with_magnitude(size, 5.0);
                transform_with(channel, &mut charged, 48_000.0, 137.5, 37.5);
                let rng = channel.rng_state;
                channel.reset();
                assert_eq!(channel.rng_state, rng);
                let mut fresh = SpectralSmearChannel::new(rng);
                let mut actual = spectrum_with_magnitude(size, 1.0);
                let mut expected = actual.clone();
                transform_with(channel, &mut actual, 48_000.0, 137.5, 37.5);
                transform_with(&mut fresh, &mut expected, 48_000.0, 137.5, 37.5);
                assert_eq!(actual, expected);
                assert_eq!(channel.last_mag, fresh.last_mag);
                assert_eq!(channel.rng_state, fresh.rng_state);
                assert_eq!(channel.last_mag.len(), length);
                assert_eq!(channel.last_mag.capacity(), capacity);
                assert_eq!(channel.last_mag.as_ptr(), storage);
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralSmearEffect::default();
        assert_eq!(
            [
                effect.params.length_ms,
                effect.params.chaos_pct,
                effect.params.fft_param
            ],
            [200.0, 0.0, 2.0]
        );
        assert_eq!(effect.chan_l.rng_state, 0xc0ff_ee01);
        assert_eq!(effect.chan_r.rng_state, 0xc0ff_ee02);
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Length", 10.0, 2_000.0, 200.0),
                ("Chaos", 0.0, 100.0, 0.0),
                ("FFT", 0.0, 3.0, 2.0),
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(
            specs[1..]
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
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
        assert!(
            matches!(specs[2].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, value, expected) in [
            (0, -99.0, 10.0),
            (0, 137.5, 137.5),
            (0, 9_999.0, 2_000.0),
            (1, -99.0, 0.0),
            (1, 37.5, 37.5),
            (1, 999.0, 100.0),
        ] {
            effect.set_param(slot, value);
            assert_eq!(
                [effect.params.length_ms, effect.params.chaos_pct][slot],
                expected
            );
        }
        for (value, size) in [
            (-99.0, 512),
            (0.49, 512),
            (0.5, 1_024),
            (1.5, 2_048),
            (99.0, 4_096),
        ] {
            effect.set_param(2, value);
            assert_eq!(effect.latency_samples(), size);
            assert_eq!(effect.engine_r.latency_samples(), size);
        }
        effect.set_param(99, f32::INFINITY);
        assert_eq!(
            [effect.params.length_ms, effect.params.chaos_pct],
            [2_000.0, 100.0]
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_f64_envelope_and_phase_reference() {
        struct Reference {
            held: Vec<f64>,
        }
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, rate: f32) {
                let decay = (-(size as f64) / (2.0 * 137.5 * 0.001 * rate as f64)).exp();
                for bin in 0..=size / 2 {
                    let value = spectrum[bin];
                    let magnitude = (value.re as f64)
                        .hypot(value.im as f64)
                        .max(self.held[bin] * decay);
                    self.held[bin] = magnitude;
                    let angle = (value.im as f64).atan2(value.re as f64);
                    spectrum[bin] = Complex::new(
                        (magnitude * angle.cos()) as f32,
                        (magnitude * angle.sin()) as f32,
                    );
                    if bin > 0 && bin < size / 2 {
                        spectrum[size - bin] = spectrum[bin].conj();
                    }
                }
            }
        }
        let mut effect = configured_smear(0.0, 0.0);
        effect.set_sample_rate(32_000.0);
        let mut reference_l = SpectralEngine::new(32_000.0);
        let mut reference_r = SpectralEngine::new(32_000.0);
        reference_l.set_fft_size(512);
        reference_r.set_fft_size(512);
        let mut context_l = Reference {
            held: vec![0.0; 2_049],
        };
        let mut context_r = Reference {
            held: vec![0.0; 2_049],
        };
        let mut energy = [0.0; 2];
        for sample in 0..8_192 {
            let input = if sample < 4_096 {
                (
                    (sample as f32 * 0.17).sin() * 0.25,
                    (sample as f32 * 0.31).cos() * 0.125,
                )
            } else {
                (0.0, 0.0)
            };
            let expected = (
                reference_l.process_sample(input.0, &mut context_l),
                reference_r.process_sample(input.1, &mut context_r),
            );
            let actual = effect.process_sample(input.0, input.1);
            assert!(
                (actual.0 - expected.0).abs() < 5e-6 && (actual.1 - expected.1).abs() < 5e-6,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_replays_fresh_audio_with_matching_rng_progression_and_pending_fft() {
        let mut used = configured_smear(0.0, 37.5);
        let mut energy = [0.0; 2];
        for sample in 0..4_096 {
            let output = used.process_sample(
                (sample as f32 * 0.23).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            energy[0] += output.0 * output.0;
            energy[1] += output.1 * output.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
        let rng = [used.chan_l.rng_state, used.chan_r.rng_state];
        used.set_param(2, 1.0);
        used.reset();
        assert_eq!([used.chan_l.rng_state, used.chan_r.rng_state], rng);
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_smear(0.0, 37.5);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.chan_l.rng_state = rng[0];
        fresh.chan_r.rng_state = rng[1];
        fresh.set_param(2, 1.0);
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
    fn sample_rate_configuration_and_host_reset_preserve_fft_and_random_stream_intent() {
        for index in 0..4 {
            let mut used = configured_smear(index as f32, 25.0);
            let mut energy = [0.0; 2];
            for sample in 0..12_288 {
                let output = used.process_sample(
                    (sample as f32 * 0.19).sin() * 0.25,
                    (sample as f32 * 0.27).cos() * 0.125,
                );
                energy[0] += output.0 * output.0;
                energy[1] += output.1 * output.1;
            }
            assert!(energy.into_iter().all(|energy| energy > 0.01));
            let rng = [used.chan_l.rng_state, used.chan_r.rng_state];
            used.set_sample_rate(32_000.0);
            used.reset();
            let mut fresh = configured_smear(index as f32, 25.0);
            fresh.set_sample_rate(32_000.0);
            fresh.chan_l.rng_state = rng[0];
            fresh.chan_r.rng_state = rng[1];
            fresh.reset();
            assert_eq!([used.chan_l.rng_state, used.chan_r.rng_state], rng);
            assert_eq!(used.sample_rate, 32_000.0);
            assert_eq!(used.latency_samples(), [512, 1_024, 2_048, 4_096][index]);
            assert_eq!(used.engine_r.latency_samples(), used.latency_samples());
            for sample in 0..12_288 {
                let input = (
                    (sample as f32 * 0.29).cos() * 0.25,
                    (sample as f32 * 0.41).sin() * 0.125,
                );
                assert_eq!(
                    used.process_sample(input.0, input.1),
                    fresh.process_sample(input.0, input.1)
                );
            }
        }
    }

    fn drive(e: &mut SpectralSmearEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralSmearEffect::default().parameters().len(), 3);
    }

    #[test]
    fn silence_in_silence_out() {
        // Note: smear holds magnitudes; for a clean test the smear state must
        // start zeroed. Default state is zeroed.
        let mut e = SpectralSmearEffect::default();
        e.set_param(0, 500.0); // Length = 500 ms (slot 0)
        let out = drive(&mut e, 8192, |_| 0.0);
        // After warm-up, with no input there should be no held magnitude.
        let tail = &out[2 * e.latency_samples()..];
        assert!(tail.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn smear_tail_extends_past_input() {
        // Drive ~50 ms of noise, then silence. With Length=500 ms, the smear
        // tail should still produce non-zero output 200 ms after the source
        // drops to silence -- but the energy should be lower than the source.
        let sr = 48_000.0_f32;
        let mut e = SpectralSmearEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 500.0); // Length = 500 ms (slot 0)
        let burst_samples = (0.050 * sr) as usize;
        let tail_samples = (0.200 * sr) as usize;
        // Pre-generate noise into a buffer so the `drive` closure stays Fn.
        let mut rng = 1u32;
        let noise: Vec<f32> = (0..burst_samples)
            .map(|_| {
                rng = xorshift(rng);
                (rng as f32) / (u32::MAX as f32) - 0.5
            })
            .collect();
        let out = drive(&mut e, burst_samples + tail_samples, |i| {
            if i < burst_samples { noise[i] } else { 0.0 }
        });
        // Burst-region energy (input is ~ N * (1/12) for uniform [-0.5, 0.5]).
        let burst_e: f32 = out[..burst_samples].iter().map(|x| x * x).sum();
        // Tail-region energy after the burst.
        let tail_e: f32 = out[burst_samples + 2 * e.latency_samples()..]
            .iter()
            .map(|x| x * x)
            .sum();
        assert!(
            tail_e > 0.001,
            "smear tail should hold energy past source drop; got {tail_e}"
        );
        // Sanity: tail shouldn't exceed the burst (energy decays).
        assert!(
            tail_e < burst_e * 2.0,
            "tail {tail_e} unexpectedly larger than burst {burst_e}"
        );
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
