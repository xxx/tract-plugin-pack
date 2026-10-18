//! Spectral Mirror: flips a portion of the spectrum around a centre frequency.
//!
//! Within +/- Width/2 octaves of Freq, the bin at offset +d above the centre
//! is swapped with the bin at offset -d below. Conjugate-mirror so the
//! reconstructed real-output preserves the right phase relationships.

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
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins =
            ((self.freq_hz * (self.bw_oct * 0.5).exp2() - self.freq_hz) / bin_hz).ceil() as i32;
        // For each offset d in [1..=band_bins], swap centre+d with centre-d
        // (conjugate-swap so phase reflects, not just copies).
        for d in 1..=band_bins {
            let kp = centre_bin + d;
            let kn = centre_bin - d;
            if kn > 0 && (kp as usize) < half {
                let a = spectrum[kp as usize];
                let b = spectrum[kn as usize];
                spectrum[kp as usize] = b.conj();
                spectrum[kn as usize] = a.conj();
            }
        }
        // Rebuild negative half from conjugates of (now-rewritten) positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralMirrorEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralMirrorEffect {
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

impl Default for SpectralMirrorEffect {
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

impl Effect for SpectralMirrorEffect {
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
            spectrum[bin] = Complex::new(bin as f32 * 0.125, bin as f32 * -0.25);
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn configured_mirror(fft_index: f32) -> SpectralMirrorEffect {
        let mut effect = SpectralMirrorEffect::default();
        effect.set_param(0, 1_600.0);
        effect.set_param(1, 2.0);
        effect.set_param(2, fft_index);
        effect
    }

    #[test]
    fn mirror_conjugates_exact_pairs_and_preserves_centres_band_edges_dc_and_nyquist() {
        // At 51.2 kHz and FFT 512, bins are 100 Hz apart. The pair counts
        // include octave-radius rounding and clipping before DC/Nyquist.
        for (rate, frequency, width, centre, pairs) in [
            (51_200.0, 1_600.0, 0.1, 16, 1),
            (51_200.0, 1_600.0, 1.0, 16, 7),
            (51_200.0, 1_550.0, 1.0, 16, 7),
            (51_200.0, 1_600.0, 2.0, 16, 15),
            (51_200.0, 20_000.0, 2.0, 200, 55),
            (51_200.0, 20.0, 0.1, 0, 0),
            (51_200.0, 100.0, 4.0, 1, 0),
            (8_000.0, 20_000.0, 0.1, 1_280, 0),
        ] {
            let size = 512;
            let original = indexed_spectrum(size);
            let mut expected = original.clone();
            for offset in 1..=pairs {
                let lower = original[centre - offset];
                let upper = original[centre + offset];
                expected[centre + offset] = Complex::new(lower.re, -lower.im);
                expected[centre - offset] = Complex::new(upper.re, -upper.im);
            }
            for bin in 1..size / 2 {
                expected[size - bin] = expected[bin].conj();
            }
            let mut actual = original;
            TransformCtx {
                freq_hz: frequency,
                bw_oct: width,
            }
            .transform(&mut actual, size, rate);
            assert_eq!(
                actual, expected,
                "rate {rate}, frequency {frequency}, width {width}"
            );
        }
    }

    #[test]
    fn mirroring_twice_restores_complex_spectra_at_every_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            for width in [0.1, 1.0, 4.0] {
                let original = indexed_spectrum(size);
                let mut actual = original.clone();
                let mut transform = TransformCtx {
                    freq_hz: 1_500.0,
                    bw_oct: width,
                };
                transform.transform(&mut actual, size, 48_000.0);
                assert_ne!(actual, original);
                transform.transform(&mut actual, size, 48_000.0);
                assert_eq!(actual, original);
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralMirrorEffect::default();
        assert_eq!(effect.params.fft_param, 2.0);
        assert_eq!(effect.params.freq_hz, 1_000.0);
        assert_eq!(effect.params.bw_oct, 1.0);
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
        for (value, expected) in [(-99.0, 20.0), (1_234.0, 1_234.0), (99_999.0, 20_000.0)] {
            effect.set_param(0, value);
            assert_eq!(effect.params.freq_hz, expected);
        }
        for (value, expected) in [(-99.0, 0.1), (1.75, 1.75), (99.0, 4.0)] {
            effect.set_param(1, value);
            assert_eq!(effect.params.bw_oct, expected);
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
        assert_eq!(effect.params.freq_hz, 20_000.0);
        assert_eq!(effect.params.bw_oct, 4.0);
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_a_fixed_pair_reflection_at_a_non_default_sample_rate() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                // Freq=1600 Hz at 51200 Hz is bin N/32; Width=2 reflects
                // all positive bins below twice that centre.
                let source = spectrum.to_vec();
                let centre = size / 32;
                for bin in 1..2 * centre {
                    if bin != centre {
                        let reflected = source[2 * centre - bin];
                        spectrum[bin] = Complex::new(reflected.re, -reflected.im);
                    }
                }
                for bin in 1..size / 2 {
                    spectrum[size - bin] = spectrum[bin].conj();
                }
            }
        }
        let mut effect = configured_mirror(0.0);
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
    fn reset_clears_both_channels_and_preserves_pending_fft_selection() {
        let mut used = configured_mirror(0.0);
        for sample in 0..4_096 {
            used.process_sample((sample as f32 * 0.23).sin() * 0.25, 0.125);
        }
        used.set_param(2, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_mirror(0.0);
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
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_mirror_band() {
        for index in 0..4 {
            let mut used = configured_mirror(index as f32);
            for sample in 0..5_000 {
                used.process_sample((sample as f32 * 0.19).sin() * 0.25, 0.125);
            }
            used.set_sample_rate(32_000.0);
            let mut fresh = configured_mirror(index as f32);
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

    fn drive(e: &mut SpectralMirrorEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralMirrorEffect::default();
        e.set_param(0, 5000.0); // Freq (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralMirrorEffect::default().parameters().len(), 3);
    }

    #[test]
    fn mirror_swaps_test_tones() {
        // A 500 Hz sine mirrored around 1000 Hz with 2-octave Width should
        // emerge centred near 2000 Hz (since 500 and 2000 are equidistant
        // from 1000 in log-frequency space, but we mirror linearly in bins
        // -- so the centre_bin reflection sends a peak at bin_500 to
        // bin_centre + (bin_centre - bin_500). Verify by forward-FFT of the
        // tail and checking energy is at the mirror position, not the source.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let mut e = SpectralMirrorEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 1000.0); // centre at 1 kHz (slot 0)
        e.set_param(1, 2.0); // 2 octaves wide (slot 1)
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * 500.0 * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        // Energy at the source frequency (500 Hz) should drop, energy at the
        // linear-bin mirror (~1500 Hz: 1000 + (1000 - 500)) should rise.
        let bin_500 = (500.0 * 2048.0 / sr).round() as usize;
        let bin_1500 = (1500.0 * 2048.0 / sr).round() as usize;
        let e_500: f32 = tail[bin_500.saturating_sub(2)..=bin_500 + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        let e_1500: f32 = tail[bin_1500.saturating_sub(2)..=bin_1500 + 2]
            .iter()
            .map(|c| c.norm_sqr())
            .sum();
        assert!(
            e_1500 > e_500,
            "expected mirror energy at 1500 Hz > source 500 Hz; got 1500={e_1500} 500={e_500}"
        );
    }
}
