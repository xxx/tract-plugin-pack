//! Spectral Reverb: per-bin one-pole feedback with frequency-dependent T60.
//!
//! Each bin maintains a running tail: tail_k = tail_k * g_k + input_k.
//! The per-bin gain g_k is derived from a target T60 (time to drop 60 dB),
//! which is shaped by Tone: at Tone=0 ("dark") HF bins decay fast (T60 = 0.1
//! \* Time); at Tone=1 ("bright") LF bins decay fast. Tone=0.5 gives
//! uniform decay with T60 = 0.55 * Time across the spectrum.

use super::{Effect, ParamFormat, ParamScaling, ParamSpec};
use rustfft::num_complex::Complex;
use tract_dsp::spectral_engine::{FFT_SIZES, SpectralEngine, SpectralTransform};

const MAX_HALF_PLUS_ONE: usize = 4096 / 2 + 1;

struct SpectralReverbChannel {
    /// Per-bin running tail (complex, so phase advances naturally each hop).
    tail: Vec<Complex<f32>>,
}

impl SpectralReverbChannel {
    fn new() -> Self {
        Self {
            tail: vec![Complex::default(); MAX_HALF_PLUS_ONE],
        }
    }
    fn reset(&mut self) {
        self.tail.fill(Complex::default());
    }
}

#[derive(Clone, Copy)]
struct ParamsCache {
    fft_param: f32,
    time_s: f32,
    tone_pct: f32, // 0..100
}

impl Default for ParamsCache {
    fn default() -> Self {
        Self {
            fft_param: 2.0,
            time_s: 2.0,
            tone_pct: 50.0,
        }
    }
}

struct TransformCtx<'a> {
    chan: &'a mut SpectralReverbChannel,
    params: ParamsCache,
}

impl SpectralTransform for TransformCtx<'_> {
    fn transform(&mut self, spectrum: &mut [Complex<f32>], fft_size: usize, sample_rate: f32) {
        let half = fft_size / 2;
        let bin_hz = sample_rate / fft_size as f32;
        let hop_samples = (fft_size / 2) as f32;
        let time_s = self.params.time_s.max(0.001);
        let tone = (self.params.tone_pct * 0.01).clamp(0.0, 1.0);

        for k in 1..=half {
            // Bin centre frequency, log-normalised 0..1 from 20 Hz to 20 kHz.
            let f_hz = (k as f32 * bin_hz).max(1.0);
            let f_norm = ((f_hz / 20.0).ln() / (20_000.0_f32 / 20.0).ln()).clamp(0.0, 1.0);
            // Damping factor per bin: 1.0 = full Time, 0.1 = 10% of Time.
            // bright curve: LF damped (low f_norm -> 0.1), HF preserved -> 1.0.
            let bright_damping = 1.0 - 0.9 * (1.0 - f_norm);
            // dark curve: HF damped (high f_norm -> 0.1), LF preserved -> 1.0.
            let dark_damping = 1.0 - 0.9 * f_norm;
            // Lerp between dark (tone=0) and bright (tone=1).
            let damp = (1.0 - tone) * dark_damping + tone * bright_damping;
            let t60_k = time_s * damp;
            // Per-hop decay coefficient: by definition T60 is the time for
            // amplitude to fall by 60 dB = 10^-3. Solve g^N = 10^-3 with
            // N = t60_samples / hop_samples, giving
            // g = 10^(-3 * hop_samples / (t60_k * sample_rate)).
            let g = 10.0_f32.powf(-3.0 * hop_samples / (t60_k * sample_rate));
            self.chan.tail[k].re = self.chan.tail[k].re * g + spectrum[k].re;
            self.chan.tail[k].im = self.chan.tail[k].im * g + spectrum[k].im;
            spectrum[k] = self.chan.tail[k];
            spectrum[fft_size - k] = spectrum[k].conj();
        }
        // DC (k=0) is untouched.
    }
}

pub struct SpectralReverbEffect {
    sample_rate: f32,
    params: ParamsCache,
    chan_l: SpectralReverbChannel,
    chan_r: SpectralReverbChannel,
    engine_l: SpectralEngine,
    engine_r: SpectralEngine,
}

impl SpectralReverbEffect {
    pub const PARAMS: [ParamSpec; 3] = [
        ParamSpec {
            name: "Time",
            min: 0.1,
            max: 20.0,
            default: 2.0,
            scaling: ParamScaling::Log,
            format: ParamFormat::Number {
                decimals: 2,
                unit: " s",
            },
        },
        ParamSpec {
            name: "Tone",
            min: 0.0,
            max: 100.0,
            default: 50.0,
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

impl Default for SpectralReverbEffect {
    fn default() -> Self {
        let sr = 48_000.0;
        Self {
            sample_rate: sr,
            params: ParamsCache::default(),
            chan_l: SpectralReverbChannel::new(),
            chan_r: SpectralReverbChannel::new(),
            engine_l: SpectralEngine::new(sr),
            engine_r: SpectralEngine::new(sr),
        }
    }
}

impl Effect for SpectralReverbEffect {
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
            0 => self.params.time_s = value.clamp(0.1, 20.0),
            1 => self.params.tone_pct = value.clamp(0.0, 100.0),
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

    fn impulse_spectrum(size: usize) -> Vec<Complex<f32>> {
        let mut bins = vec![Complex::default(); size];
        bins[0] = Complex::new(-0.25, 0.0);
        bins[size / 2] = Complex::new(0.5, 0.0);
        for bin in 1..size / 2 {
            bins[bin] = Complex::new(0.75, -0.25);
            bins[size - bin] = bins[bin].conj();
        }
        bins
    }

    fn transform(
        channel: &mut SpectralReverbChannel,
        bins: &mut [Complex<f32>],
        time: f32,
        tone: f32,
        sample_rate: f32,
    ) {
        let size = bins.len();
        TransformCtx {
            chan: channel,
            params: ParamsCache {
                time_s: time,
                tone_pct: tone,
                ..ParamsCache::default()
            },
        }
        .transform(bins, size, sample_rate);
    }

    fn assert_complex_near(actual: Complex<f32>, expected: Complex<f64>) {
        for (actual, expected) in [(actual.re, expected.re), (actual.im, expected.im)] {
            assert!(
                (actual as f64 - expected).abs() < 2e-7 + expected.abs() * 4e-5,
                "{actual} vs {expected}"
            );
        }
    }

    fn configured_reverb(fft_index: f32) -> SpectralReverbEffect {
        let mut effect = SpectralReverbEffect::default();
        for (index, value) in [0.4, 25.0, fft_index].into_iter().enumerate() {
            effect.set_param(index, value);
        }
        effect
    }

    #[test]
    fn impulse_decay_matches_tabulated_tone_curves_and_sixty_db_times() {
        // 10 Hz/bin and 50 ms/hop: probes span both frequency clamps and three decades.
        let probes = [1, 2, 20, 200, 2_000, 2_048];
        let original = impulse_spectrum(4_096);
        for (tone, damping) in [
            (0.0, [1.0, 1.0, 0.7, 0.4, 0.1, 0.1]),
            (25.0, [0.775, 0.775, 0.625, 0.475, 0.325, 0.325]),
            (50.0, [0.55; 6]),
            (75.0, [0.325, 0.325, 0.475, 0.625, 0.775, 0.775]),
            (100.0, [0.1, 0.1, 0.4, 0.7, 1.0, 1.0]),
        ] {
            let mut channel = SpectralReverbChannel::new();
            for hop in 0..=80 {
                let mut bins = if hop == 0 {
                    original.clone()
                } else {
                    vec![Complex::default(); 4_096]
                };
                transform(&mut channel, &mut bins, 2.0, tone, 40_960.0);
                assert_eq!(
                    bins[0],
                    if hop == 0 {
                        original[0]
                    } else {
                        Complex::default()
                    }
                );
                for (&bin, ratio) in probes.iter().zip(damping) {
                    let gain = 10.0_f64.powf(-3.0 * hop as f64 * 0.05 / (2.0 * ratio));
                    let expected = Complex::new(
                        original[bin].re as f64 * gain,
                        original[bin].im as f64 * gain,
                    );
                    assert_complex_near(bins[bin], expected);
                    assert_complex_near(channel.tail[bin], expected);
                    if bin < 2_048 {
                        assert_eq!(bins[4_096 - bin], bins[bin].conj());
                    }
                }
            }
        }
    }

    #[test]
    fn midpoint_decay_is_frequency_independent_at_all_fft_sizes_rates_and_time_bounds() {
        for size in [512, 1_024, 2_048, 4_096] {
            for sr in [32_000.0, 96_000.0] {
                for time in [0.1, 2.0, 20.0] {
                    let original = impulse_spectrum(size);
                    let mut channel = SpectralReverbChannel::new();
                    for hop in 0..=32 {
                        let mut bins = if hop == 0 {
                            original.clone()
                        } else {
                            vec![Complex::default(); size]
                        };
                        transform(&mut channel, &mut bins, time, 50.0, sr);
                        let elapsed = hop as f64 * (size / 2) as f64 / sr as f64;
                        let gain = 10.0_f64.powf(-3.0 * elapsed / (0.55 * time as f64));
                        for bin in 1..=size / 2 {
                            assert_complex_near(
                                bins[bin],
                                Complex::new(
                                    original[bin].re as f64 * gain,
                                    original[bin].im as f64 * gain,
                                ),
                            );
                            if bin < size / 2 {
                                assert_eq!(bins[size - bin], bins[bin].conj());
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn new_inputs_and_time_tone_changes_accumulate_without_erasing_the_tail() {
        let mut channel = SpectralReverbChannel::new();
        let first = impulse_spectrum(4_096);
        let mut bins = first.clone();
        transform(&mut channel, &mut bins, 1.0, 0.0, 40_960.0);
        let mut second = vec![Complex::new(-0.125, 0.375); 4_096];
        second[0] = Complex::new(0.25, 0.0);
        second[2_048] = Complex::new(-0.25, 0.0);
        for bin in 1..2_048 {
            second[4_096 - bin] = second[bin].conj();
        }
        let mut bins = second.clone();
        transform(&mut channel, &mut bins, 1.0, 0.0, 40_960.0);
        assert_eq!(bins[0], second[0]);
        for hop in 0..=8 {
            if hop > 0 {
                bins.fill(Complex::default());
                bins[0] = Complex::new(hop as f32 * 0.125, 0.0);
                transform(&mut channel, &mut bins, 2.0, 100.0, 40_960.0);
                assert_eq!(bins[0].re, hop as f32 * 0.125);
            }
            for (bin, dark, bright) in [
                (2, 1.0, 0.1),
                (20, 0.7, 0.4),
                (200, 0.4, 0.7),
                (2_000, 0.1, 1.0),
                (2_048, 0.1, 1.0),
            ] {
                let first_gain = 10.0_f64.powf(-3.0 * 0.05 / dark);
                let later_gain = 10.0_f64.powf(-3.0 * hop as f64 * 0.05 / (2.0 * bright));
                let expected = Complex::new(
                    (first[bin].re as f64 * first_gain + second[bin].re as f64) * later_gain,
                    (first[bin].im as f64 * first_gain + second[bin].im as f64) * later_gain,
                );
                assert_complex_near(bins[bin], expected);
                if bin < 2_048 {
                    assert_eq!(bins[4_096 - bin], bins[bin].conj());
                }
            }
        }
    }

    #[test]
    fn fft_changes_preserve_dormant_tails_and_reset_reuses_preallocated_storage() {
        let mut effect = SpectralReverbEffect::default();
        for channel in [&mut effect.chan_l, &mut effect.chan_r] {
            assert!(channel.tail.len() >= 2_049);
            let storage = channel.tail.as_ptr();
            let length = channel.tail.len();
            let capacity = channel.tail.capacity();
            channel.tail[1] = Complex::new(0.75, -0.25);
            channel.tail[2_048] = Complex::new(0.5, 0.0);
            let mut large_visits = 0;
            for (index, size) in [512, 4_096, 1_024, 2_048, 4_096].into_iter().enumerate() {
                let mut silence = vec![Complex::default(); size];
                transform(channel, &mut silence, 2.0, 50.0, size as f32 * 20.0);
                let low_gain = 10.0_f64.powf(-3.0 * (index + 1) as f64 * 0.025 / 1.1);
                assert_complex_near(silence[1], Complex::new(0.75 * low_gain, -0.25 * low_gain));
                if size == 4_096 {
                    large_visits += 1;
                }
                let high_gain = 10.0_f64.powf(-3.0 * large_visits as f64 * 0.025 / 1.1);
                assert_complex_near(channel.tail[2_048], Complex::new(0.5 * high_gain, 0.0));
                assert_eq!(channel.tail.as_ptr(), storage);
                assert_eq!(channel.tail.len(), length);
                assert_eq!(channel.tail.capacity(), capacity);
            }
            channel.reset();
            let mut silence = vec![Complex::default(); 4_096];
            transform(channel, &mut silence, 2.0, 50.0, 81_920.0);
            assert!(silence.iter().all(|value| *value == Complex::default()));
            assert_eq!(channel.tail.as_ptr(), storage);
            assert_eq!(channel.tail.len(), length);
            assert_eq!(channel.tail.capacity(), capacity);
        }
    }

    #[test]
    fn parameter_defaults_bounds_and_fft_selection_match_metadata() {
        let mut effect = SpectralReverbEffect::default();
        assert_eq!(
            (
                effect.params.time_s,
                effect.params.tone_pct,
                effect.params.fft_param
            ),
            (2.0, 50.0, 2.0)
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
                ("Time", 0.1, 20.0, 2.0),
                ("Tone", 0.0, 100.0, 50.0),
                ("FFT", 0.0, 3.0, 2.0)
            ]
        );
        assert!(matches!(specs[0].scaling, ParamScaling::Log));
        assert!(matches!(specs[1].scaling, ParamScaling::Linear));
        assert!(matches!(specs[2].scaling, ParamScaling::Linear));
        assert!(matches!(
            specs[0].format,
            ParamFormat::Number {
                decimals: 2,
                unit: " s"
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
        for (slot, cases) in [
            (0, [(-999.0, 0.1), (3.75, 3.75), (999.0, 20.0)]),
            (1, [(-999.0, 0.0), (37.5, 37.5), (999.0, 100.0)]),
        ] {
            for (value, expected) in cases {
                effect.set_param(slot, value);
                assert_eq!(
                    [effect.params.time_s, effect.params.tone_pct][slot],
                    expected
                );
            }
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
            (effect.params.time_s, effect.params.tone_pct),
            (20.0, 100.0)
        );
        assert_eq!(effect.latency_samples(), 4_096);
    }

    #[test]
    fn stereo_audio_matches_an_independent_convolution_of_past_spectra() {
        struct Reference {
            frames: Vec<Vec<Complex<f32>>>,
        }
        impl SpectralTransform for Reference {
            fn transform(&mut self, bins: &mut [Complex<f32>], size: usize, sr: f32) {
                assert_eq!((size, sr), (2_048, 40_960.0));
                self.frames.push(bins.to_vec());
                let mut sum = vec![Complex::<f64>::default(); size / 2 + 1];
                // Direct convolution with the flat 0.55-second decay, without tail recurrence.
                for (lag, frame) in self.frames.iter().rev().enumerate() {
                    let gain = 10.0_f64.powf(-3.0 * lag as f64 * 0.025 / 0.55);
                    for bin in 1..=size / 2 {
                        sum[bin].re += frame[bin].re as f64 * gain;
                        sum[bin].im += frame[bin].im as f64 * gain;
                    }
                }
                for bin in 1..=size / 2 {
                    bins[bin] = Complex::new(sum[bin].re as f32, sum[bin].im as f32);
                    bins[size - bin] = bins[bin].conj();
                }
            }
        }
        let mut effect = configured_reverb(2.0);
        effect.set_param(0, 1.0);
        effect.set_param(1, 50.0);
        effect.set_sample_rate(40_960.0);
        let mut left = SpectralEngine::new(40_960.0);
        let mut right = SpectralEngine::new(40_960.0);
        let mut left_ref = Reference { frames: Vec::new() };
        let mut right_ref = Reference { frames: Vec::new() };
        let mut energy = [0.0; 2];
        for sample in 0..16_384 {
            let input = match sample / 4_096 {
                0 => ((sample as f32 * 0.17).sin() * 0.125 + 0.025, 0.0),
                1 => (0.0, (sample as f32 * 0.31).cos() * 0.0625 - 0.0125),
                2 => (0.0, 0.0),
                _ => (
                    (sample as f32 * 0.11).cos() * 0.125,
                    (sample as f32 * 0.27).sin() * 0.25,
                ),
            };
            let expected = (
                left.process_sample(input.0, &mut left_ref),
                right.process_sample(input.1, &mut right_ref),
            );
            let actual = effect.process_sample(input.0, input.1);
            assert!(
                (actual.0 - expected.0).abs() < 2e-6 && (actual.1 - expected.1).abs() < 2e-6,
                "sample {sample}: {actual:?} vs {expected:?}"
            );
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
    }

    #[test]
    fn host_reset_clears_stereo_tails_and_preserves_pending_fft() {
        let mut used = configured_reverb(0.0);
        let mut warm_energy = [0.0; 2];
        for sample in 0..16_384 {
            let output = used.process_sample(
                (sample as f32 * 0.23).sin() * 0.25,
                (sample as f32 * 0.31).cos() * 0.125,
            );
            warm_energy[0] += output.0 * output.0;
            warm_energy[1] += output.1 * output.1;
        }
        assert!(warm_energy.into_iter().all(|e| e > 0.01));
        used.set_param(2, 1.0);
        used.reset();
        assert_eq!(used.latency_samples(), 1_024);
        let mut fresh = configured_reverb(0.0);
        for _ in 0..1_024 {
            fresh.process_sample(0.0, 0.0);
        }
        fresh.set_param(2, 1.0);
        fresh.reset();
        let mut energy = [0.0; 2];
        for sample in 0..16_384 {
            let input = (
                (sample as f32 * 0.11).cos() * 0.125,
                (sample as f32 * 0.27).sin() * 0.25,
            );
            let actual = used.process_sample(input.0, input.1);
            assert_eq!(actual, fresh.process_sample(input.0, input.1));
            energy[0] += actual.0 * actual.0;
            energy[1] += actual.1 * actual.1;
        }
        assert!(energy.into_iter().all(|e| e > 0.01));
    }

    #[test]
    fn sample_rate_configuration_and_host_reset_replay_a_fresh_selected_fft() {
        for index in 0..4 {
            let mut used = configured_reverb(index as f32);
            for sample in 0..16_384 {
                used.process_sample(
                    (sample as f32 * 0.19).sin() * 0.25,
                    (sample as f32 * 0.27).cos() * 0.125,
                );
            }
            used.set_sample_rate(32_000.0);
            used.reset();
            let mut fresh = configured_reverb(index as f32);
            fresh.set_sample_rate(32_000.0);
            fresh.reset();
            assert_eq!(used.sample_rate, 32_000.0);
            assert_eq!(used.latency_samples(), [512, 1_024, 2_048, 4_096][index]);
            assert_eq!(used.engine_r.latency_samples(), used.latency_samples());
            let mut energy = [0.0; 2];
            for sample in 0..16_384 {
                let input = (
                    (sample as f32 * 0.29).cos() * 0.25,
                    (sample as f32 * 0.13).sin() * 0.125,
                );
                let actual = used.process_sample(input.0, input.1);
                assert_eq!(actual, fresh.process_sample(input.0, input.1));
                energy[0] += actual.0 * actual.0;
                energy[1] += actual.1 * actual.1;
            }
            assert!(energy.into_iter().all(|e| e > 0.01));
        }
    }

    fn drive(e: &mut SpectralReverbEffect, n: usize, src: impl Fn(usize) -> f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = src(i);
                e.process_sample(x, x).0
            })
            .collect()
    }

    #[test]
    fn parameters_count_is_three() {
        assert_eq!(SpectralReverbEffect::default().parameters().len(), 3);
    }

    #[test]
    fn silence_in_eventually_silence_out() {
        // Reverb has an indefinite tail by design, so the FIRST samples after
        // input dies aren't zero -- we drive a long silence and check that
        // the tail decays toward zero over Time-many seconds. With Time=0.1 s
        // (the minimum) and ~50 ms of "pulse" input, after ~1 s of silence
        // the tail should be very quiet.
        let mut e = SpectralReverbEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 0.1); // Time = 100 ms (slot 0)
        e.set_param(1, 50.0); // Tone = neutral (slot 1)
        let sr = 48_000.0;
        let burst = (0.050 * sr) as usize;
        let tail = (1.0 * sr) as usize; // 1 s of silence
        let out = drive(&mut e, burst + tail, |i| if i < burst { 0.5 } else { 0.0 });
        // Last 100 ms of tail should be very quiet (well below burst level).
        let final_100ms = &out[out.len() - (0.1 * sr) as usize..];
        let max_late: f32 = final_100ms
            .iter()
            .cloned()
            .fold(0.0_f32, |a, x| a.max(x.abs()));
        assert!(
            max_late < 0.05,
            "100 ms reverb should be ~quiet 1 s after burst; got max={max_late}"
        );
    }

    #[test]
    fn reverb_tail_outlives_input() {
        // Drive a 50 ms burst at Time = 2 s; observe non-zero output 200 ms
        // after the burst ends.
        let mut e = SpectralReverbEffect::default();
        e.set_param(2, 1.0); // FFT = 1024 (slot 2)
        e.set_param(0, 2.0); // Time = 2 s (slot 0)
        let sr = 48_000.0_f32;
        let burst = (0.050 * sr) as usize;
        let tail = (0.200 * sr) as usize;
        let out = drive(&mut e, burst + tail, |i| {
            if i < burst {
                (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin() * 0.5
            } else {
                0.0
            }
        });
        let tail_energy: f32 = out[burst + 2 * e.latency_samples()..]
            .iter()
            .map(|x| x * x)
            .sum();
        assert!(
            tail_energy > 0.001,
            "expected non-zero reverb tail; got {tail_energy}"
        );
    }
}
