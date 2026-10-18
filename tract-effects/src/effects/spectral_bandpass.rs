//! Spectral Bandpass: FFT-based brickwall bandpass filter.
//!
//! Zeros every bin outside [Freq * 2^(-bw/2), Freq * 2^(bw/2)]. No smoothing
//! at the edges -- Infiltrator calls this "brickwall".

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    freq_hz: f32,
    bw_oct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            freq_hz: 1000.0,
            bw_oct: 1.0,
        }
    }
}

struct TransformCtx {
    freq_hz: f32,
    bw_oct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let low_hz = self.freq_hz * (-(self.bw_oct * 0.5)).exp2();
        let high_hz = self.freq_hz * (self.bw_oct * 0.5).exp2();
        let low_bin = (low_hz / bin_hz).floor() as i32;
        let high_bin = (high_hz / bin_hz).ceil() as i32;
        for k in 0..=half {
            if (k as i32) < low_bin || (k as i32) > high_bin {
                spectrum[k] = Complex::default();
                if k != 0 && k != half {
                    spectrum[fft_size - k] = Complex::default();
                }
            }
        }
    }
}

pub struct SpectralBandpassEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralBandpassEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Width",
            min: 0.1,
            max: 4.0,
            default: 1.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 2,
                unit: " oct",
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

impl Default for SpectralBandpassEffect {
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

impl Effect for SpectralBandpassEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            freq_hz: self.params.freq_hz,
            bw_oct: self.params.bw_oct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            freq_hz: self.params.freq_hz,
            bw_oct: self.params.bw_oct,
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
            0 => self.params.freq_hz = value.clamp(20.0, 20_000.0),
            1 => self.params.bw_oct = value.clamp(0.1, 4.0),
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

    fn indexed_spectrum(size: usize) -> Vec<Complex<f32>> {
        let mut spectrum = vec![Complex::default(); size];
        spectrum[0] = Complex::new(0.25, 0.0);
        spectrum[size / 2] = Complex::new(-0.75, 0.0);
        for bin in 1..size / 2 {
            spectrum[bin] = Complex::new(bin as f32 * 0.125, bin as f32 * -0.25 + 0.0625);
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn assert_mask(
        actual: &[Complex<f32>],
        original: &[Complex<f32>],
        band: Option<(usize, usize)>,
    ) {
        let size = original.len();
        for (index, (&actual, &original)) in actual.iter().zip(original).enumerate() {
            let bin = index.min(size - index);
            let kept = band.is_some_and(|(low, high)| (low..=high).contains(&bin));
            let expected = if kept { original } else { Complex::default() };
            assert_eq!(
                actual, expected,
                "spectrum index {index}, folded bin {bin}, band {band:?}"
            );
        }
    }

    fn configured_bandpass(fft_index: f32) -> SpectralBandpassEffect {
        let mut effect = SpectralBandpassEffect::default();
        for (index, value) in [1_600.0, 2.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn octave_cutoffs_round_outward_and_keep_exact_complex_values_at_both_edges() {
        // At 51.2 kHz and FFT 512, each bin spans 100 Hz.
        for (rate, frequency, width, band) in [
            (51_200.0, 1_600.0, 2.0, Some((8, 32))),
            (51_200.0, 1_600.0, 1.0, Some((11, 23))),
            (51_200.0, 1_600.0, 0.1, Some((15, 17))),
            (51_200.0, 1_600.0, 4.0, Some((4, 64))),
            (51_200.0, 1_550.0, 2.0, Some((7, 31))),
            (51_200.0, 20.0, 0.1, Some((0, 1))),
            (51_200.0, 20_000.0, 2.0, Some((100, 256))),
            (8_000.0, 20_000.0, 2.0, None),
            (40_960.0, 1_600.0, 2.0, Some((10, 40))),
            (96_000.0, 1_600.0, 2.0, Some((4, 18))),
        ] {
            let original = indexed_spectrum(512);
            let mut actual = original.clone();
            TransformCtx {
                freq_hz: frequency,
                bw_oct: width,
            }
            .transform(&mut actual, 512, rate);
            assert_mask(&actual, &original, band);
        }
    }

    #[test]
    fn bin_masks_track_sample_rate_and_every_supported_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            let original = indexed_spectrum(size);
            for (rate, low, high) in [(32_000.0, 32, 128), (64_000.0, 16, 64), (128_000.0, 8, 32)] {
                let mut actual = original.clone();
                TransformCtx {
                    freq_hz: 4_000.0,
                    bw_oct: 2.0,
                }
                .transform(&mut actual, size, rate);
                assert_mask(
                    &actual,
                    &original,
                    Some((low * (size / 512), high * (size / 512))),
                );
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralBandpassEffect::default();
        assert_eq!(
            [
                effect.params.freq_hz,
                effect.params.bw_oct,
                effect.params.fft_param
            ],
            [1_000.0, 1.0, 2.0]
        );
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            specs
                .iter()
                .map(|spec| (spec.name, spec.min, spec.max, spec.default))
                .collect::<Vec<_>>(),
            [
                ("Freq", 20.0, 20_000.0, 1_000.0),
                ("Width", 0.1, 4.0, 1.0),
                ("FFT", 0.0, 3.0, 2.0),
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[0].format, ParamFormat::Hertz));
        assert!(
            specs[1..]
                .iter()
                .all(|spec| matches!(spec.scaling, ParamScaling::Linear))
        );
        assert!(matches!(
            specs[1].format,
            ParamFormat::Number {
                decimals: 2,
                unit: " oct"
            }
        ));
        assert!(
            matches!(specs[2].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, value, expected) in [
            (0, -99.0, 20.0),
            (0, 1_234.0, 1_234.0),
            (0, 99_999.0, 20_000.0),
            (1, -99.0, 0.1),
            (1, 1.75, 1.75),
            (1, 99.0, 4.0),
        ] {
            effect.set_param(slot, value);
            assert_eq!(
                [effect.params.freq_hz, effect.params.bw_oct][slot],
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
            [effect.params.freq_hz, effect.params.bw_oct],
            [20_000.0, 4.0]
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_mask_of_contiguous_spectrum_ranges() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                // The 800..3200 Hz passband at 51200 Hz is [N/64,N/16].
                let low = size / 64;
                let high = size / 16;
                spectrum[..low].fill(Complex::default());
                spectrum[high + 1..size - high].fill(Complex::default());
                spectrum[size - low + 1..].fill(Complex::default());
            }
        }
        let mut effect = configured_bandpass(0.0);
        effect.set_sample_rate(51_200.0);
        let mut reference_l = SpectralEngine::new(51_200.0);
        let mut reference_r = SpectralEngine::new(51_200.0);
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
            assert_eq!(actual, expected, "sample {sample}");
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_clears_active_stereo_history_and_preserves_pending_fft_selection() {
        let mut used = configured_bandpass(0.0);
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
        let mut fresh = configured_bandpass(0.0);
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
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_band() {
        for index in 0..4 {
            let mut used = configured_bandpass(index as f32);
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
            let mut fresh = configured_bandpass(index as f32);
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

    fn drive(e: &mut SpectralBandpassEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralBandpassEffect::default();
        e.set_param(0, 5000.0); // Freq is now slot 0
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn narrow_band_kills_out_of_band_content() {
        let sr = 48_000.0;
        let mut e = SpectralBandpassEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 1000.0); // Freq (slot 0)
        // Half-octave passband centred at 1 kHz.
        e.set_param(1, 0.5); // Width (slot 1)
        // Drive a 5 kHz sine -- well outside the passband.
        let out = drive(&mut e, 4096, |i| {
            (2.0 * std::f32::consts::PI * 5000.0 * i as f32 / sr).sin()
        });
        let tail: Vec<f32> = out[2 * e.latency_samples()..].into();
        let peak = tail.iter().cloned().fold(0.0_f32, f32::max);
        assert!(
            peak < 0.1,
            "expected out-of-band 5 kHz to be attenuated to ~0, got peak {peak}"
        );
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralBandpassEffect::default().parameters().len(), 3);
    }
}
