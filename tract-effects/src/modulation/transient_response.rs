use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransientAlgorithm {
    #[default]
    Envelope,
    Peak,
    Rms,
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TransientSettings {
    pub algorithm: TransientAlgorithm,
    pub detail: f32,
}
impl Default for TransientSettings {
    fn default() -> Self {
        Self {
            algorithm: TransientAlgorithm::Envelope,
            detail: 50.0,
        }
    }
}
impl TransientSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn normalized(self) -> Self {
        Self {
            detail: if self.detail.is_finite() {
                self.detail.clamp(0.0, 100.0)
            } else {
                50.0
            },
            ..self
        }
    }
    pub(super) fn window_factor(self) -> f32 {
        2.0_f32.powf((50.0 - self.normalized().detail) / 25.0)
    }
}
impl precise::DetectorCoefficients {
    pub(super) fn for_response(sr: f32, response: TransientSettings) -> Self {
        if response.is_default() {
            return Self::new(sr);
        }
        let factor = response.window_factor();
        match response.algorithm {
            TransientAlgorithm::Envelope => Self {
                fast_a: one_pole_coef(TRANSIENT_FAST_ATTACK_MS * factor, sr),
                fast_r: one_pole_coef(TRANSIENT_FAST_RELEASE_MS * factor, sr),
                slow_a: one_pole_coef(TRANSIENT_SLOW_ATTACK_MS * factor, sr),
                slow_r: one_pole_coef(TRANSIENT_SLOW_RELEASE_MS * factor, sr),
            },
            TransientAlgorithm::Peak => Self {
                fast_r: one_pole_coef(20.0 * factor, sr),
                ..Self::default()
            },
            TransientAlgorithm::Rms => {
                let fast = one_pole_coef(2.0 * factor, sr);
                let slow = one_pole_coef(200.0 * factor, sr);
                Self {
                    fast_a: fast,
                    fast_r: fast,
                    slow_a: slow,
                    slow_r: slow,
                }
            }
        }
    }
}
impl TransientDetector {
    #[expect(
        clippy::too_many_arguments,
        reason = "Extends the legacy detector inputs with its response settings."
    )]
    pub(super) fn process_response(
        &mut self,
        input: f32,
        threshold: f32,
        hold_ms: f32,
        sr: f32,
        coefficients: precise::DetectorCoefficients,
        floor: f32,
        response: TransientSettings,
    ) -> bool {
        if response.algorithm == TransientAlgorithm::Envelope {
            return self.process_cached(input, threshold, hold_ms, sr, coefficients, floor);
        }
        let target = if input.is_finite() {
            input.clamp(-64.0, 64.0).abs()
        } else {
            0.0
        };
        let crossed = match response.algorithm {
            TransientAlgorithm::Peak => {
                let reference = self.slow_env;
                self.fast_env = target;
                self.slow_env = target.max(reference * coefficients.fast_r);
                target >= TRANSIENT_NOISE_FLOOR && target >= floor && target > reference * threshold
            }
            TransientAlgorithm::Rms => {
                let power = target * target;
                self.fast_env = power + (self.fast_env - power) * coefficients.fast_a;
                self.slow_env = power + (self.slow_env - power) * coefficients.slow_a;
                self.slow_env >= TRANSIENT_NOISE_FLOOR * TRANSIENT_NOISE_FLOOR
                    && self.fast_env >= floor * floor
                    && self.fast_env > self.slow_env * (threshold * threshold)
            }
            TransientAlgorithm::Envelope => false,
        };
        if self.refractory > 0 {
            self.refractory -= 1;
            return false;
        }
        if crossed {
            self.refractory = ((hold_ms.max(0.0) * 0.001 * sr) as u32).max(1);
        }
        crossed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(algorithm: TransientAlgorithm, detail: f32) -> TrackModulation {
        TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            },
            detector_response: TransientSettings { algorithm, detail },
            ..TrackModulation::default()
        }
    }
    #[test]
    fn default_envelope_response_is_sample_exact_with_the_legacy_detector() {
        for sr in [1000.0, 44100.0, 48000.0, 96000.0] {
            let cfg = config(TransientAlgorithm::Envelope, 50.0);
            let mut state = ModulationState::default();
            state.begin_block(&cfg, &[0.0], &[0.0], sr);
            let mut reference = TransientDetector::default();
            for i in 0..12000 {
                let value = if i % 193 < 41 { 0.7 } else { 0.0001 };
                assert_eq!(
                    state.next_precise_trigger([value; 2]),
                    reference.process_sample(value, 1.5, 5.0, sr as f32)
                );
            }
        }
    }
    #[test]
    fn peak_detail_recovers_between_fast_hits_and_silence_never_fires() {
        let count = |detail| {
            let cfg = config(TransientAlgorithm::Peak, detail);
            let mut state = ModulationState::default();
            state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
            let mut count = 0;
            for i in 0..240 {
                count += usize::from(
                    state.next_precise_trigger([if i % 12 == 0 { 0.8 } else { 0.0 }; 2]),
                );
            }
            for _ in 0..100 {
                assert!(!state.next_precise_trigger([1e-8; 2]));
            }
            count
        };
        assert!(count(100.0) > count(0.0));
        assert_eq!(count(100.0), 20);
    }
    #[test]
    fn rms_reading_is_an_amplitude_and_absolute_threshold_still_gates_it() {
        let mut cfg = config(TransientAlgorithm::Rms, 50.0);
        cfg.audio_trigger.level_db = Some(-6.0);
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.0], &[0.0], 48000.0);
        for _ in 0..48000 {
            assert!(!state.next_precise_trigger([0.1; 2]));
        }
        let reading = state.detector_reading().unwrap();
        assert!((reading.level - 0.1).abs() < 1e-5);
        assert_eq!(reading.response, cfg.detector_response);
        let legacy = serde_json::to_string(&TrackModulation::default()).unwrap();
        assert!(!legacy.contains("detector_response"));
        let saved = serde_json::to_string(&cfg).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&saved)
                .unwrap()
                .detector_response,
            cfg.detector_response
        );
    }
    #[test]
    fn zero_frame_response_and_rate_round_trips_do_not_lengthen_hold() {
        let cfg = config(TransientAlgorithm::Peak, 100.0);
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert!(state.next_precise_trigger([0.1; 2]));
        assert!(!state.next_precise_trigger([0.0; 2]));
        let temporary = config(TransientAlgorithm::Rms, 0.0);
        state.begin_block(&temporary, &[], &[], 333.0);
        state.begin_block(&cfg, &[], &[], 1000.0);
        assert_eq!(
            state.detector_reading().unwrap().response,
            cfg.detector_response
        );
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        for value in [0.2, 0.4, 0.8, 1.6] {
            assert!(!state.next_precise_trigger([value; 2]));
        }
        assert!(state.next_precise_trigger([3.2; 2]));
    }
    #[test]
    fn strict_rms_sensitivity_remains_reachable_across_detail_and_audio_sample_rates() {
        for sr in [1000.0, 8000.0, 48000.0, 96000.0] {
            for detail in [0.0, 50.0, 100.0] {
                let mut cfg = config(TransientAlgorithm::Rms, detail);
                cfg.trigger = TriggerSource::TransientSample {
                    threshold: 6.0,
                    hold_ms: 5.0,
                };
                let mut state = ModulationState::default();
                state.begin_block(&cfg, &[0.0], &[0.0], sr);
                assert!(
                    state.next_precise_trigger([1.0; 2]),
                    "sr {sr}, Detail {detail}"
                );
            }
        }
    }
    #[test]
    fn inactive_response_edits_rebuild_the_selected_coefficients_when_reenabled() {
        let mut actual = ModulationState::default();
        let old = config(TransientAlgorithm::Peak, 0.0);
        actual.begin_block(&old, &[0.0], &[0.0], 1000.0);
        actual.next_precise_trigger([0.8; 2]);
        let selected = config(TransientAlgorithm::Rms, 100.0);
        let mut inactive = selected.clone();
        inactive.trigger = TriggerSource::Free;
        actual.begin_block(&inactive, &[0.0], &[0.0], 1000.0);
        let mut reference = ModulationState::default();
        actual.begin_block(&selected, &[0.0], &[0.0], 1000.0);
        reference.begin_block(&selected, &[0.0], &[0.0], 1000.0);
        for i in 0..1000 {
            let value = if i % 17 == 0 { 0.8 } else { 0.0001 };
            assert_eq!(
                actual.next_precise_trigger([value; 2]),
                reference.next_precise_trigger([value; 2])
            );
            assert_eq!(actual.detector_reading(), reference.detector_reading());
        }
    }
    #[test]
    fn algorithm_edits_preserve_hold_detail_edits_preserve_level_and_input_changes_reset_both() {
        let mut cfg = config(TransientAlgorithm::Peak, 100.0);
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert!(state.next_precise_trigger([0.1; 2]));
        assert!(!state.next_precise_trigger([0.0; 2]));
        cfg.detector_response.algorithm = TransientAlgorithm::Rms;
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert_eq!(state.detector_reading().unwrap().level, 0.0);
        for value in [0.2, 0.4, 0.8, 1.6] {
            assert!(!state.next_precise_trigger([value; 2]));
        }
        assert!(state.next_precise_trigger([3.2; 2]));
        let level = state.detector_reading().unwrap().level;
        cfg.detector_response.detail = 25.0;
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert_eq!(state.detector_reading().unwrap().level, level);
        cfg.audio_trigger.input = DetectorInput::Sidechain;
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert_eq!(state.detector_reading().unwrap().level, 0.0);
        assert!(state.next_precise_trigger_with_sidechain([0.0; 2], [1.0; 2]));
    }
    #[test]
    fn response_settings_normalize_and_legacy_block_triggers_ignore_them() {
        for detail in [f32::NAN, f32::INFINITY, -3.0, 200.0] {
            let settings = TransientSettings {
                algorithm: TransientAlgorithm::Peak,
                detail,
            }
            .normalized();
            assert_eq!(
                settings.detail,
                if detail.is_finite() {
                    detail.clamp(0.0, 100.0)
                } else {
                    50.0
                }
            );
        }
        for algorithm in [
            TransientAlgorithm::Envelope,
            TransientAlgorithm::Peak,
            TransientAlgorithm::Rms,
        ] {
            let mut cfg = config(algorithm, 100.0);
            cfg.trigger = TriggerSource::Transient {
                threshold: 1.5,
                hold_ms: 5.0,
            };
            let mut normal = cfg.clone();
            normal.detector_response = TransientSettings::default();
            let mut actual = ModulationState::default();
            let mut reference = ModulationState::default();
            for i in 0..100 {
                let sample = if i % 7 == 0 { 0.8 } else { 0.0 };
                actual.begin_block(&cfg, &[sample; 13], &[sample; 13], 1000.0);
                reference.begin_block(&normal, &[sample; 13], &[sample; 13], 1000.0);
                assert_eq!(actual.trigger_epoch(), reference.trigger_epoch());
            }
        }
    }
    #[test]
    fn inactive_same_family_algorithm_changes_cannot_reinterpret_old_envelopes_as_power() {
        let mut cfg = config(TransientAlgorithm::Envelope, 50.0);
        cfg.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 0.0,
        };
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        state.next_precise_trigger([1.0; 2]);
        state.next_precise_trigger([0.0; 2]);
        cfg.trigger = TriggerSource::TransientSample {
            threshold: f32::NAN,
            hold_ms: 0.0,
        };
        cfg.detector_response.algorithm = TransientAlgorithm::Rms;
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        cfg.trigger = TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 0.0,
        };
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        assert_eq!(state.detector_reading().unwrap().level, 0.0);
        assert!(!state.next_precise_trigger([0.0; 2]));
    }
}
