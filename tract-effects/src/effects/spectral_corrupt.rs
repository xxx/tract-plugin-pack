//! Spectral Corrupt: zero the quietest or loudest fraction of bins.
//!
//! Amount > 0 zeros the |Amount|% quietest bins (creative noise-gate).
//! Amount < 0 zeros the loudest (inverted -- emphasises noise floor).
//! Amount == 0 is passthrough (modulo Decay carry).
//!
//! Decay is a one-pole carry of last frame's per-bin gain toward the new
//! target. Decay=0 -> instant gate switch (clicks/glitch). The carry is capped
//! at 0.99, so even Decay=100% continues moving toward the new target.
//! Higher values produce a slower fade.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

/// Maximum half-spectrum entries we'll ever need to hold -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralCorruptChannel {
    /// Scratch for the sort -- indices 0..count of the active spectrum.
    bin_indices: Vec<u16>,
    /// Per-bin gain memory -- the previous frame's applied gain. Decay
    /// lerps toward the new target.
    bin_gains: Vec<f32>,
    /// Pre-allocated target buffer (write 1.0/0.0 per bin, then apply).
    target_buf: Vec<f32>,
}

impl SpectralCorruptChannel {
    fn new() -> Self {
        Self {
            bin_indices: vec![0u16; MAX_HALF_PLUS_ONE],
            // Start at gain=1.0 so a fresh effect with Amount=0 is exact passthrough.
            bin_gains: vec![1.0; MAX_HALF_PLUS_ONE],
            target_buf: vec![1.0; MAX_HALF_PLUS_ONE],
        }
    }

    fn reset(&mut self) {
        self.bin_gains.fill(1.0);
        self.target_buf.fill(1.0);
        // Don't bother re-zeroing bin_indices -- they're overwritten per frame.
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
    decay_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
            decay_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralCorruptChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let count = half + 1;
        let amt = self.params.amount_pct * 0.01;
        let d = (self.params.decay_pct * 0.01).clamp(0.0, 0.99);

        if amt.abs() < 1e-3 {
            // No gating this frame; just decay any held gain back toward 1.0.
            for k in 0..count {
                self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + (1.0 - d);
                spectrum[k].re *= self.chan.bin_gains[k];
                spectrum[k].im *= self.chan.bin_gains[k];
                if k != 0 && k != half {
                    spectrum[fft_size - k] = spectrum[k].conj();
                }
            }
            return;
        }

        // Rank bins by magnitude using the pre-allocated index scratch.
        for (k, index) in self.chan.bin_indices[..count].iter_mut().enumerate() {
            *index = k as u16;
        }
        self.chan.bin_indices[..count].sort_unstable_by(|&a, &b| {
            let ma = spectrum[a as usize].norm();
            let mb = spectrum[b as usize].norm();
            ma.partial_cmp(&mb).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Build the target_buf: 0 for bins in the zero-set, 1 for the rest.
        let cut = ((amt.abs() * count as f32).round() as usize).min(count);
        self.chan.target_buf[..count].fill(1.0);
        let zero_range = if amt > 0.0 {
            // Quietest |amt|*count are zeroed.
            &self.chan.bin_indices[..cut]
        } else {
            // Loudest |amt|*count are zeroed.
            &self.chan.bin_indices[count - cut..count]
        };
        for &k in zero_range {
            self.chan.target_buf[k as usize] = 0.0;
        }

        // Apply: per-bin gain = lerp(last, target, 1 - d).
        for k in 0..count {
            let t = self.chan.target_buf[k];
            self.chan.bin_gains[k] = self.chan.bin_gains[k] * d + t * (1.0 - d);
            spectrum[k].re *= self.chan.bin_gains[k];
            spectrum[k].im *= self.chan.bin_gains[k];
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralCorruptEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralCorruptChannel,
    chan_r: SpectralCorruptChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralCorruptEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Amount",
            min: -100.0,
            max: 100.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        ParamSpec {
            name: "Decay",
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

impl Default for SpectralCorruptEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralCorruptChannel::new(),
            chan_r: SpectralCorruptChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralCorruptEffect {
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
            0 => self.params.amount_pct = value.clamp(-100.0, 100.0),
            1 => self.params.decay_pct = value.clamp(0.0, 100.0),
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

    fn known_rank(bin: usize, count: usize) -> usize {
        (2 * bin + 17) % count
    }

    fn ranked_spectrum(size: usize) -> Vec<Complex<f32>> {
        let half = size / 2;
        let count = half + 1;
        let mut spectrum = vec![Complex::default(); size];
        for bin in 0..=half {
            let unit = (known_rank(bin, count) + 1) as f32 * 0.0625;
            spectrum[bin] = if bin == 0 {
                Complex::new(-5.0 * unit, 0.0)
            } else if bin == half {
                Complex::new(5.0 * unit, 0.0)
            } else {
                Complex::new(3.0 * unit, -4.0 * unit)
            };
            if bin > 0 && bin < half {
                spectrum[size - bin] = spectrum[bin].conj();
            }
        }
        spectrum
    }

    fn mask_reference(source: &[Complex<f32>], cut: usize, quiet: bool) -> Vec<Complex<f32>> {
        let size = source.len();
        let count = size / 2 + 1;
        source
            .iter()
            .enumerate()
            .map(|(index, &value)| {
                let rank = known_rank(index.min(size - index), count);
                let gated = if quiet {
                    rank < cut
                } else {
                    rank >= count - cut
                };
                if gated { Complex::default() } else { value }
            })
            .collect()
    }

    fn transform_with(
        channel: &mut SpectralCorruptChannel,
        spectrum: &mut [Complex<f32>],
        amount: f32,
        decay: f32,
    ) {
        let size = spectrum.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                fft_param: 2.0,
                amount_pct: amount,
                decay_pct: decay,
            },
        }
        .transform(spectrum, size, 48_000.0);
    }

    fn assert_close(actual: f32, expected: f64) {
        assert!(
            (actual as f64 - expected).abs() < 2e-6 + expected.abs() * 3e-5,
            "{actual} vs {expected}"
        );
    }

    fn configured_corrupt(fft_index: f32) -> SpectralCorruptEffect {
        let mut effect = SpectralCorruptEffect::default();
        for (index, value) in [37.5, 50.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn signed_amounts_gate_known_quietest_or_loudest_ranks_at_every_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            let count = size / 2 + 1;
            let original = ranked_spectrum(size);
            let mut channel = SpectralCorruptChannel::new();
            for (amount, cut) in [(25.0, count / 4), (50.0, count.div_ceil(2)), (100.0, count)] {
                for sign in [1.0, -1.0] {
                    let mut actual = original.clone();
                    transform_with(&mut channel, &mut actual, sign * amount, 0.0);
                    assert_eq!(
                        actual,
                        mask_reference(&original, cut, sign > 0.0),
                        "FFT {size}, Amount {}",
                        sign * amount
                    );
                }
            }
        }
    }

    #[test]
    fn amount_rounding_and_deadband_keep_the_exact_threshold_active() {
        let size = 4_096;
        let original = ranked_spectrum(size);
        for amount in [-0.0999, 0.0, 0.0999] {
            let mut channel = SpectralCorruptChannel::new();
            let mut actual = original.clone();
            transform_with(&mut channel, &mut actual, amount, 0.0);
            assert_eq!(actual, original);
        }
        for (amount, cut) in [
            (0.1, 2),
            (-0.1, 2),
            (49.999, 1_024),
            (50.0, 1_025),
            (-49.999, 1_024),
            (-50.0, 1_025),
        ] {
            let mut channel = SpectralCorruptChannel::new();
            let mut actual = original.clone();
            transform_with(&mut channel, &mut actual, amount, 0.0);
            assert_eq!(
                actual,
                mask_reference(&original, cut, amount > 0.0),
                "Amount {amount}"
            );
        }
    }

    #[test]
    fn held_gains_follow_closed_form_fades_and_amount_zero_recovery_with_the_decay_cap() {
        for size in [512, 4_096] {
            let original = ranked_spectrum(size);
            let count = size / 2 + 1;
            let cut = count / 4;
            for (decay_pct, decay) in [(0.0, 0.0_f64), (50.0, 0.5), (99.0, 0.99), (100.0, 0.99)] {
                let mut channel = SpectralCorruptChannel::new();
                for hop in 1..=6 {
                    let mut actual = original.clone();
                    transform_with(&mut channel, &mut actual, 25.0, decay_pct);
                    for bin in 0..count {
                        let gain = if known_rank(bin, count) < cut {
                            decay.powi(hop)
                        } else {
                            1.0
                        };
                        assert_close(channel.bin_gains[bin], gain);
                        assert_close(actual[bin].re, original[bin].re as f64 * gain);
                        assert_close(actual[bin].im, original[bin].im as f64 * gain);
                    }
                    for bin in 1..size / 2 {
                        assert_eq!(actual[size - bin], actual[bin].conj());
                    }
                }
                let start = decay.powi(6);
                for hop in 1..=6 {
                    let mut actual = original.clone();
                    transform_with(&mut channel, &mut actual, 0.0, decay_pct);
                    for bin in 0..count {
                        let gain = if known_rank(bin, count) < cut {
                            1.0 - (1.0 - start) * decay.powi(hop)
                        } else {
                            1.0
                        };
                        assert_close(channel.bin_gains[bin], gain);
                        assert_close(actual[bin].re, original[bin].re as f64 * gain);
                        assert_close(actual[bin].im, original[bin].im as f64 * gain);
                    }
                    for bin in 1..size / 2 {
                        assert_eq!(actual[size - bin], actual[bin].conj());
                    }
                }
            }
        }
    }

    #[test]
    fn scratch_is_overwritten_and_gain_reset_reuses_storage_across_fft_changes() {
        let mut effect = SpectralCorruptEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert!(
                channel.bin_indices.len() >= 2_049
                    && channel.bin_gains.len() >= 2_049
                    && channel.target_buf.len() >= 2_049
            );
            let lengths = (
                channel.bin_indices.len(),
                channel.bin_gains.len(),
                channel.target_buf.len(),
            );
            let capacities = (
                channel.bin_indices.capacity(),
                channel.bin_gains.capacity(),
                channel.target_buf.capacity(),
            );
            let storage = (
                channel.bin_indices.as_ptr(),
                channel.bin_gains.as_ptr(),
                channel.target_buf.as_ptr(),
            );
            for size in [512, 4_096, 1_024, 2_048, 4_096] {
                channel.bin_gains.fill(0.0);
                channel.reset();
                let original = ranked_spectrum(size);
                let count = size / 2 + 1;
                for (amount, cut, quiet) in [
                    (100.0, count, true),
                    (50.0, count.div_ceil(2), true),
                    (-25.0, count / 4, false),
                ] {
                    channel.bin_indices.fill(u16::MAX);
                    channel.target_buf.fill(f32::NAN);
                    let mut actual = original.clone();
                    transform_with(channel, &mut actual, amount, 0.0);
                    assert_eq!(actual, mask_reference(&original, cut, quiet));
                }
                channel.reset();
                channel.target_buf.fill(f32::NAN);
                let mut restored = original.clone();
                transform_with(channel, &mut restored, 0.0, 100.0);
                assert_eq!(restored, original);
                assert_eq!(
                    (
                        channel.bin_indices.len(),
                        channel.bin_gains.len(),
                        channel.target_buf.len()
                    ),
                    lengths
                );
                assert_eq!(
                    (
                        channel.bin_indices.capacity(),
                        channel.bin_gains.capacity(),
                        channel.target_buf.capacity()
                    ),
                    capacities
                );
                assert_eq!(
                    (
                        channel.bin_indices.as_ptr(),
                        channel.bin_gains.as_ptr(),
                        channel.target_buf.as_ptr()
                    ),
                    storage
                );
            }
        }
    }

    #[test]
    fn tied_magnitudes_preserve_gate_cardinality_without_requiring_a_tie_order() {
        let size = 512;
        let mut original = vec![Complex::default(); size];
        original[0] = Complex::new(-1.0, 0.0);
        original[size / 2] = Complex::new(1.0, 0.0);
        for bin in 1..size / 2 {
            original[bin] = [
                Complex::new(1.0, 0.0),
                Complex::new(0.0, 1.0),
                Complex::new(-1.0, 0.0),
                Complex::new(0.0, -1.0),
            ][bin % 4];
            original[size - bin] = original[bin].conj();
        }
        for amount in [50.0, -50.0] {
            let mut channel = SpectralCorruptChannel::new();
            let mut actual = original.clone();
            transform_with(&mut channel, &mut actual, amount, 0.0);
            assert_eq!(
                actual[..=size / 2]
                    .iter()
                    .filter(|value| **value == Complex::default())
                    .count(),
                129
            );
            for bin in 0..=size / 2 {
                assert!(actual[bin] == Complex::default() || actual[bin] == original[bin]);
            }
            for bin in 1..size / 2 {
                assert_eq!(actual[size - bin], actual[bin].conj());
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralCorruptEffect::default();
        assert_eq!(
            [
                effect.params.amount_pct,
                effect.params.decay_pct,
                effect.params.fft_param
            ],
            [0.0, 0.0, 2.0]
        );
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Amount", -100.0, 100.0, 0.0),
                ("Decay", 0.0, 100.0, 0.0),
                ("FFT", 0.0, 3.0, 2.0),
            ]
        );
        assert!(
            specs
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
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
        assert!(
            matches!(specs[2].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, value, expected) in [
            (0, -999.0, -100.0),
            (0, -37.5, -37.5),
            (0, 999.0, 100.0),
            (1, -99.0, 0.0),
            (1, 37.5, 37.5),
            (1, 999.0, 100.0),
        ] {
            effect.set_param(slot, value);
            assert_eq!(
                [effect.params.amount_pct, effect.params.decay_pct][slot],
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
            [effect.params.amount_pct, effect.params.decay_pct],
            [100.0, 100.0]
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_all_cut_and_recovery_gain_reference() {
        struct Reference {
            gain: f64,
            target: f64,
        }
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                // Decay=50%: each hop averages the previous gain and target.
                self.gain = (self.gain + self.target) * 0.5;
                for bin in 0..=size / 2 {
                    spectrum[bin] *= self.gain as f32;
                    if bin > 0 && bin < size / 2 {
                        spectrum[size - bin] = spectrum[bin].conj();
                    }
                }
            }
        }
        let mut effect = configured_corrupt(0.0);
        effect.set_param(0, 100.0);
        effect.set_sample_rate(32_000.0);
        let mut reference_l = SpectralEngine::new(32_000.0);
        let mut reference_r = SpectralEngine::new(32_000.0);
        reference_l.set_fft_size(512);
        reference_r.set_fft_size(512);
        let mut context_l = Reference {
            gain: 1.0,
            target: 0.0,
        };
        let mut context_r = Reference {
            gain: 1.0,
            target: 0.0,
        };
        let mut energy = [0.0; 2];
        for sample in 0..8_192 {
            if sample == 4_096 {
                effect.set_param(0, 0.0);
                context_l.target = 1.0;
                context_r.target = 1.0;
            }
            let input = (
                (sample as f32 * 0.17).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            let expected = (
                reference_l.process_sample(input.0, &mut context_l),
                reference_r.process_sample(input.1, &mut context_r),
            );
            let actual = effect.process_sample(input.0, input.1);
            assert!(
                (actual.0 - expected.0).abs() < 1e-6 && (actual.1 - expected.1).abs() < 1e-6,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_clears_stereo_gate_history_while_preserving_pending_fft() {
        let mut used = configured_corrupt(0.0);
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
        used.set_param(2, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_corrupt(0.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
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
    fn sample_rate_configuration_and_host_reset_replay_a_fresh_selected_fft() {
        for index in 0..4 {
            let mut used = configured_corrupt(index as f32);
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
            used.set_sample_rate(32_000.0);
            used.reset();
            let mut fresh = configured_corrupt(index as f32);
            fresh.set_sample_rate(32_000.0);
            fresh.reset();
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

    fn drive(e: &mut SpectralCorruptEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralCorruptEffect::default().parameters().len(), 3);
    }

    #[test]
    fn amount_zero_with_decay_zero_is_passthrough() {
        let mut e = SpectralCorruptEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.0); // Amount = 0 (slot 0)
        e.set_param(1, 0.0); // Decay = 0 -> bin_gains snap straight to 1.0 (slot 1)
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
        let mut e = SpectralCorruptEffect::default();
        e.set_param(0, 50.0); // arbitrary non-zero Amount (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
