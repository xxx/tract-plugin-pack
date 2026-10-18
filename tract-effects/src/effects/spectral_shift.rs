//! Spectral Shift: per-bin frequency translation and scaling.
//!
//! Translate moves bins by +/- 100% of Nyquist; out-of-range bins are zeroed
//! (vs. SpectralRotate's wrap). Scale (0.5..2.0) expands or contracts the
//! spectrum around DC. The combined source position for destination bin k is
//! `(k - translate_bins) / scale`, linearly interpolated between floor and
//! ceil. Scale=1 + |Translate| < 0.5 bins short-circuits to identity.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

#[derive(Clone, Copy)]
pub(super) struct ParamsCache {
    pub fft_param: f32,
    pub scale: f32,
    pub translate_pct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            scale: 1.0,
            translate_pct: 0.0,
        }
    }
}

struct TransformCtx {
    scale: f32,
    translate_pct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sample_rate: f32) {
        let half = fft_size as i32 / 2;
        let scale = self.scale.clamp(0.5, 2.0);
        let translate_bins = (self.translate_pct * 0.01) * half as f32;
        // Identity short-circuit: scale==1 and integer translate==0 -> no-op.
        let identity = (scale - 1.0).abs() < 1e-6 && translate_bins.abs() < 0.5;
        if identity {
            return;
        }

        // Stash positive half into negative-half slots so we can read them
        // while writing new positive-half values.
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k];
        }
        // For each destination bin k, the source position in the stashed
        // positive half is (k - translate_bins) / scale. Linear-interpolate
        // between floor and ceil. Out-of-range source -> zero.
        for k in 1..half {
            let src = (k as f32 - translate_bins) / scale;
            let src_floor = src.floor() as i32;
            let src_ceil = src.ceil() as i32;
            let frac = src - src_floor as f32;
            let read_stash = |s: i32| -> Complex<f32> {
                if (1..half).contains(&s) {
                    spectrum[fft_size - s as usize]
                } else {
                    Complex::default()
                }
            };
            let a = read_stash(src_floor);
            let b = read_stash(src_ceil);
            spectrum[k as usize] = Complex::new(
                a.re * (1.0 - frac) + b.re * frac,
                a.im * (1.0 - frac) + b.im * frac,
            );
        }
        // Rebuild negative half from conjugates of (now-rewritten) positive half.
        for k in 1..half as usize {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralShiftEffect {
    sample_rate: f32,
    pub(super) params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralShiftEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Scale",
            min: 0.5,
            max: 2.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: "",
            },
        },
        ParamSpec {
            name: "Translate",
            min: -100.0,
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

impl Default for SpectralShiftEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralShiftEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            scale: self.params.scale,
            translate_pct: self.params.translate_pct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            scale: self.params.scale,
            translate_pct: self.params.translate_pct,
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
    }

    fn set_param(&mut self, index: usize, value: f32) {
        match index {
            0 => self.params.scale = value.clamp(0.5, 2.0),
            1 => self.params.translate_pct = value.clamp(-100.0, 100.0),
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

    fn sparse_spectrum(size: usize) -> Vec<Complex<f32>> {
        let half = size / 2;
        let mut spectrum = vec![Complex::default(); size];
        spectrum[0] = Complex::new(0.25, 0.0);
        spectrum[half] = Complex::new(-0.75, 0.0);
        for (bin, value) in [
            (1, Complex::new(0.75, -0.5)),
            (2, Complex::new(-0.25, 0.875)),
            (half / 3, Complex::new(0.125, -0.625)),
            (half - 2, Complex::new(0.625, 0.375)),
            (half - 1, Complex::new(-0.875, 0.25)),
        ] {
            spectrum[bin] = value;
            spectrum[size - bin] = value.conj();
        }
        spectrum
    }

    fn scatter_reference(
        source: &[Complex<f32>],
        scale: f64,
        translation: f64,
    ) -> Vec<Complex<f32>> {
        let size = source.len();
        let half = size / 2;
        let mut accumulated = vec![Complex::<f64>::default(); half];
        for (bin, value) in source.iter().enumerate().take(half).skip(1) {
            if *value == Complex::default() {
                continue;
            }
            let target = bin as f64 * scale + translation;
            for (destination, sum) in accumulated.iter_mut().enumerate().skip(1) {
                let weight = (1.0 - (destination as f64 - target).abs() / scale).max(0.0);
                sum.re += value.re as f64 * weight;
                sum.im += value.im as f64 * weight;
            }
        }
        let mut expected = vec![Complex::default(); size];
        expected[0] = source[0];
        expected[half] = source[half];
        for bin in 1..half {
            expected[bin] = Complex::new(accumulated[bin].re as f32, accumulated[bin].im as f32);
            expected[size - bin] = expected[bin].conj();
        }
        expected
    }

    fn assert_spectrum_close(actual: &[Complex<f32>], expected: &[Complex<f32>], tolerance: f32) {
        for (bin, (actual, expected)) in actual.iter().zip(expected).enumerate() {
            assert!(
                (actual.re - expected.re).abs() <= tolerance
                    && (actual.im - expected.im).abs() <= tolerance,
                "bin {bin}: {actual:?} vs {expected:?}"
            );
        }
    }

    fn configured_shift(fft_index: f32, scale: f32, translate: f32) -> SpectralShiftEffect {
        let mut effect = SpectralShiftEffect::default();
        effect.set_param(0, scale);
        effect.set_param(1, translate);
        effect.set_param(2, fft_index);
        effect
    }

    #[test]
    fn integer_and_fractional_translations_match_triangular_scattering_at_every_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            let half = size / 2;
            let source = sparse_spectrum(size);
            for translation in [-(half as f32), -1.25, -0.5, 0.5, 1.25, half as f32] {
                let mut actual = source.clone();
                let mut transform = TransformCtx {
                    scale: 1.0,
                    translate_pct: translation * 100.0 / half as f32,
                };
                transform.transform(&mut actual, size, 48_000.0);
                let expected = scatter_reference(&source, 1.0, translation as f64);
                assert_spectrum_close(&actual, &expected, 1e-6);
                assert_eq!(actual[0], source[0]);
                assert_eq!(actual[half], source[half]);
                if translation.abs() == half as f32 {
                    assert!(
                        actual[1..half]
                            .iter()
                            .all(|value| *value == Complex::default())
                    );
                }
            }
        }
    }

    #[test]
    fn scaling_and_combined_translation_match_a_source_driven_interpolation_oracle() {
        for size in [512, 1_024, 2_048, 4_096] {
            let half = size / 2;
            let source = sparse_spectrum(size);
            for scale in [0.5, 1.5, 2.0] {
                for translation in [0.0, -0.75, half as f32 * 0.25] {
                    let mut actual = source.clone();
                    TransformCtx {
                        scale,
                        translate_pct: translation * 100.0 / half as f32,
                    }
                    .transform(&mut actual, size, 32_000.0);
                    let expected = scatter_reference(&source, scale as f64, translation as f64);
                    // A non-binary source coordinate can round by one f32 ULP.
                    let tolerance = if scale == 1.5 { 2e-4 } else { 1e-6 };
                    assert_spectrum_close(&actual, &expected, tolerance);
                }
            }
        }
    }

    #[test]
    fn identity_shortcut_respects_scale_epsilon_and_the_exact_half_bin_boundary() {
        let size = 512;
        let source = sparse_spectrum(size);
        let unit = 1.0_f32.to_bits();
        for scale in [1.0, f32::from_bits(unit + 8), f32::from_bits(unit - 16)] {
            for translation in [-0.49, 0.0, 0.49] {
                let mut actual = source.clone();
                TransformCtx {
                    scale,
                    translate_pct: translation * 100.0 / 256.0,
                }
                .transform(&mut actual, size, 48_000.0);
                assert_eq!(actual, source);
            }
        }
        for (scale, translation) in [
            (1.0, -0.5),
            (1.0, 0.5),
            (f32::from_bits(unit + 9), 0.0),
            (f32::from_bits(unit - 17), 0.0),
        ] {
            let mut actual = source.clone();
            TransformCtx {
                scale,
                translate_pct: translation * 100.0 / 256.0,
            }
            .transform(&mut actual, size, 48_000.0);
            assert_ne!(actual, source);
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralShiftEffect::default();
        assert_eq!(effect.params.fft_param, 2.0);
        assert_eq!(effect.params.scale, 1.0);
        assert_eq!(effect.params.translate_pct, 0.0);
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Scale", 0.5, 2.0, 1.0),
                ("Translate", -100.0, 100.0, 0.0),
                ("FFT", 0.0, 3.0, 2.0),
            ]
        );
        assert!(
            specs
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 2,
                unit: ""
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
        for (value, expected) in [(-99.0, 0.5), (1.25, 1.25), (99.0, 2.0)] {
            effect.set_param(0, value);
            assert_eq!(effect.params.scale, expected);
        }
        for (value, expected) in [(-999.0, -100.0), (12.5, 12.5), (999.0, 100.0)] {
            effect.set_param(1, value);
            assert_eq!(effect.params.translate_pct, expected);
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
            (effect.params.scale, effect.params.translate_pct),
            (2.0, 100.0)
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_scaled_and_translated_scatter() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                let source = spectrum.to_vec();
                let half = size / 2;
                spectrum[1..half].fill(Complex::default());
                for (bin, value) in source.iter().enumerate().take(half).skip(1) {
                    // Scale=2, Translate=25%: each source has a three-bin
                    // triangular footprint, centred at 2*j + N/8.
                    let target = 2 * bin + size / 8;
                    for (destination, weight) in
                        [(target - 1, 0.5), (target, 1.0), (target + 1, 0.5)]
                    {
                        if destination < half {
                            spectrum[destination] += *value * weight;
                        }
                    }
                }
                for bin in 1..half {
                    spectrum[size - bin] = spectrum[bin].conj();
                }
            }
        }
        let mut effect = configured_shift(0.0, 2.0, 25.0);
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
                (actual.0 - expected.0).abs() < 1e-6 && (actual.1 - expected.1).abs() < 1e-6,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_clears_both_channels_and_preserves_pending_fft_selection() {
        let mut used = configured_shift(0.0, 0.75, -12.5);
        for sample in 0..4_096 {
            used.process_sample((sample as f32 * 0.23).sin() * 0.25, 0.125);
        }
        used.set_param(2, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_shift(0.0, 0.75, -12.5);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(2, 1.0);
        fresh.reset();
        for sample in 0..8_192 {
            let input = ((sample as f32 * 0.11).cos() * 0.125, -0.25);
            assert_eq!(
                used.process_sample(input.0, input.1),
                fresh.process_sample(input.0, input.1)
            );
        }
    }

    #[test]
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_transform_parameters() {
        for index in 0..4 {
            let mut used = configured_shift(index as f32, 1.5, -12.5);
            for sample in 0..5_000 {
                used.process_sample((sample as f32 * 0.19).sin() * 0.25, 0.125);
            }
            used.set_sample_rate(32_000.0);
            let mut fresh = configured_shift(index as f32, 1.5, -12.5);
            fresh.set_sample_rate(32_000.0);
            assert_eq!(used.sample_rate, 32_000.0);
            assert_eq!(used.latency_samples(), [512, 1_024, 2_048, 4_096][index]);
            for sample in 0..8_192 {
                let input = ((sample as f32 * 0.13).cos() * 0.25, -0.125);
                assert_eq!(
                    used.process_sample(input.0, input.1),
                    fresh.process_sample(input.0, input.1)
                );
            }
        }
    }

    fn drive(e: &mut SpectralShiftEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralShiftEffect::default().parameters().len(), 3);
    }

    #[test]
    fn translate_zero_is_passthrough() {
        let mut e = SpectralShiftEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(1, 0.0); // Translate = 0 (slot 1)
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
        let mut e = SpectralShiftEffect::default();
        e.set_param(1, 50.0); // Translate (slot 1)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn translate_positive_moves_energy_up_with_zero_out_of_range() {
        // A 1 kHz sine translated by +25% of Nyquist (= +6 kHz at 48 kHz SR)
        // should produce energy at ~7 kHz (1 + 6). Verify via forward-FFT
        // of the tail; also confirm DC/low-bin region has little energy.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralShiftEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2), Nyquist = 24 kHz, 25% = 6 kHz
        e.set_param(1, 25.0); // Translate (slot 1)
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_7k = (7000.0 * 2048.0 / sr).round() as usize;
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let e_7k: f32 = tail[bin_7k.saturating_sub(2)..=bin_7k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1k: f32 = tail[bin_1k.saturating_sub(2)..=bin_1k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_7k > e_1k,
            "expected translated energy at 7 kHz > source 1 kHz; got 7k={e_7k} 1k={e_1k}"
        );
    }

    #[test]
    fn scale_2_doubles_frequency_content() {
        // A 500 Hz sine fed through Scale=2 should produce most of its energy
        // around 1 kHz (the source bin maps to destination = src * scale).
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralShiftEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 2.0); // Scale = 2 (slot 0)
        e.set_param(1, 0.0); // Translate = 0 (slot 1)
        let n = 8192_usize;
        let out: Vec<f32> = (0..n)
            .map(|i| {
                let x = (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr).sin();
                e.process_sample(x, x).0
            })
            .collect();
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_500 = (500.0 * 2048.0 / sr).round() as usize;
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let e_500: f32 = tail[bin_500.saturating_sub(2)..=bin_500 + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1k: f32 = tail[bin_1k.saturating_sub(2)..=bin_1k + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_1k > e_500,
            "expected energy at 1 kHz > 500 Hz; got 1k={e_1k} 500={e_500}"
        );
    }
}
