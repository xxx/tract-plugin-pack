//! Spectral Rotate: circular shift of the spectrum.
//!
//! Bins are rotated by `shift_bins = round(shift_pct/100 * N/2)`. Unlike
//! SpectralShift (which zeros out-of-range bins), Rotate wraps modulo N/2
//! so nothing is lost. See the spec doc for the full DSP outline.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

#[derive(Clone, Copy, Default)]
struct ParamsCache {
    fft_param: f32,
    shift_pct: f32,
}

struct TransformCtx {
    shift_pct: f32,
}

impl SpectralTransform for TransformCtx {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, _sample_rate: f32) {
        // Real-input spectrum is conjugate-symmetric around bin N/2.
        // Operate on the positive-frequency half [1..half) and mirror.
        let half = fft_size / 2;
        let shift_bins = ((self.shift_pct * 0.01) * half as f32).round() as i32;
        if shift_bins == 0 {
            return;
        }
        let n = fft_size;
        // Stash positive-half bins into negative-half slots.
        for k in 1..half {
            spectrum[n - k] = spectrum[k];
        }
        // Rotate write.
        for k in 1..half as i32 {
            let src = (((k - 1 - shift_bins).rem_euclid(half as i32 - 1)) + 1) as usize;
            // Read from the stashed mirror: stashed[m] is at spectrum[n - m].
            spectrum[k as usize] = spectrum[n - src];
        }
        // Rebuild negative half from conjugates of rotated positive half.
        for k in 1..half {
            spectrum[n - k] = spectrum[k].conj();
        }
        // DC (k=0) and Nyquist (k=half) are untouched.
    }
}

pub struct SpectralRotateEffect {
    sample_rate: f32,
    params: ParamsCache,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralRotateEffect {
    pub const PARAMS: [ParamSpec; 2] = [
        // Shift is intentionally narrow (+/-15%). Outside this range the
        // bin rotation moves nearly all musical energy out of the audible
        // low-bin band and replaces it with high-bin near-silence -- the
        // output collapses to noise/silence. Keep the dial in the useful
        // textural range.
        ParamSpec {
            name: "Shift",
            min: -15.0,
            max: 15.0,
            default: 0.0,
            scaling: ParamScaling::Linear,
            format: ParamFormat::Number {
                decimals: 0,
                unit: " %",
            },
        },
        // FFT in the LAST slot so it isn't the first dial users grab to
        // modulate. Modulating FFT mid-stream causes audible artefacts;
        // putting it where it's clearly a setting-not-a-knob keeps the
        // accidental-modulation rate down.
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

impl Default for SpectralRotateEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache {
                fft_param: 2.0,
                shift_pct: 0.0,
            },
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralRotateEffect {
    fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut ctx_l = TransformCtx {
            shift_pct: self.params.shift_pct,
        };
        let lo = self.engine_l.process_sample(left, &mut ctx_l);
        let mut ctx_r = TransformCtx {
            shift_pct: self.params.shift_pct,
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
        // Re-apply the FFT-size param so the new engines pick it up.
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
            0 => self.params.shift_pct = value.clamp(-15.0, 15.0),
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

    fn indexed_spectrum(size: usize) -> Vec<Complex<f32>> {
        let mut spectrum = vec![Complex::default(); size];
        spectrum[0] = Complex::new(0.25, 0.0);
        spectrum[size / 2] = Complex::new(-0.75, 0.0);
        for bin in 1..size / 2 {
            spectrum[bin] = Complex::new(bin as f32, bin as f32 * -0.25);
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn rotate_reference(spectrum: &mut [Complex<f32>], shift: i32) {
        let size = spectrum.len();
        let half = size / 2;
        if shift >= 0 {
            spectrum[1..half].rotate_right(shift as usize);
        } else {
            spectrum[1..half].rotate_left((-shift) as usize);
        }
        for bin in 1..half {
            spectrum[size - bin] = spectrum[bin].conj();
        }
    }

    fn configured_rotate(fft_index: f32, shift: f32) -> SpectralRotateEffect {
        let mut effect = SpectralRotateEffect::default();
        effect.set_param(0, shift);
        effect.set_param(1, fft_index);
        effect
    }

    #[test]
    fn one_bin_rotation_moves_each_positive_bin_and_preserves_its_phase() {
        let size = 512;
        let half = size / 2;
        let mut spectrum = indexed_spectrum(size);
        let mut expected = spectrum.clone();
        rotate_reference(&mut expected, 1);
        TransformCtx {
            shift_pct: 100.0 / half as f32,
        }
        .transform(&mut spectrum, size, 48_000.0);
        assert_eq!(spectrum, expected);
    }

    #[test]
    fn rotations_match_slice_permutations_at_rounding_wrap_and_fft_boundaries() {
        for (size, maximum_shift) in [(512, 38), (1_024, 77), (2_048, 154), (4_096, 307)] {
            let half = size / 2;
            for (percent, shift) in [
                (0.0, 0),
                (49.0 / half as f32, 0),
                (-49.0 / half as f32, 0),
                (50.0 / half as f32, 1),
                (-50.0 / half as f32, -1),
                (15.0, maximum_shift),
                (-15.0, -maximum_shift),
            ] {
                let mut spectrum = indexed_spectrum(size);
                let mut expected = spectrum.clone();
                rotate_reference(&mut expected, shift);
                TransformCtx { shift_pct: percent }.transform(&mut spectrum, size, 96_000.0);
                assert_eq!(spectrum, expected, "FFT {size}, shift {percent}");
                TransformCtx {
                    shift_pct: -percent,
                }
                .transform(&mut spectrum, size, 96_000.0);
                assert_eq!(spectrum, indexed_spectrum(size));
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralRotateEffect::default();
        assert_eq!(effect.params.fft_param, 2.0);
        assert_eq!(effect.params.shift_pct, 0.0);
        assert_eq!(effect.latency_samples(), 2_048);
        let specs = effect.parameters();
        assert_eq!(
            (specs[0].min, specs[0].max, specs[0].default),
            (-15.0, 15.0, 0.0)
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 0,
                unit: " %"
            }
        ));
        assert_eq!(
            (specs[1].min, specs[1].max, specs[1].default),
            (0.0, 3.0, 2.0)
        );
        assert!(
            matches!(specs[1].format, ParamFormat::Enum { labels } if labels == ["512", "1024", "2048", "4096"])
        );
        for (value, expected) in [(-99.0, -15.0), (-7.5, -7.5), (99.0, 15.0)] {
            effect.set_param(0, value);
            assert_eq!(effect.params.shift_pct, expected);
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
        assert_eq!(effect.params.shift_pct, 15.0);
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_independent_slice_rotations() {
        struct Reference;
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, _: f32) {
                // 12.5% of Nyquist is exactly N/16 bins at every supported size.
                rotate_reference(spectrum, (size / 16) as i32);
            }
        }
        let mut effect = configured_rotate(0.0, 12.5);
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
            assert_eq!(actual, expected, "sample {sample}");
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_clears_both_channels_and_preserves_pending_fft_selection() {
        let mut used = configured_rotate(0.0, -12.5);
        for sample in 0..4_096 {
            used.process_sample((sample as f32 * 0.23).sin() * 0.25, 0.125);
        }
        used.set_param(1, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        // Reproduce the same pending switch from the previously active 512 slot.
        let mut fresh = configured_rotate(0.0, -12.5);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(1, 1.0);
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
    fn sample_rate_rebuilds_both_engines_with_the_selected_fft_and_shift() {
        for index in 0..4 {
            let mut used = configured_rotate(index as f32, 12.5);
            for sample in 0..5_000 {
                used.process_sample((sample as f32 * 0.19).sin() * 0.25, 0.125);
            }
            used.set_sample_rate(32_000.0);
            let mut fresh = configured_rotate(index as f32, 12.5);
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

    fn drive(e: &mut SpectralRotateEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_lists_shift_then_fft() {
        let e = SpectralRotateEffect::default();
        let p = e.parameters();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "Shift");
        assert_eq!(p[1].name, "FFT");
    }

    #[test]
    fn shift_zero_is_passthrough() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(1, 1.0); // FFT = 1024 for shorter warm-up
        e.set_param(0, 0.0); // Shift = 0
        let f = 1000.0;
        let sr = 48_000.0;
        let n = 4096_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        // After warm-up, output should retain non-trivial energy.
        let energy: f32 = out[2 * e.latency_samples()..].iter().map(|x| x * x).sum();
        assert!(energy > 1.0);
    }

    #[test]
    fn silence_in_silence_out() {
        let mut e = SpectralRotateEffect::default();
        e.set_param(0, 10.0); // arbitrary non-zero shift within +/-15% range
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn fft_size_param_changes_latency() {
        // The latency reported to the host changes IMMEDIATELY when the FFT
        // param moves -- no audio needs to flow first. The audio path's
        // active slot still latches the switch until the next hop boundary,
        // but `latency_samples()` reports the pending slot's FFT size so the
        // host can re-align PDC right away.
        let mut e = SpectralRotateEffect::default();
        for (i, expected) in [(0.0, 512), (1.0, 1024), (2.0, 2048), (3.0, 4096)] {
            e.set_param(1, i); // FFT is the LAST param (slot 1)
            assert_eq!(e.latency_samples(), expected);
        }
    }

    #[test]
    fn shift_positive_moves_energy_up() {
        // A 1 kHz sine shifted +50% should produce most of its energy
        // above 1 kHz. Tested via a forward FFT of the output.
        use rustfft::num_complex::Complex;
        let sr = 48_000.0_f32;
        let f = 1000.0_f32;
        let mut e = SpectralRotateEffect::default();
        e.set_param(1, 1.0); // FFT = 1024 (slot 1)
        e.set_param(0, 15.0); // Shift = 15% (slot 0) -- max in the new range
        let n = 8192_usize;
        let out = drive(&mut e, n, |i| {
            (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()
        });
        let tail_start = 2 * e.latency_samples();
        let mut tail: Vec<Complex<f32>> = out[tail_start..tail_start + 2048]
            .iter()
            .map(|&x| Complex::new(x, 0.0))
            .collect();
        let mut planner = rustfft::FftPlanner::<f32>::new();
        planner.plan_fft_forward(2048).process(&mut tail);
        let bin_1k = (1000.0 * 2048.0 / sr).round() as usize;
        let lo: f32 = tail[..bin_1k].iter().map(|c| c.norm_sqr()).sum();
        let hi: f32 = tail[bin_1k..1024].iter().map(|c| c.norm_sqr()).sum();
        assert!(
            hi > lo,
            "expected hi-band energy > lo-band; got hi={hi} lo={lo}"
        );
    }
}
