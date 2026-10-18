//! Spectral Compress: per-bin compression toward a target spectrum.
//!
//! Each bin's magnitude is tracked by a one-pole envelope follower
//! (~50 ms tau). The compression target is a power-law curve f^tone,
//! normalised at 1 kHz: tone=-1 -> pink (1/f), tone=0 -> flat, tone=+1
//! -> white (f). Per-bin ratio = (target / current)^Amount; the
//! magnitude scaling preserves the bin's phase.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralCompressChannel {
    /// Per-bin one-pole envelope of magnitude. Pre-allocated to the
    /// largest half-spectrum size so the audio thread never reallocates.
    avg_mag: Vec<f32>,
}

impl SpectralCompressChannel {
    fn new() -> Self {
        Self {
            avg_mag: vec![0.0; MAX_HALF_PLUS_ONE],
        }
    }
    fn reset(&mut self) {
        self.avg_mag.fill(0.0);
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    amount_pct: f32,
    tone_pct: f32, // -100..+100
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            amount_pct: 0.0,
            tone_pct: 0.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralCompressChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let amount = (self.params.amount_pct * 0.01).clamp(0.0, 1.0);
        if amount < 1e-3 {
            return;
        }
        let tone = (self.params.tone_pct * 0.01).clamp(-1.0, 1.0);
        let bin_hz = sample_rate / fft_size as f32;
        // One-pole follower coefficient, ~50 ms tau at the hop rate.
        let hop_samples = (fft_size / 2) as f32;
        let tau_samples = 0.050 * sample_rate;
        let alpha = (-hop_samples / tau_samples).exp();

        for k in 1..=half {
            let m = spectrum[k].norm();
            self.chan.avg_mag[k] = self.chan.avg_mag[k] * alpha + m * (1.0 - alpha);
            // Target curve: f^tone with reference at 1 kHz.
            let f_hz = k as f32 * bin_hz;
            let f_norm = (f_hz / 1000.0).max(1e-3);
            let target = f_norm.powf(tone);
            let current = self.chan.avg_mag[k].max(1e-12);
            let ratio = (target / current).powf(amount);
            spectrum[k].re *= ratio;
            spectrum[k].im *= ratio;
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        // DC (k=0) is untouched.
    }
}

pub struct SpectralCompressEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralCompressChannel,
    chan_r: SpectralCompressChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralCompressEffect {
    pub const PARAMS: [ParamSpec; 3] = [
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
        ParamSpec {
            name: "Tone",
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

impl Default for SpectralCompressEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralCompressChannel::new(),
            chan_r: SpectralCompressChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralCompressEffect {
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
            1 => self.params.tone_pct = value.clamp(-100.0, 100.0),
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
        spectrum[0] = Complex::new(0.25, 0.0);
        spectrum[half] = Complex::new(-magnitude, 0.0);
        for bin in 1..half {
            spectrum[bin] = Complex::new(0.6 * magnitude, -0.8 * magnitude);
            spectrum[size - bin] = spectrum[bin].conj();
        }
        spectrum
    }

    fn transform_with(
        channel: &mut SpectralCompressChannel,
        spectrum: &mut [Complex<f32>],
        rate: f32,
        amount: f32,
        tone: f32,
    ) {
        let size = spectrum.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                fft_param: 2.0,
                amount_pct: amount,
                tone_pct: tone,
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

    fn configured_compress(fft_index: f32) -> SpectralCompressEffect {
        let mut effect = SpectralCompressEffect::default();
        for (index, value) in [37.5, -50.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn magnitude_followers_match_closed_form_attack_and_release_at_each_fft_and_rate() {
        for size in [512, 1_024, 2_048, 4_096] {
            for rate in [8_000.0, 48_000.0, 192_000.0] {
                let mut channel = SpectralCompressChannel::new();
                let step = size as f64 / (2.0 * 0.050 * rate as f64);
                for hop in 1..=16 {
                    let (magnitude, envelope) = if hop <= 8 {
                        (5.0, 5.0 * (1.0 - (-(hop as f64) * step).exp()))
                    } else {
                        let attack_end = 5.0 * (1.0 - (-8.0 * step).exp());
                        (
                            1.0,
                            1.0 + (attack_end - 1.0) * (-((hop - 8) as f64) * step).exp(),
                        )
                    };
                    let mut spectrum = spectrum_with_magnitude(size, magnitude);
                    transform_with(&mut channel, &mut spectrum, rate, 50.0, 0.0);
                    let gain = (1.0 / envelope).sqrt();
                    for bin in [1, size / 8, size / 2] {
                        assert_close(channel.avg_mag[bin], envelope);
                        if bin == size / 2 {
                            assert_close(spectrum[bin].re, -(magnitude as f64) * gain);
                            assert_eq!(spectrum[bin].im, 0.0);
                        } else {
                            assert_close(spectrum[bin].re, 0.6 * magnitude as f64 * gain);
                            assert_close(spectrum[bin].im, -0.8 * magnitude as f64 * gain);
                            assert_eq!(spectrum[size - bin], spectrum[bin].conj());
                        }
                    }
                    assert_eq!(spectrum[0], Complex::new(0.25, 0.0));
                    assert_eq!(channel.avg_mag[0], 0.0);
                }
            }
        }
    }

    #[test]
    fn steady_state_targets_follow_tone_and_amount_while_preserving_complex_phase() {
        for tone in [-100.0, -50.0, 0.0, 50.0, 100.0] {
            for amount in [0.1, 25.0, 50.0, 100.0] {
                let mut channel = SpectralCompressChannel::new();
                channel.avg_mag.fill(5.0);
                let mut spectrum = spectrum_with_magnitude(512, 5.0);
                transform_with(&mut channel, &mut spectrum, 51_200.0, amount, tone);
                for (bin, normalized_frequency) in [(1, 0.1_f64), (10, 1.0), (20, 2.0), (256, 25.6)]
                {
                    let log_gain = amount as f64 / 100.0
                        * (tone as f64 / 100.0 * normalized_frequency.ln() - 5.0_f64.ln());
                    let gain = log_gain.exp();
                    let expected = if bin == 256 {
                        (-5.0 * gain, 0.0)
                    } else {
                        (3.0 * gain, -4.0 * gain)
                    };
                    assert_close(spectrum[bin].re, expected.0);
                    assert_close(spectrum[bin].im, expected.1);
                    if bin < 256 {
                        assert_eq!(spectrum[512 - bin], spectrum[bin].conj());
                    }
                }
                assert_eq!(spectrum[0], Complex::new(0.25, 0.0));
            }
        }
    }

    #[test]
    fn amount_deadband_preserves_audio_and_history_but_exact_threshold_is_active() {
        let mut channel = SpectralCompressChannel::new();
        channel.avg_mag.fill(0.375);
        let history = channel.avg_mag.clone();
        let source = spectrum_with_magnitude(512, 5.0);
        for amount in [0.0, 0.05, 0.0999] {
            let mut actual = source.clone();
            transform_with(&mut channel, &mut actual, 48_000.0, amount, -50.0);
            assert_eq!(actual, source);
            assert_eq!(channel.avg_mag, history);
        }
        let mut actual = source.clone();
        transform_with(&mut channel, &mut actual, 48_000.0, 0.1, -50.0);
        assert_ne!(actual, source);
        let expected = 5.0 + (0.375 - 5.0) * (-256.0_f64 / 2_400.0).exp();
        assert_close(channel.avg_mag[1], expected);
    }

    #[test]
    fn silent_and_tiny_bins_use_the_current_and_frequency_floors_without_nonfinite_output() {
        let size = 512;
        let mut channel = SpectralCompressChannel::new();
        let mut tiny = vec![Complex::default(); size];
        tiny[0] = Complex::new(2.0, 0.0);
        tiny[1] = Complex::new(3e-15, -4e-15);
        tiny[size - 1] = tiny[1].conj();
        tiny[size / 2] = Complex::new(-5e-15, 0.0);
        transform_with(&mut channel, &mut tiny, 48_000.0, 100.0, 0.0);
        assert!((tiny[1].re - 0.003).abs() < 2e-8);
        assert!((tiny[1].im + 0.004).abs() < 2e-8);
        assert!((tiny[size / 2].re + 0.005).abs() < 2e-8);
        assert_eq!(tiny[0], Complex::new(2.0, 0.0));
        assert_eq!(tiny[size - 1], tiny[1].conj());
        let mut silence = vec![Complex::default(); size];
        transform_with(&mut channel, &mut silence, 48_000.0, 100.0, -100.0);
        assert!(silence.iter().all(|value| *value == Complex::default()));

        for (tone, target) in [(-100.0, 1_000.0), (100.0, 0.001)] {
            let mut channel = SpectralCompressChannel::new();
            channel.avg_mag.fill(5.0);
            let mut spectrum = spectrum_with_magnitude(4_096, 5.0);
            // At 2048 Hz, bins 1 and 2 are 0.5/1 Hz: both use f_norm=0.001.
            transform_with(&mut channel, &mut spectrum, 2_048.0, 100.0, tone);
            for bin in [1, 2] {
                assert_close(spectrum[bin].re, 0.6 * target);
                assert_close(spectrum[bin].im, -0.8 * target);
            }
        }
    }

    #[test]
    fn follower_storage_is_bounded_reused_and_reset_including_inactive_upper_bins() {
        let mut effect = SpectralCompressEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert_eq!(channel.avg_mag.len(), 2_049);
            let storage = channel.avg_mag.as_ptr();
            let capacity = channel.avg_mag.capacity();
            for size in [512, 4_096, 1_024, 2_048] {
                channel.avg_mag.fill(2.0);
                channel.reset();
                let source = spectrum_with_magnitude(size, 5.0);
                let mut actual = source.clone();
                let mut expected = source;
                let mut fresh = SpectralCompressChannel::new();
                transform_with(channel, &mut actual, 48_000.0, 50.0, -50.0);
                transform_with(&mut fresh, &mut expected, 48_000.0, 50.0, -50.0);
                assert_eq!(actual, expected);
                assert_eq!(channel.avg_mag, fresh.avg_mag);
                assert_eq!(channel.avg_mag.len(), 2_049);
                assert_eq!(channel.avg_mag.capacity(), capacity);
                assert_eq!(channel.avg_mag.as_ptr(), storage);
            }
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralCompressEffect::default();
        assert_eq!(
            [
                effect.params.amount_pct,
                effect.params.tone_pct,
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
                ("Amount", 0.0, 100.0, 0.0),
                ("Tone", -100.0, 100.0, 0.0),
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
            (0, -99.0, 0.0),
            (0, 37.5, 37.5),
            (0, 999.0, 100.0),
            (1, -999.0, -100.0),
            (1, -12.5, -12.5),
            (1, 999.0, 100.0),
        ] {
            effect.set_param(slot, value);
            assert_eq!(
                [effect.params.amount_pct, effect.params.tone_pct][slot],
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
            [effect.params.amount_pct, effect.params.tone_pct],
            [100.0, 100.0]
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_log_gain_and_polar_reference() {
        struct Reference {
            averages: Vec<f64>,
        }
        impl SpectralTransform for Reference {
            fn transform(&mut self, spectrum: &mut [Complex<f32>], size: usize, rate: f32) {
                let decay = (-(size as f64) / (2.0 * 0.050 * rate as f64)).exp();
                for bin in 1..=size / 2 {
                    let value = spectrum[bin];
                    let magnitude = (value.re as f64).hypot(value.im as f64);
                    let average = magnitude + (self.averages[bin] - magnitude) * decay;
                    self.averages[bin] = average;
                    let frequency = (bin as f64 * rate as f64 / (size as f64 * 1_000.0)).max(0.001);
                    let log_gain = 0.375 * (-0.5 * frequency.ln() - average.max(1e-12).ln());
                    let amplitude = magnitude * log_gain.exp();
                    let angle = (value.im as f64).atan2(value.re as f64);
                    spectrum[bin] = Complex::new(
                        (amplitude * angle.cos()) as f32,
                        (amplitude * angle.sin()) as f32,
                    );
                    if bin < size / 2 {
                        spectrum[size - bin] = spectrum[bin].conj();
                    }
                }
            }
        }
        let mut effect = configured_compress(0.0);
        effect.set_sample_rate(32_000.0);
        let mut reference_l = SpectralEngine::new(32_000.0);
        let mut reference_r = SpectralEngine::new(32_000.0);
        reference_l.set_fft_size(512);
        reference_r.set_fft_size(512);
        let mut context_l = Reference {
            averages: vec![0.0; 2_049],
        };
        let mut context_r = Reference {
            averages: vec![0.0; 2_049],
        };
        let mut energy = [0.0; 2];
        for sample in 0..8_192 {
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
                (actual.0 - expected.0).abs() < 2e-5 && (actual.1 - expected.1).abs() < 2e-5,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|energy| energy > 0.01));
    }

    #[test]
    fn reset_clears_stereo_audio_and_follower_history_while_preserving_pending_fft() {
        let mut used = configured_compress(0.0);
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
        let mut fresh = configured_compress(0.0);
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
            let mut used = configured_compress(index as f32);
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
            let mut fresh = configured_compress(index as f32);
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

    fn drive(e: &mut SpectralCompressEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralCompressEffect::default().parameters().len(), 3);
    }

    #[test]
    fn amount_zero_is_passthrough() {
        let mut e = SpectralCompressEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.0); // Amount = 0 -> return early (slot 0)
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
        let mut e = SpectralCompressEffect::default();
        e.set_param(0, 50.0); // arbitrary non-zero Amount (slot 0)
        let out = drive(&mut e, 4096, |_| 0.0);
        assert!(out.iter().all(|x| x.abs() < 1e-6));
    }
}
