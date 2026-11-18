//! Detector-only settings and causal one-pole filters.

pub const LEVEL_MIN_DB: f32 = -60.0;
pub const LEVEL_MAX_DB: f32 = 0.0;
pub const FILTER_MIN_HZ: f32 = 20.0;
pub const FILTER_MAX_HZ: f32 = 20000.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DetectorInput {
    #[default]
    Main,
    Sidechain,
}
impl DetectorInput {
    pub fn is_main(&self) -> bool {
        *self == Self::Main
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AudioTriggerSettings {
    #[serde(skip_serializing_if = "DetectorInput::is_main")]
    pub input: DetectorInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highpass_hz: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lowpass_hz: Option<f32>,
}
impl AudioTriggerSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn normalized(self) -> Self {
        let finite = |value: Option<f32>, lo: f32, hi: f32| {
            value.filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi))
        };
        Self {
            input: self.input,
            level_db: finite(self.level_db, LEVEL_MIN_DB, LEVEL_MAX_DB),
            highpass_hz: finite(self.highpass_hz, FILTER_MIN_HZ, FILTER_MAX_HZ),
            lowpass_hz: finite(self.lowpass_hz, FILTER_MIN_HZ, FILTER_MAX_HZ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorReading {
    pub level: f32,
    pub settings: AudioTriggerSettings,
    pub response: super::TransientSettings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct AudioFilter {
    pub settings: AudioTriggerSettings,
    sample_rate: f64,
    hp_gain: f64,
    lp_gain: f64,
    hp_feedback: f64,
    lp_feedback: f64,
    hp_output: f64,
    lp_output: f64,
    last_input: f64,
    last_highpass: f64,
    pub level_floor: f32,
}
impl AudioFilter {
    pub(super) fn configure(&mut self, settings: AudioTriggerSettings, sr: f64) {
        let settings = settings.normalized();
        if settings == self.settings && sr == self.sample_rate {
            return;
        }
        if self.settings.highpass_hz.is_none() && settings.highpass_hz.is_some() {
            self.hp_output = self.last_input;
        }
        if self.settings.lowpass_hz.is_none() && settings.lowpass_hz.is_some() {
            self.lp_output = self.last_highpass;
        }
        if settings.level_db != self.settings.level_db {
            self.level_floor = settings.level_db.map_or(0.0, |db| 10.0_f32.powf(db * 0.05));
        }
        let gain = |cut: Option<f32>| {
            cut.map_or(0.0, |hz| {
                let normalized_hz = hz as f64 / sr;
                if normalized_hz >= 0.5 {
                    1.0
                } else {
                    let g = (std::f64::consts::PI * normalized_hz).tan();
                    g / (1.0 + g)
                }
            })
        };
        if sr != self.sample_rate || settings.highpass_hz != self.settings.highpass_hz {
            self.hp_gain = gain(settings.highpass_hz);
            self.hp_feedback = 1.0 - 2.0 * self.hp_gain;
        }
        if sr != self.sample_rate || settings.lowpass_hz != self.settings.lowpass_hz {
            self.lp_gain = gain(settings.lowpass_hz);
            self.lp_feedback = 1.0 - 2.0 * self.lp_gain;
        }
        self.settings = settings;
        self.sample_rate = sr;
    }
    pub(super) fn reset(&mut self) {
        self.hp_output = 0.0;
        self.lp_output = 0.0;
        self.last_input = 0.0;
        self.last_highpass = 0.0;
    }
    pub(super) fn process(&mut self, input: f32) -> f32 {
        let mut signal = input as f64;
        // Input/output history avoids exposing large integrator states on cutoff edits.
        if self.settings.highpass_hz.is_some() {
            let low = if self.hp_gain == 1.0 {
                signal
            } else {
                self.hp_gain * (signal + self.last_input) + self.hp_feedback * self.hp_output
            };
            self.hp_output = low;
            signal -= low;
        }
        self.last_input = input as f64;
        let highpass = signal;
        if self.settings.lowpass_hz.is_some() {
            let low = if self.lp_gain == 1.0 {
                signal
            } else {
                self.lp_gain * (signal + self.last_highpass) + self.lp_feedback * self.lp_output
            };
            self.lp_output = low;
            signal = low;
        }
        self.last_highpass = highpass;
        signal as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_input_is_implicit_even_when_the_threshold_is_serialized() {
        let settings = AudioTriggerSettings {
            level_db: Some(-18.0),
            ..Default::default()
        };
        let json = serde_json::to_value(settings).unwrap();
        assert_eq!(json["level_db"], -18.0);
        assert!(json.get("input").is_none());
        assert_eq!(
            serde_json::from_value::<AudioTriggerSettings>(json).unwrap(),
            settings
        );
    }

    #[test]
    fn threshold_edits_keep_both_filter_histories_and_reset_discards_them() {
        let mut settings = AudioTriggerSettings {
            highpass_hz: Some(200.0),
            lowpass_hz: Some(2000.0),
            ..Default::default()
        };
        let mut filter = AudioFilter::default();
        filter.configure(settings, 48000.0);
        for sample in [0.25, 0.5, -0.125, 0.0] {
            filter.process(sample);
        }
        let mut reference = filter;
        settings.level_db = Some(-18.0);
        filter.configure(settings, 48000.0);
        for sample in [0.75, 0.0, 0.0, 0.0] {
            assert_eq!(filter.process(sample), reference.process(sample));
        }
        assert!((filter.level_floor - 0.12589254).abs() < 1e-6);
        filter.reset();
        let mut fresh = AudioFilter::default();
        fresh.configure(settings, 48000.0);
        for sample in [0.0, 0.75, 0.125, 0.0] {
            assert_eq!(filter.process(sample), fresh.process(sample));
        }
    }

    #[test]
    fn cutoff_edits_and_sample_rate_changes_redesign_each_enabled_filter() {
        for (highpass_hz, lowpass_hz) in [(Some(1000.0), None), (None, Some(1000.0))] {
            let mut settings = AudioTriggerSettings {
                highpass_hz,
                lowpass_hz,
                ..Default::default()
            };
            let mut filter = AudioFilter::default();
            filter.configure(settings, 48000.0);
            settings.highpass_hz = settings.highpass_hz.map(|hz| hz * 4.0);
            settings.lowpass_hz = settings.lowpass_hz.map(|hz| hz * 4.0);
            for sr in [48000.0, 96000.0] {
                filter.configure(settings, sr);
                let tangent = (std::f64::consts::PI * 4000.0 / sr).tan();
                let one_pole = tangent / (1.0 + tangent);
                let expected = if highpass_hz.is_some() {
                    1.0 - one_pole
                } else {
                    one_pole
                };
                filter.reset();
                assert!((f64::from(filter.process(1.0)) - expected).abs() < 1e-7);
            }
        }
    }
    #[test]
    fn bypass_is_bit_exact_and_settings_round_trip() {
        let mut filter = AudioFilter::default();
        filter.configure(AudioTriggerSettings::default(), 48000.0);
        for value in [0.0, -0.0, 0.1, -0.4, 1.0] {
            assert_eq!(filter.process(value).to_bits(), value.to_bits());
        }
        assert_eq!(
            serde_json::from_str::<AudioTriggerSettings>("{}").unwrap(),
            AudioTriggerSettings::default()
        );
        let settings = AudioTriggerSettings {
            input: DetectorInput::Main,
            level_db: Some(-12.0),
            highpass_hz: Some(100.0),
            lowpass_hz: Some(4000.0),
        };
        assert_eq!(
            serde_json::from_str::<AudioTriggerSettings>(
                &serde_json::to_string(&settings).unwrap()
            )
            .unwrap(),
            settings
        );
        assert_eq!(
            AudioTriggerSettings {
                input: DetectorInput::Main,
                level_db: Some(f32::NAN),
                highpass_hz: Some(-1.0),
                lowpass_hz: Some(f32::INFINITY)
            }
            .normalized(),
            AudioTriggerSettings {
                highpass_hz: Some(20.0),
                ..AudioTriggerSettings::default()
            }
        );
    }
    fn gain(settings: AudioTriggerSettings, hz: f64, sr: f64) -> f64 {
        let mut filter = AudioFilter::default();
        filter.configure(settings, sr);
        let mut input_energy = 0.0;
        let mut output_energy = 0.0;
        for i in 0..sr as usize {
            let input = (std::f64::consts::TAU * hz * i as f64 / sr).sin() as f32;
            let output = filter.process(input);
            if i > sr as usize / 2 {
                input_energy += (input as f64).powi(2);
                output_energy += (output as f64).powi(2);
            }
        }
        (output_energy / input_energy).sqrt()
    }
    #[test]
    fn filters_have_expected_cutoffs_and_passbands_including_near_nyquist() {
        for sr in [44100.0, 48000.0, 96000.0] {
            let hp = AudioTriggerSettings {
                highpass_hz: Some(1000.0),
                ..AudioTriggerSettings::default()
            };
            assert!(gain(hp, 100.0, sr) < 0.11);
            assert!(gain(hp, 4000.0, sr) > 0.95);
            let lp = AudioTriggerSettings {
                lowpass_hz: Some(1000.0),
                ..AudioTriggerSettings::default()
            };
            assert!(gain(lp, 100.0, sr) > 0.99);
            assert!(gain(lp, 8000.0, sr) < 0.15);
        }
        for cutoff in [1000.0, 20000.0] {
            for hp in [false, true] {
                let settings = if hp {
                    AudioTriggerSettings {
                        highpass_hz: Some(cutoff),
                        ..AudioTriggerSettings::default()
                    }
                } else {
                    AudioTriggerSettings {
                        lowpass_hz: Some(cutoff),
                        ..AudioTriggerSettings::default()
                    }
                };
                assert!(
                    (gain(settings, cutoff as f64, 48000.0) - std::f64::consts::FRAC_1_SQRT_2)
                        .abs()
                        < 1e-4
                );
            }
        }
        assert!(
            gain(
                AudioTriggerSettings {
                    highpass_hz: Some(20000.0),
                    ..AudioTriggerSettings::default()
                },
                22500.0,
                48000.0
            ) > 0.9
        );
    }
    #[test]
    fn sub_nyquist_cutoffs_retain_their_requested_frequency() {
        for (sr, cutoff) in [
            (8000.0, 3900.0),
            (32000.0, 15000.0),
            (32000.0, 15900.0),
            (40000.0, 19900.0),
        ] {
            for highpass in [false, true] {
                let settings = AudioTriggerSettings {
                    highpass_hz: highpass.then_some(cutoff),
                    lowpass_hz: (!highpass).then_some(cutoff),
                    ..AudioTriggerSettings::default()
                };
                let measured = gain(settings, cutoff as f64, sr);
                assert!(
                    (measured - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-4,
                    "{sr} Hz sample rate, {cutoff} Hz cutoff, highpass={highpass}: gain={measured}"
                );
            }
        }
    }
    #[test]
    fn cutoffs_at_or_above_nyquist_approach_filter_limits() {
        for sr in [8.0, 8000.0, 32000.0, 40000.0] {
            for highpass in [false, true] {
                let mut filter = AudioFilter::default();
                filter.configure(
                    AudioTriggerSettings {
                        highpass_hz: highpass.then_some(1000.0),
                        lowpass_hz: (!highpass).then_some(1000.0),
                        ..AudioTriggerSettings::default()
                    },
                    32000.0,
                );
                for input in [1.0, -1.0].into_iter().cycle().take(1000) {
                    filter.process(input);
                }
                filter.configure(
                    AudioTriggerSettings {
                        highpass_hz: highpass.then_some(20000.0),
                        lowpass_hz: (!highpass).then_some(20000.0),
                        ..AudioTriggerSettings::default()
                    },
                    sr,
                );
                for input in [0.7, -0.3, 0.2, -0.6].into_iter().cycle().take(10000) {
                    let output = filter.process(input);
                    let expected = if highpass { 0.0 } else { input };
                    assert!(
                        output.is_finite() && (output - expected).abs() < 1e-6,
                        "{sr} Hz sample rate, highpass={highpass}: input={input}, output={output}"
                    );
                }
            }
        }
    }
    #[test]
    fn nyquist_energy_does_not_spike_after_cutoff_or_sample_rate_edits() {
        for cutoff in [15000.0, 15999.0, 20000.0] {
            for next_rate in [32000.0, 48000.0] {
                for highpass in [false, true] {
                    let settings = |cutoff| AudioTriggerSettings {
                        highpass_hz: highpass.then_some(cutoff),
                        lowpass_hz: (!highpass).then_some(cutoff),
                        ..AudioTriggerSettings::default()
                    };
                    let mut filter = AudioFilter::default();
                    filter.configure(settings(cutoff), 32000.0);
                    for input in [1.0, -1.0].into_iter().cycle().take(32000) {
                        filter.process(input);
                    }
                    filter.configure(settings(1000.0), next_rate);
                    for input in [1.0, -1.0].into_iter().cycle().take(1000) {
                        let output = filter.process(input);
                        assert!(
                            output.is_finite() && output.abs() <= 2.0,
                            "from {cutoff} Hz at 32 kHz to 1 kHz at {next_rate} Hz, highpass={highpass}: output={output}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn enabling_filters_seeds_dc_history_and_low_sample_rates_stay_finite() {
        let mut filter = AudioFilter::default();
        filter.configure(AudioTriggerSettings::default(), 48000.0);
        for _ in 0..100 {
            assert_eq!(filter.process(0.7), 0.7);
        }
        filter.configure(
            AudioTriggerSettings {
                highpass_hz: Some(500.0),
                ..AudioTriggerSettings::default()
            },
            48000.0,
        );
        assert!(filter.process(0.7).abs() < 1e-12);
        filter.configure(
            AudioTriggerSettings {
                highpass_hz: Some(20000.0),
                lowpass_hz: Some(20000.0),
                ..AudioTriggerSettings::default()
            },
            8.0,
        );
        for _ in 0..100 {
            assert!(filter.process(0.7).is_finite());
        }
    }
}
