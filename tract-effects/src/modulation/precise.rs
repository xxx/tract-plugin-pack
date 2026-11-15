use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct DetectorCoefficients {
    pub fast_a: f32,
    pub fast_r: f32,
    pub slow_a: f32,
    pub slow_r: f32,
}
impl DetectorCoefficients {
    pub(super) fn new(sr: f32) -> Self {
        Self {
            fast_a: one_pole_coef(TRANSIENT_FAST_ATTACK_MS, sr),
            fast_r: one_pole_coef(TRANSIENT_FAST_RELEASE_MS, sr),
            slow_a: one_pole_coef(TRANSIENT_SLOW_ATTACK_MS, sr),
            slow_r: one_pole_coef(TRANSIENT_SLOW_RELEASE_MS, sr),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PreciseTrigger {
    source: TriggerSource,
    family: u8,
    active: bool,
    sample_rate: f64,
    phase: f64,
    pending: bool,
    detector: TransientDetector,
    coefficients: DetectorCoefficients,
    coefficient_rate: f32,
}
impl PreciseTrigger {
    pub(super) fn active(&self) -> bool {
        self.active
    }
    pub(super) fn reset(&mut self) {
        self.phase = 0.0;
        self.pending = false;
        self.detector.reset();
    }
    pub(super) fn prepare(&mut self, source: TriggerSource, sr: f64) {
        let family = match source {
            TriggerSource::FreeHzSample { .. } => 1,
            TriggerSource::TransientSample { .. } => 2,
            _ => 0,
        };
        if family != self.family {
            self.family = family;
            self.phase = 0.0;
            self.pending = false;
            self.detector.reset();
        }
        self.source = source;
        self.active = sr.is_finite()
            && sr > 0.0
            && (sr as f32).is_finite()
            && (sr as f32) > 0.0
            && match source {
                TriggerSource::FreeHzSample { hz } => hz.is_finite() && hz > 0.0,
                TriggerSource::TransientSample { threshold, hold_ms } => {
                    threshold.is_finite()
                        && threshold > 0.0
                        && hold_ms.is_finite()
                        && hold_ms >= 0.0
                }
                _ => false,
            };
        if !self.active {
            return;
        }
        if matches!(source, TriggerSource::FreeHzSample { .. }) {
            if self.sample_rate > 0.0 && self.sample_rate != sr {
                self.phase = self.phase / self.sample_rate * sr;
            }
        } else {
            if self.sample_rate > 0.0 && self.sample_rate != sr {
                self.detector.refractory =
                    (self.detector.refractory as f64 * sr / self.sample_rate).ceil() as u32;
            }
            if self.coefficient_rate != sr as f32 {
                self.coefficients = DetectorCoefficients::new(sr as f32);
                self.coefficient_rate = sr as f32;
            }
        }
        self.sample_rate = sr;
    }
    pub(super) fn next_sample(&mut self, input: [f32; 2]) -> bool {
        if !self.active {
            return false;
        }
        match self.source {
            TriggerSource::FreeHzSample { hz } => {
                // Integrating Hz units keeps integer sample periods exact.
                let fired = self.pending;
                self.phase += hz as f64;
                self.pending = self.phase >= self.sample_rate;
                if self.pending {
                    self.phase = self.phase.rem_euclid(self.sample_rate);
                }
                fired
            }
            TriggerSource::TransientSample { threshold, hold_ms } => {
                let mono = (input[0] + input[1]) * 0.5;
                self.detector.process_cached(
                    if mono.is_finite() { mono } else { 0.0 },
                    threshold,
                    hold_ms,
                    self.sample_rate as f32,
                    self.coefficients,
                )
            }
            _ => false,
        }
    }
}

impl ModulationState {
    pub fn inherit_precise_timing(&mut self, previous: &Self) {
        self.precise = previous.precise;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hz_events(parts: &[usize], sr: f64, hz: f32) -> Vec<usize> {
        let mut runtime = PreciseTrigger::default();
        let mut events = Vec::new();
        let mut cursor = 0;
        let mut part = 0;
        while cursor < (sr * 2.0) as usize {
            let len = parts[part % parts.len()].min((sr * 2.0) as usize - cursor);
            runtime.prepare(TriggerSource::FreeHzSample { hz }, sr);
            for sample in cursor..cursor + len {
                if runtime.next_sample([0.0; 2]) {
                    events.push(sample);
                }
            }
            cursor += len;
            part += 1;
        }
        events
    }

    #[test]
    fn fractional_hz_positions_do_not_depend_on_host_partitions() {
        for sr in [44100.0, 48000.0, 96000.0, 192000.0] {
            for hz in [10.0, 17.3] {
                let one = hz_events(&[1], sr, hz);
                let blocks = hz_events(&[17, 64, 511, 1024], sr, hz);
                assert_eq!(one, blocks);
                for (index, position) in one.iter().enumerate() {
                    let expected = ((index + 1) as f64 * sr / hz as f64).ceil() as usize;
                    assert!(
                        position.abs_diff(expected) <= 1,
                        "{sr}/{hz}: {position}, {expected}"
                    );
                }
                assert!(one[0] > 0);
            }
        }
    }

    #[test]
    fn first_hz_fire_is_after_one_full_period_of_consumed_samples() {
        let mut runtime = PreciseTrigger::default();
        runtime.prepare(TriggerSource::FreeHzSample { hz: 2.0 }, 8.0);
        let events: Vec<_> = (0..10).filter(|_| runtime.next_sample([0.0; 2])).collect();
        assert_eq!(events, [4, 8]);
    }

    #[test]
    fn supra_sample_periods_coalesce_without_losing_fractional_progress_on_rate_edits() {
        let mut runtime = PreciseTrigger::default();
        runtime.prepare(TriggerSource::FreeHzSample { hz: 10.0 }, 8.0);
        assert!(!runtime.next_sample([0.0; 2]));
        assert!(runtime.next_sample([0.0; 2]));
        runtime.prepare(TriggerSource::FreeHzSample { hz: 2.0 }, 8.0);
        let events: Vec<_> = (2..10).filter(|_| runtime.next_sample([0.0; 2])).collect();
        assert_eq!(events, [2, 4, 8]);
        runtime.prepare(TriggerSource::FreeHzSample { hz: 1000.0 }, 8.0);
        runtime.reset();
        for _ in 0..20 {
            runtime.next_sample([0.0; 2]);
        }
        runtime.prepare(TriggerSource::FreeHzSample { hz: 2.0 }, 8.0);
        assert!(runtime.next_sample([0.0; 2]));
        for _ in 0..3 {
            assert!(!runtime.next_sample([0.0; 2]));
        }
        assert!(runtime.next_sample([0.0; 2]));
    }

    #[test]
    fn family_switch_reuses_valid_coefficients_and_rescales_refractory_time() {
        let mut runtime = PreciseTrigger::default();
        runtime.prepare(TriggerSource::FreeHzSample { hz: 2.0 }, 48000.0);
        runtime.prepare(
            TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 50.0,
            },
            48000.0,
        );
        assert!(runtime.next_sample([4.0; 2]));
        let remaining = runtime.detector.refractory;
        runtime.prepare(
            TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 50.0,
            },
            96000.0,
        );
        assert_eq!(runtime.detector.refractory, remaining * 2);
        assert!(!runtime.next_sample([f32::NAN, f32::INFINITY]));
        runtime.reset();
        assert!(runtime.next_sample([4.0; 2]));
        runtime.prepare(
            TriggerSource::TransientSample {
                threshold: f32::NAN,
                hold_ms: 50.0,
            },
            96000.0,
        );
        assert!(!runtime.active());
    }

    #[test]
    fn transient_positions_and_envelopes_match_the_existing_detector() {
        for sr in [44100.0, 48000.0, 96000.0, 192000.0] {
            let mut precise = PreciseTrigger::default();
            let mut legacy = TransientDetector::default();
            let source = TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            };
            let mut events = Vec::new();
            for sample in 0..sr as usize {
                if sample % 17 == 0 {
                    precise.prepare(source, sr);
                }
                let value = if sample % ((sr * 0.1) as usize) < 64 {
                    1.0
                } else {
                    0.0
                };
                let expected = legacy.process_sample(value, 1.5, 5.0, sr as f32);
                let actual = precise.next_sample([value; 2]);
                assert_eq!(actual, expected, "sample {sample}, {sr}");
                assert_eq!(precise.detector, legacy);
                if actual {
                    events.push(sample);
                }
            }
            assert!(events.len() >= 8);
        }
    }

    #[test]
    fn rate_changes_reset_and_invalid_settings_obey_clock_rules() {
        let mut runtime = PreciseTrigger::default();
        runtime.prepare(TriggerSource::FreeHzSample { hz: 10.0 }, 48000.0);
        for _ in 0..2400 {
            assert!(!runtime.next_sample([0.0; 2]));
        }
        runtime.prepare(TriggerSource::FreeHzSample { hz: f32::NAN }, 48000.0);
        for _ in 0..100 {
            assert!(!runtime.next_sample([0.0; 2]));
        }
        runtime.prepare(TriggerSource::FreeHzSample { hz: 20.0 }, 48000.0);
        for _ in 0..1200 {
            assert!(!runtime.next_sample([0.0; 2]));
        }
        assert!(runtime.next_sample([0.0; 2]));
        runtime.reset();
        for _ in 0..2400 {
            assert!(!runtime.next_sample([0.0; 2]));
        }
        assert!(runtime.next_sample([0.0; 2]));
        runtime.prepare(TriggerSource::FreeHzSample { hz: 1e9 }, 48000.0);
        runtime.reset();
        assert!(!runtime.next_sample([0.0; 2]));
        assert!(runtime.next_sample([0.0; 2]));
        assert!(runtime.next_sample([0.0; 2]));
    }
}
