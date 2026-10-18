//! Spectral Twist: fold in-band bins toward (or away from) a centre frequency.
//!
//! Within +/- Bandwidth/2 octaves of Freq, each source bin at offset d from
//! the centre maps to a destination at offset d * (1 - Twist). Twist=+1
//! collapses the band onto Freq (all in-band bins -> centre). Twist=-1
//! doubles their spread. Twist=0 is identity. Out-of-band passes through.
//!
//! Multiple sources can map to the same destination at Twist > 0 -- we
//! accumulate (add) into the destination so energy isn't lost.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    freq_hz: f32,
    twist_pct: f32, // -100..+100
    bw_oct: f32,
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            freq_hz: 1000.0,
            twist_pct: 0.0,
            bw_oct: 1.0,
        }
    }
}

struct TransformCtx {
    freq_hz: f32,
    twist_pct: f32,
    bw_oct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let twist = (self.twist_pct * 0.01).clamp(-1.0, 1.0);
        if twist == 0.0 {
            return;
        }
        let centre_bin = (self.freq_hz / bin_hz).round() as i32;
        let band_bins =
            (((self.freq_hz * (self.bw_oct * 0.5).exp2()) - self.freq_hz) / bin_hz).ceil() as i32;
        let scale = 1.0 - twist; // twist=+1 -> scale=0 (collapse), twist=-1 -> scale=2 (spread)

        // Stash positive half into negative-half slots so we can read sources
        // while writing destinations.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k];
        }
        // Zero in-band destinations so we can accumulate cleanly.
        for k in 1..half as i32 {
            let d = k - centre_bin;
            if d.abs() <= band_bins {
                spectrum[k as usize] = Complex::default();
            }
        }
        // Map each in-band source to its scaled destination, accumulating.
        for d_src in -band_bins..=band_bins {
            let src = centre_bin + d_src;
            if src < 1 || (src as usize) >= half {
                continue;
            }
            let d_dst_f = d_src as f32 * scale;
            let dst = centre_bin + d_dst_f.round() as i32;
            if dst < 1 || (dst as usize) >= half {
                continue;
            }
            let src_val = spectrum[fft_size - src as usize]; // from stash
            spectrum[dst as usize].re += src_val.re;
            spectrum[dst as usize].im += src_val.im;
        }
        // Rebuild negative half from conjugates of the rewritten positive half.
        for k in 1..half {
            spectrum[fft_size - k] = spectrum[k].conj();
        }
    }
}

pub struct SpectralTwistEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralTwistEffect {
    pub const PARAMS: [ParamSpec; 4] = [
        ParamSpec {
            name: "Freq",
            min: 20.0,
            max: 20_000.0,
            default: 1000.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Hertz,
        },
        ParamSpec {
            name: "Twist",
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

impl Default for SpectralTwistEffect {
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

impl Effect for SpectralTwistEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            freq_hz: self.params.freq_hz,
            twist_pct: self.params.twist_pct,
            bw_oct: self.params.bw_oct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            freq_hz: self.params.freq_hz,
            twist_pct: self.params.twist_pct,
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
            1 => self.params.twist_pct = value.clamp(-100.0, 100.0),
            2 => self.params.bw_oct = value.clamp(0.1, 4.0),
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

    fn rebuild_conjugates(spectrum: &mut [Complex<f32>]) {
        let size = spectrum.len();
        for bin in 1..size / 2 {
            spectrum[size - bin] = spectrum[bin].conj();
        }
    }

    fn configured_twist(fft_index: f32, twist: f32) -> SpectralTwistEffect {
        let mut effect = SpectralTwistEffect::default();
        for (index, value) in [1_600.0, twist, 2.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn small_nonzero_twists_move_bins_when_the_rounded_destination_changes() {
        let size = 4_096;
        for (percent, destination) in [(0.09, 1_599), (-0.09, 1_601), (0.1, 1_599), (-0.1, 1_601)] {
            let mut spectrum = vec![Complex::default(); size];
            spectrum[0] = Complex::new(0.25, 0.0);
            spectrum[size / 2] = Complex::new(-0.75, 0.0);
            spectrum[1_600] = Complex::new(0.75, -0.25);
            spectrum[2_040] = Complex::new(0.125, 0.5);
            for bin in 1..size / 2 {
                spectrum[size - bin] = spectrum[bin].conj();
            }
            let mut expected = spectrum.clone();
            expected[destination] = expected[1_600];
            expected[1_600] = Complex::default();
            for bin in 1..size / 2 {
                expected[size - bin] = expected[bin].conj();
            }
            TransformCtx {
                freq_hz: 10_000.0,
                twist_pct: percent,
                bw_oct: 2.0,
            }
            .transform(&mut spectrum, size, 40_960.0);
            assert_eq!(spectrum, expected, "Twist {percent}%");
        }
    }

    #[test]
    fn fixed_bin_maps_cover_collision_sums_spreading_and_signed_rounding_ties() {
        let size = 512;
        let original = indexed_spectrum(size);
        // Freq=1600 Hz, Width=1 at 100 Hz/bin selects sources 9..=23.
        for (percent, destinations) in [
            (100.0, [16; 15]),
            (
                50.0,
                [12, 13, 13, 14, 14, 15, 15, 16, 17, 17, 18, 18, 19, 19, 20],
            ),
            (
                -50.0,
                [5, 7, 8, 10, 11, 13, 14, 16, 18, 19, 21, 22, 24, 25, 27],
            ),
            (
                -100.0,
                [2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 22, 24, 26, 28, 30],
            ),
        ] {
            let mut expected = original.clone();
            expected[9..24].fill(Complex::default());
            for (source, destination) in (9..24).zip(destinations) {
                expected[destination] += original[source];
            }
            rebuild_conjugates(&mut expected);
            let mut actual = original.clone();
            TransformCtx {
                freq_hz: 1_600.0,
                twist_pct: percent,
                bw_oct: 1.0,
            }
            .transform(&mut actual, size, 51_200.0);
            assert_eq!(actual, expected, "Twist {percent}%");
        }
    }

    #[test]
    fn octave_radius_and_centre_rounding_select_exact_collapse_sources() {
        for (rate, frequency, width, centre, first, last) in [
            (51_200.0, 1_600.0, 0.1, 16, 15, 17),
            (51_200.0, 1_600.0, 1.0, 16, 9, 23),
            (51_200.0, 1_550.0, 1.0, 16, 9, 23),
            (51_200.0, 1_600.0, 2.0, 16, 1, 32),
            (51_200.0, 20_000.0, 2.0, 200, 1, 255),
            (51_200.0, 100.0, 0.1, 1, 1, 2),
            (51_200.0, 20.0, 0.1, 0, 1, 1),
            (8_000.0, 20_000.0, 2.0, 1_280, 1, 255),
        ] {
            let size = 512;
            let original = indexed_spectrum(size);
            let mut expected = original.clone();
            expected[first..=last].fill(Complex::default());
            if (1..size / 2).contains(&centre) {
                expected[centre] = original[first..=last].iter().copied().sum();
            }
            rebuild_conjugates(&mut expected);
            let mut actual = original;
            TransformCtx {
                freq_hz: frequency,
                twist_pct: 100.0,
                bw_oct: width,
            }
            .transform(&mut actual, size, rate);
            assert_eq!(
                actual, expected,
                "rate {rate}, Freq {frequency}, Width {width}"
            );
        }
    }

    #[test]
    fn spreading_discards_dc_and_nyquist_destinations_and_preserves_out_of_band_audio() {
        let size = 512;
        let original = indexed_spectrum(size);
        for frequency in [800.0, 20_000.0] {
            let mut expected = original.clone();
            if frequency == 800.0 {
                expected[1..17].fill(Complex::default());
                // Sources 1..4 leave the positive interior; 5..16 survive.
                for (source, destination) in (5..17).zip((2..=24).step_by(2)) {
                    expected[destination] += original[source];
                }
            } else {
                expected[1..size / 2].fill(Complex::default());
                // At centre 200 only sources 101..227 land in bins 2..254.
                for (source, destination) in (101..228).zip((2..=254).step_by(2)) {
                    expected[destination] = original[source];
                }
            }
            rebuild_conjugates(&mut expected);
            let mut actual = original.clone();
            TransformCtx {
                freq_hz: frequency,
                twist_pct: -100.0,
                bw_oct: 2.0,
            }
            .transform(&mut actual, size, 51_200.0);
            assert_eq!(actual, expected, "Freq {frequency}");
        }
    }

    #[test]
    fn collapse_and_zero_twist_preserve_their_contracts_at_every_fft_size() {
        for size in [512, 1_024, 2_048, 4_096] {
            let original = indexed_spectrum(size);
            let mut unchanged = original.clone();
            TransformCtx {
                freq_hz: 1_600.0,
                twist_pct: 0.0,
                bw_oct: 2.0,
            }
            .transform(&mut unchanged, size, 51_200.0);
            assert_eq!(unchanged, original);
            let centre = size / 32;
            let mut expected = original.clone();
            expected[1..=2 * centre].fill(Complex::default());
            expected[centre] = original[1..=2 * centre].iter().copied().sum();
            rebuild_conjugates(&mut expected);
            let mut actual = original;
            TransformCtx {
                freq_hz: 1_600.0,
                twist_pct: 100.0,
                bw_oct: 2.0,
            }
            .transform(&mut actual, size, 51_200.0);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralTwistEffect::default();
        assert_eq!(
            [
                effect.params.freq_hz,
                effect.params.twist_pct,
                effect.params.bw_oct,
                effect.params.fft_param
            ],
            [1_000.0, 0.0, 1.0, 2.0]
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
                ("Twist", -100.0, 100.0, 0.0),
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
                decimals: 0,
                unit: " %"
            }
        ));
        assert!(matches!(
            specs[2].format,
            ParamFormat::Number {
                decimals: 2,
                unit: " oct"
            }
        ));
        assert!(
            matches!(specs[3].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (slot, value, expected) in [
            (0, -99.0, 20.0),
            (0, 1_234.0, 1_234.0),
            (0, 99_999.0, 20_000.0),
            (1, -999.0, -100.0),
            (1, 12.5, 12.5),
            (1, 999.0, 100.0),
            (2, -99.0, 0.1),
            (2, 1.75, 1.75),
            (2, 99.0, 4.0),
        ] {
            effect.set_param(slot, value);
            assert_eq!(
                [
                    effect.params.freq_hz,
                    effect.params.twist_pct,
                    effect.params.bw_oct
                ][slot],
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
            effect.set_param(3, value);
            assert_eq!(effect.latency_samples(), size);
            assert_eq!(effect.engine_r.latency_samples(), size);
        }
        effect.set_param(99, f32::INFINITY);
        assert_eq!(
            [
                effect.params.freq_hz,
                effect.params.twist_pct,
                effect.params.bw_oct
            ],
            [20_000.0, 100.0, 4.0]
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_complex_band_sum() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                // Freq=1600 at 51200 Hz is bin N/32; Width=2 includes 1..N/16.
                let centre = size / 32;
                let sum: Complex<f32> = spectrum[1..=2 * centre].iter().copied().sum();
                spectrum[1..=2 * centre].fill(Complex::default());
                spectrum[centre] = sum;
                rebuild_conjugates(spectrum);
            }
        }
        let mut effect = configured_twist(0.0, 100.0);
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
        let mut used = configured_twist(0.0, -50.0);
        for sample in 0..4_096 {
            used.process_sample((sample as f32 * 0.23).sin() * 0.25, 0.125);
        }
        used.set_param(3, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_twist(0.0, -50.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(3, 1.0);
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
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_twist_parameters() {
        for index in 0..4 {
            let mut used = configured_twist(index as f32, 50.0);
            for sample in 0..5_000 {
                used.process_sample((sample as f32 * 0.19).sin() * 0.25, 0.125);
            }
            used.set_sample_rate(32_000.0);
            let mut fresh = configured_twist(index as f32, 50.0);
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

    fn drive(e: &mut SpectralTwistEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_four() {
        assert_eq!(SpectralTwistEffect::default().parameters().len(), 4);
    }

    #[test]
    fn twist_zero_is_passthrough() {
        let mut e = SpectralTwistEffect::default();
        e.set_param(3, 1.0); // FFT = 1024 (slot 3)
        e.set_param(1, 0.0); // Twist = 0 -> early return (slot 1)
        let f = 1000.0;
        let sr = 48_000.0;
        let out = drive(&mut e, 4096, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralTwistEffect::default();
        e.set_param(1, 50.0); // arbitrary twist (slot 1)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
