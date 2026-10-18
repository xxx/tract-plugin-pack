//! Spectral Spread: box-blur the magnitude spectrum across bins.
//!
//! Per-bin magnitude is replaced by the mean magnitude within +/- radius bins,
//! where radius scales linearly with Amount (0-100% -> 0-16 bins). Phase
//! preserved per-bin -- output bin is the original complex value rescaled to
//! the new magnitude. Kernel cap (16 bins) keeps Amount=100% as
//! detail-softening rather than spectrum-smashing.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

/// Maximum half-spectrum size we ever need to scratch -- (4096 / 2) + 1.
const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

/// Per-channel mutable state. Pre-allocated to the largest possible
/// half-spectrum size so no allocation ever happens on the audio thread.
struct SpectralSpreadChannel {
    mags: Vec<f32>,
}

impl SpectralSpreadChannel {
    fn new() -> Self {
        Self {
            mags: vec![0.0; MAX_HALF_PLUS_ONE],
        }
    }

    fn reset(&mut self) {
        self.mags.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
        }
    }
}

/// Carrier passed into `SpectralEngine::process_sample`. Borrows the
/// per-channel scratch AND a copy of the params snapshot.
struct TransformCtx<'a> {
    chan: &'a mut SpectralSpreadChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sr: f32) {
        let half = fft_size / 2;
        let radius = ((self.params.amount_pct * 0.01) * 16.0).round() as i32;
        if radius == 0 {
            return;
        }
        // Snapshot magnitudes of the positive half (DC + ... + Nyquist).
        let mags = &mut self.chan.mags[..=half];
        for k in 0..=half {
            mags[k] = spectrum[k].norm();
        }
        // Box-blur: each bin's new magnitude is the mean within +/- radius.
        // Apply the scaling factor (new / old) to the original complex value
        // to preserve phase.
        for k in 0..=half {
            let lo = (k as i32 - radius).max(0) as usize;
            let hi = (k as i32 + radius).min(half as i32) as usize;
            let sum: f32 = mags[lo..=hi].iter().sum();
            let new_mag = sum / (hi - lo + 1) as f32;
            let old_mag = mags[k].max(1e-20);
            let ratio = new_mag / old_mag;
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            if k != 0 && k != half {
                spectrum[fft_size - k] = spectrum[k].conj();
            }
        }
    }
}

pub struct SpectralSpreadEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralSpreadChannel,
    chan_r: SpectralSpreadChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralSpreadEffect {
    pub const PARAMS: [ParamSpec; 2] = [
        ParamSpec {
            name: "Amount",
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

impl Default for SpectralSpreadEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralSpreadChannel::new(),
            chan_r: SpectralSpreadChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralSpreadEffect {
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
            0 => self.params.amount_pct = value.clamp(0.0, 100.0),
            1 => {
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

    fn varied_spectrum(size: usize) -> Vec<Complex<f32>> {
        let half = size / 2;
        let mut spectrum = vec![Complex::default(); size];
        spectrum[0] = Complex::new(-10.0, 0.0);
        spectrum[half] = Complex::new(15.0, 0.0);
        for bin in 1..half {
            let magnitude_unit = (bin % 17 + 1) as f32;
            let real_sign = if bin % 2 == 0 { 1.0 } else { -1.0 };
            let imag_sign = if bin % 4 < 2 { 1.0 } else { -1.0 };
            spectrum[bin] = Complex::new(
                real_sign * 3.0 * magnitude_unit,
                imag_sign * 4.0 * magnitude_unit,
            );
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn blur_reference(source: &[Complex<f32>], radius: usize) -> Vec<Complex<f32>> {
        if radius == 0 {
            return source.to_vec();
        }
        let size = source.len();
        let half = size / 2;
        let magnitudes: Vec<f64> = source[..=half]
            .iter()
            .map(|value| (value.re as f64).hypot(value.im as f64))
            .collect();
        let mut prefix = vec![0.0; half + 2];
        for (bin, &magnitude) in magnitudes.iter().enumerate() {
            prefix[bin + 1] = prefix[bin] + magnitude;
        }
        let mut expected = vec![Complex::default(); size];
        for bin in 0..=half {
            let start = bin.saturating_sub(radius);
            let end = (bin + radius + 1).min(half + 1);
            let mean = (prefix[end] - prefix[start]) / (end - start) as f64;
            let magnitude = magnitudes[bin];
            if magnitude > 0.0 {
                let amplitude = mean * (magnitude / 1e-20).min(1.0);
                let angle = (source[bin].im as f64).atan2(source[bin].re as f64);
                expected[bin] = Complex::new(
                    (amplitude * angle.cos()) as f32,
                    (amplitude * angle.sin()) as f32,
                );
            }
            if bin > 0 && bin < half {
                expected[size - bin] = expected[bin].conj();
            }
        }
        expected
    }

    fn assert_spectrum_close(actual: &[Complex<f32>], expected: &[Complex<f32>], tolerance: f32) {
        for (bin, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual.re - expected.re).abs() < tolerance
                    && (actual.im - expected.im).abs() < tolerance,
                "bin {bin}: {actual:?} vs {expected:?}"
            );
        }
    }

    fn transform_with(
        channel: &mut SpectralSpreadChannel,
        spectrum: &mut [Complex<f32>],
        amount: f32,
    ) {
        let size = spectrum.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                fft_param: 2.0,
                amount_pct: amount,
            },
        }
        .transform(spectrum, size, 48_000.0);
    }

    fn configured_spread(fft_index: f32, amount: f32) -> SpectralSpreadEffect {
        let mut effect = SpectralSpreadEffect::default();
        effect.set_param(0, amount);
        effect.set_param(1, fft_index);
        effect
    }

    #[test]
    fn box_means_preserve_phase_with_clipped_edge_windows_and_rounded_radii() {
        for size in [512, 1_024, 2_048, 4_096] {
            let original = varied_spectrum(size);
            let mut channel = SpectralSpreadChannel::new();
            for (amount, radius) in [
                (0.0, 0),
                (3.124, 0),
                (3.125, 1),
                (9.374, 1),
                (9.375, 2),
                (50.0, 8),
                (96.875, 16),
                (100.0, 16),
            ] {
                let mut actual = original.clone();
                transform_with(&mut channel, &mut actual, amount);
                assert_spectrum_close(&actual, &blur_reference(&original, radius), 2e-5);
                if radius == 0 {
                    assert_eq!(actual, original);
                }
                assert_eq!(actual[0].im, 0.0);
                assert_eq!(actual[size / 2].im, 0.0);
            }
        }
    }

    #[test]
    fn zero_and_tiny_bins_are_finite_and_obey_the_magnitude_floor() {
        let size = 512;
        let mut channel = SpectralSpreadChannel::new();
        let mut sparse = vec![Complex::default(); size];
        sparse[0] = Complex::new(2.0, 0.0);
        sparse[2] = Complex::new(3.0, 4.0);
        sparse[size - 2] = sparse[2].conj();
        transform_with(&mut channel, &mut sparse, 6.25);
        let mut expected = vec![Complex::default(); size];
        expected[0] = Complex::new(1.0, 0.0);
        expected[2] = Complex::new(1.0, 4.0 / 3.0);
        expected[size - 2] = expected[2].conj();
        assert_spectrum_close(&sparse, &expected, 2e-7);
        for bin in [1, 3, 4, size / 2] {
            assert_eq!(sparse[bin], Complex::default());
        }

        let mut tiny = vec![Complex::default(); size];
        for (bin, value) in [(1, 1.0), (2, 1e-21), (3, 1.0)] {
            tiny[bin] = Complex::new(value, 0.0);
            tiny[size - bin] = tiny[bin];
        }
        transform_with(&mut channel, &mut tiny, 6.25);
        assert!((tiny[2].re - 1.0 / 15.0).abs() < 1e-7);
        assert_eq!(tiny[2].im, 0.0);
        assert!(
            tiny.iter()
                .all(|value| value.re.is_finite() && value.im.is_finite())
        );
        let mut silence = vec![Complex::default(); size];
        transform_with(&mut channel, &mut silence, 100.0);
        assert!(silence.iter().all(|value| *value == Complex::default()));
    }

    #[test]
    fn scratch_is_bounded_reused_and_overwritten_after_bypass_and_fft_changes() {
        let mut effect = SpectralSpreadEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            // The documented allocation is exactly the largest supported half-spectrum.
            assert_eq!(channel.mags.len(), 2_049);
            let storage = channel.mags.as_ptr();
            let capacity = channel.mags.capacity();
            for size in [512, 4_096, 1_024, 2_048, 512] {
                channel.mags.fill(f32::NAN);
                let original = varied_spectrum(size);
                let mut actual = original.clone();
                transform_with(channel, &mut actual, 0.0);
                assert_eq!(actual, original);
                transform_with(channel, &mut actual, 100.0);
                assert_spectrum_close(&actual, &blur_reference(&original, 16), 2e-5);
                assert_eq!(channel.mags.len(), 2_049);
                assert_eq!(channel.mags.capacity(), capacity);
                assert_eq!(channel.mags.as_ptr(), storage);
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralSpreadEffect::default();
        assert_eq!(
            [effect.params.amount_pct, effect.params.fft_param],
            [0.0, 2.0]
        );
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [("Amount", 0.0, 100.0, 0.0), ("FFT", 0.0, 3.0, 2.0),]
        );
        assert!(
            specs
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 0,
                unit: " %"
            }
        ));
        assert!(
            matches!(specs[1].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (value, expected) in [(-99.0, 0.0), (37.5, 37.5), (999.0, 100.0)] {
            effect.set_param(0, value);
            assert_eq!(effect.params.amount_pct, expected);
        }
        for (value, size) in [
            (-99.0, 512),
            (0.49, 512),
            (0.5, 1_024),
            (1.5, 2_048),
            (99.0, 4_096),
        ] {
            effect.set_param(1, value);
            assert_eq!(effect.latency_samples(), size);
            assert_eq!(effect.engine_r.latency_samples(), size);
        }
        effect.set_param(99, f32::INFINITY);
        assert_eq!(effect.params.amount_pct, 100.0);
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_independent_prefix_means_and_phase_reconstruction() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], _: usize, _: f32) {
                spectrum.copy_from_slice(&blur_reference(spectrum, 8));
            }
        }
        let mut effect = configured_spread(0.0, 50.0);
        let mut reference_l = SpectralEngine::new(48_000.0);
        let mut reference_r = SpectralEngine::new(48_000.0);
        reference_l.set_fft_size(512);
        reference_r.set_fft_size(512);
        let mut energy = [0.0; 2];
        for sample in 0..8_192 {
            let input = (
                (sample as f32 * 0.17).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            let expected = (
                reference_l.process_sample(input.0, &mut Reference),
                reference_r.process_sample(input.1, &mut Reference),
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
    fn reset_clears_active_stereo_history_and_preserves_pending_fft_selection() {
        let mut used = configured_spread(0.0, 75.0);
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
        used.set_param(1, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_spread(0.0, 75.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(1, 1.0);
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
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_amount() {
        for index in 0..4 {
            let mut used = configured_spread(index as f32, 37.5);
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
            let mut fresh = configured_spread(index as f32, 37.5);
            fresh.set_sample_rate(32_000.0);
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

    fn drive(e: &mut SpectralSpreadEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_two() {
        assert_eq!(SpectralSpreadEffect::default().parameters().len(), 2);
    }

    #[test]
    fn amount_zero_is_passthrough() {
        let mut e = SpectralSpreadEffect::default();
        e.set_param(1, 1.0); // FFT = 1024 (slot 1 now)
        e.set_param(0, 0.0); // Amount = 0 -> radius=0 -> return early
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
        let mut e = SpectralSpreadEffect::default();
        e.set_param(0, 50.0); // arbitrary non-zero amount
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
