use super::{AudioTriggerSettings, DetectorInput, audio_trigger::AudioFilter};

#[cfg(test)]
mod playback_tests;

pub const FOLLOWER_ATTACK_MAX_MS: f32 = 1000.0;
pub const FOLLOWER_HOLD_MAX_MS: f32 = 2000.0;
pub const FOLLOWER_RELEASE_MAX_MS: f32 = 5000.0;
pub const FOLLOWER_GAIN_DB: f32 = 24.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FollowerMode {
    #[default]
    Follow,
    Add,
    Multiply,
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FollowerRoute {
    pub enabled: bool,
    pub mode: FollowerMode,
    pub amount: f32,
}
impl Default for FollowerRoute {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: FollowerMode::Follow,
            amount: 1.0,
        }
    }
}
impl FollowerRoute {
    pub fn normalized(self) -> Self {
        Self {
            amount: if self.amount.is_finite() {
                self.amount.clamp(
                    if self.mode == FollowerMode::Add {
                        -2.0
                    } else {
                        0.0
                    },
                    2.0,
                )
            } else {
                1.0
            },
            ..self
        }
    }
    pub fn combine(self, curve: f32, envelope: f32) -> f32 {
        if !self.enabled {
            return curve;
        }
        let envelope = self.amount * envelope;
        match self.mode {
            FollowerMode::Follow => envelope,
            FollowerMode::Add => curve + envelope,
            FollowerMode::Multiply => curve * envelope,
        }
        .clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FollowerConfig {
    pub detector: AudioTriggerSettings,
    pub attack_ms: f32,
    pub hold_ms: f32,
    pub release_ms: f32,
    pub gain_db: f32,
    pub routes: [FollowerRoute; 4],
}
impl Default for FollowerConfig {
    fn default() -> Self {
        Self {
            detector: AudioTriggerSettings::default(),
            attack_ms: 5.0,
            hold_ms: 0.0,
            release_ms: 100.0,
            gain_db: 0.0,
            routes: [FollowerRoute::default(); 4],
        }
    }
}
impl FollowerConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn normalized(self) -> Self {
        let bound = |value: f32, lo: f32, hi: f32, default: f32| {
            if value.is_finite() {
                value.clamp(lo, hi)
            } else {
                default
            }
        };
        Self {
            detector: self.detector.normalized(),
            attack_ms: bound(self.attack_ms, 0.0, FOLLOWER_ATTACK_MAX_MS, 5.0),
            hold_ms: bound(self.hold_ms, 0.0, FOLLOWER_HOLD_MAX_MS, 0.0),
            release_ms: bound(self.release_ms, 0.0, FOLLOWER_RELEASE_MAX_MS, 100.0),
            gain_db: bound(self.gain_db, -FOLLOWER_GAIN_DB, FOLLOWER_GAIN_DB, 0.0),
            routes: self.routes.map(FollowerRoute::normalized),
        }
    }
    pub fn mask(self) -> u8 {
        self.routes
            .iter()
            .enumerate()
            .fold(0, |mask, (slot, route)| {
                mask | if route.enabled { 1 << slot } else { 0 }
            })
    }
    pub fn request_key(self, slot: usize) -> u8 {
        let route = self.routes[slot];
        if !route.enabled {
            return 0;
        }
        let mode = match route.mode {
            FollowerMode::Follow => 1,
            FollowerMode::Add => 2,
            FollowerMode::Multiply => 3,
        };
        mode | if self.detector.input == DetectorInput::Sidechain {
            4
        } else {
            0
        }
    }
    fn same_response(self, other: Self) -> bool {
        self.detector == other.detector
            && self.attack_ms == other.attack_ms
            && self.hold_ms == other.hold_ms
            && self.release_ms == other.release_ms
            && self.gain_db == other.gain_db
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowerReading {
    pub level: f32,
    pub envelope: f32,
    pub config: FollowerConfig,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FollowerRuntime {
    pub(super) config: FollowerConfig,
    filters: [AudioFilter; 2],
    sample_rate: f64,
    attack_gain: f64,
    release_gain: f64,
    gain: f64,
    hold_samples: u32,
    hold_remaining: u32,
    envelope: f64,
    level: f32,
    active: bool,
}
impl Default for FollowerRuntime {
    fn default() -> Self {
        Self {
            config: FollowerConfig::default(),
            filters: [AudioFilter::default(); 2],
            sample_rate: 0.0,
            attack_gain: 1.0,
            release_gain: 1.0,
            gain: 1.0,
            hold_samples: 0,
            hold_remaining: 0,
            envelope: 0.0,
            level: 0.0,
            active: false,
        }
    }
}
fn samples(value: f64) -> u32 {
    value.ceil().clamp(0.0, u32::MAX as f64) as u32
}
impl FollowerRuntime {
    pub(super) fn reset(&mut self) {
        for filter in &mut self.filters {
            filter.reset();
        }
        self.envelope = 0.0;
        self.level = 0.0;
        self.hold_remaining = 0;
    }
    pub(super) fn prepare(&mut self, config: FollowerConfig, sr: f64) -> u8 {
        let config = config.normalized();
        let old_mask = self.config.mask();
        let new_mask = config.mask();
        let mut changed = 0;
        for slot in 0..4 {
            if config.routes[slot] != self.config.routes[slot]
                && (config.routes[slot].enabled || self.config.routes[slot].enabled)
            {
                changed |= 1 << slot;
            }
        }
        if !config.same_response(self.config) {
            changed |= old_mask | new_mask;
        }
        if !sr.is_finite() || sr <= 0.0 || !(sr as f32).is_finite() || sr as f32 <= 0.0 {
            self.active = false;
            return changed;
        }
        if new_mask == 0 {
            self.active = false;
            self.config = config;
            return changed;
        }
        if old_mask == 0 || config.detector.input != self.config.detector.input {
            self.reset();
        }
        if self.sample_rate > 0.0 && self.sample_rate != sr {
            self.hold_remaining = samples(self.hold_remaining as f64 * sr / self.sample_rate);
        }
        if self.hold_remaining > 0 && config.hold_ms != self.config.hold_ms {
            let elapsed_ms =
                (self.config.hold_ms as f64 - self.hold_remaining as f64 / sr * 1000.0).max(0.0);
            self.hold_remaining =
                samples((config.hold_ms as f64 - elapsed_ms).max(0.0) * sr * 0.001);
        }
        let coefficient = |ms: f32| {
            if ms == 0.0 {
                1.0
            } else {
                -(-1.0 / (ms as f64 * sr * 0.001)).exp_m1()
            }
        };
        if old_mask == 0 || self.sample_rate != sr || config.attack_ms != self.config.attack_ms {
            self.attack_gain = coefficient(config.attack_ms);
        }
        if old_mask == 0 || self.sample_rate != sr || config.release_ms != self.config.release_ms {
            self.release_gain = coefficient(config.release_ms);
        }
        if old_mask == 0 || config.gain_db != self.config.gain_db || self.sample_rate == 0.0 {
            self.gain = 10.0_f64.powf(config.gain_db as f64 * 0.05);
        }
        self.hold_samples = samples(config.hold_ms as f64 * sr * 0.001);
        for filter in &mut self.filters {
            filter.configure(config.detector, sr);
        }
        self.config = config;
        self.sample_rate = sr;
        self.active = true;
        changed
    }
    pub(super) fn next(&mut self, main: [f32; 2], sidechain: [f32; 2]) {
        if !self.active {
            return;
        }
        let input = match self.config.detector.input {
            DetectorInput::Main => main,
            DetectorInput::Sidechain => sidechain,
        };
        let mut peak = 0.0_f64;
        for (sample, filter) in input.into_iter().zip(&mut self.filters) {
            let sample = if sample.is_finite() {
                sample.clamp(-64.0, 64.0)
            } else {
                0.0
            };
            let filtered = filter.process(sample);
            if filtered.is_finite() {
                peak = peak.max(filtered.abs() as f64);
            }
        }
        peak *= self.gain;
        self.level = peak.min(f32::MAX as f64) as f32;
        let target = if peak >= self.filters[0].level_floor as f64 {
            peak.min(1.0)
        } else {
            0.0
        };
        if target >= self.envelope {
            self.envelope += self.attack_gain * (target - self.envelope);
            self.hold_remaining = self.hold_samples;
        } else if self.hold_remaining > 0 {
            self.hold_remaining -= 1;
        } else {
            self.envelope += self.release_gain * (target - self.envelope);
        }
        if self.envelope < 1e-30 {
            self.envelope = 0.0;
        }
    }
    pub(super) fn value(&self) -> f32 {
        self.envelope as f32
    }
    pub(super) fn reading(&self) -> Option<FollowerReading> {
        self.active.then_some(FollowerReading {
            level: self.level,
            envelope: self.value(),
            config: self.config,
        })
    }
    pub(super) fn combine(&self, slot: usize, curve: f32) -> f32 {
        self.config.routes[slot].combine(curve, self.value())
    }
}

impl super::ModulationState {
    pub fn follower_reading(&self) -> Option<FollowerReading> {
        self.follower.reading()
    }
    pub fn process_modulation_sample(
        &mut self,
        config: &super::TrackModulation,
        effect: &mut impl crate::effects::Effect,
        input: [f32; 2],
        sidechain: [f32; 2],
    ) {
        self.follower.next(input, sidechain);
        self.process_smoothing_sample(config, effect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant() -> FollowerConfig {
        let mut cfg = FollowerConfig {
            attack_ms: 0.0,
            release_ms: 0.0,
            ..FollowerConfig::default()
        };
        cfg.routes[0].enabled = true;
        cfg
    }
    #[test]
    fn linked_stereo_peak_cannot_cancel_and_missing_sidechain_is_silence() {
        let mut runtime = FollowerRuntime::default();
        let mut cfg = instant();
        runtime.prepare(cfg, 48000.0);
        runtime.next([0.3, -0.7], [0.0; 2]);
        assert_eq!(runtime.value(), 0.7);
        cfg.detector.input = DetectorInput::Sidechain;
        runtime.prepare(cfg, 48000.0);
        runtime.next([1.0; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);
        runtime.next([0.0; 2], [-0.5, 0.5]);
        assert_eq!(runtime.value(), 0.5);
    }

    #[test]
    fn holding_at_an_unchanged_fractional_rate_does_not_round_the_count_up() {
        let mut config = instant();
        config.hold_ms = 0.145;
        let rate = 48000.1;
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(config, rate);
        runtime.next([0.75; 2], [0.0; 2]);
        assert_eq!(runtime.hold_remaining, 7);
        runtime.prepare(config, rate);
        for _ in 0..7 {
            runtime.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), 0.75);
        }
        runtime.next([0.0; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);
    }
    #[test]
    fn attack_and_release_are_sample_rate_independent_time_constants() {
        for sr in [1000.0, 48000.0, 96000.0] {
            let mut cfg = instant();
            cfg.attack_ms = 10.0;
            cfg.release_ms = 20.0;
            let mut runtime = FollowerRuntime::default();
            runtime.prepare(cfg, sr);
            for _ in 0..(sr * 0.01) as usize {
                runtime.next([1.0; 2], [0.0; 2]);
            }
            let attacked = runtime.value();
            assert!((attacked - (1.0 - (-1.0_f32).exp())).abs() < 1e-6);
            for _ in 0..(sr * 0.02) as usize {
                runtime.next([0.0; 2], [0.0; 2]);
            }
            assert!((runtime.value() - attacked * (-1.0_f32).exp()).abs() < 1e-6);
        }
    }
    #[test]
    fn hold_delays_release_exactly_and_rate_changes_preserve_its_remaining_time() {
        let mut cfg = instant();
        cfg.hold_ms = 5.0;
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 1000.0);
        runtime.next([1.0; 2], [0.0; 2]);
        for _ in 0..2 {
            runtime.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), 1.0);
        }
        runtime.prepare(cfg, 2000.0);
        for _ in 0..6 {
            runtime.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), 1.0);
        }
        runtime.next([0.0; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);
    }
    #[test]
    fn threshold_gates_the_target_after_gain_without_skipping_hold_or_release() {
        let mut cfg = instant();
        cfg.detector.level_db = Some(-12.0);
        cfg.gain_db = 6.0206;
        cfg.hold_ms = 2.0;
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 1000.0);
        runtime.next([0.1; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);
        runtime.next([0.2; 2], [0.0; 2]);
        assert!((runtime.value() - 0.4).abs() < 1e-6);
        for _ in 0..2 {
            runtime.next([0.01; 2], [0.0; 2]);
            assert!((runtime.value() - 0.4).abs() < 1e-6);
        }
        runtime.next([0.01; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);
    }
    #[test]
    fn route_math_is_normalized_and_only_add_accepts_negative_amounts() {
        for (mode, amount, expected) in [
            (FollowerMode::Follow, 1.0, 0.25),
            (FollowerMode::Add, 1.0, 0.85),
            (FollowerMode::Add, -2.0, 0.1),
            (FollowerMode::Multiply, 2.0, 0.3),
        ] {
            let route = FollowerRoute {
                enabled: true,
                mode,
                amount,
            };
            assert!((route.combine(0.6, 0.25) - expected).abs() < 1e-6);
        }
        assert_eq!(
            FollowerRoute {
                enabled: true,
                amount: -1.0,
                ..FollowerRoute::default()
            }
            .normalized()
            .amount,
            0.0
        );
        assert_eq!(FollowerRoute::default().combine(0.6, 1.0), 0.6);
    }
    #[test]
    fn response_edits_while_disabled_refresh_all_cached_coefficients_on_reenable() {
        let mut cfg = instant();
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 1000.0);
        runtime.next([0.5; 2], [0.0; 2]);
        cfg.routes[0].enabled = false;
        runtime.prepare(cfg, 1000.0);
        cfg.attack_ms = 10.0;
        cfg.release_ms = 20.0;
        cfg.gain_db = 6.0206;
        runtime.prepare(cfg, 1000.0);
        cfg.routes[0].enabled = true;
        runtime.prepare(cfg, 1000.0);
        let mut fresh = FollowerRuntime::default();
        fresh.prepare(cfg, 1000.0);
        for _ in 0..10 {
            runtime.next([0.5; 2], [0.0; 2]);
            fresh.next([0.5; 2], [0.0; 2]);
            assert_eq!(runtime.value(), fresh.value());
        }
        for _ in 0..20 {
            runtime.next([0.0; 2], [0.0; 2]);
            fresh.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), fresh.value());
        }
    }

    #[test]
    fn adding_routes_keeps_history_while_reenabling_and_input_changes_start_fresh() {
        let mut cfg = instant();
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 48000.0);
        runtime.next([0.75; 2], [0.0; 2]);
        cfg.routes[1].enabled = true;
        assert_eq!(runtime.prepare(cfg, 48000.0), 2);
        assert_eq!(runtime.value(), 0.75);
        cfg.routes[0].enabled = false;
        cfg.routes[1].enabled = false;
        runtime.prepare(cfg, 48000.0);
        cfg.routes[0].enabled = true;
        runtime.prepare(cfg, 48000.0);
        assert_eq!(runtime.value(), 0.0);
        runtime.next([0.75; 2], [0.0; 2]);
        cfg.detector.input = DetectorInput::Sidechain;
        runtime.prepare(cfg, 48000.0);
        assert_eq!(runtime.value(), 0.0);
    }

    #[test]
    fn every_shared_response_edit_refreshes_old_and_new_enabled_routes() {
        let mut cfg = instant();
        cfg.routes[2].enabled = true;
        let mut runtime = FollowerRuntime::default();
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b0101);
        assert_eq!(runtime.prepare(cfg, 48000.0), 0);
        cfg.detector.input = DetectorInput::Sidechain;
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b0101);
        cfg.attack_ms = 4.0;
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b0101);
        cfg.hold_ms = 8.0;
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b0101);
        cfg.release_ms = 12.0;
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b0101);
        cfg.gain_db = 2.0;
        cfg.routes[2].enabled = false;
        cfg.routes[3].enabled = true;
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b1101);
        assert_eq!(cfg.request_key(0), 5);
        assert_eq!(cfg.request_key(2), 0);
        cfg.routes[3].mode = FollowerMode::Multiply;
        assert_eq!(cfg.request_key(3), 7);
        assert_eq!(runtime.prepare(cfg, 48000.0), 0b1000);
    }

    #[test]
    fn adjusting_hold_after_a_rate_change_counts_only_time_left_since_the_peak() {
        let mut cfg = instant();
        cfg.hold_ms = 6.0;
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 1000.0);
        runtime.next([0.8; 2], [0.0; 2]);
        for _ in 0..2 {
            runtime.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), 0.8);
        }
        cfg.hold_ms = 9.0;
        runtime.prepare(cfg, 2000.0);
        for _ in 0..14 {
            runtime.next([0.0; 2], [0.0; 2]);
            assert_eq!(runtime.value(), 0.8);
        }
        runtime.next([0.0; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.0);

        runtime.next([1.0; 2], [0.0; 2]);
        for _ in 0..18 {
            runtime.next([0.0; 2], [0.0; 2]);
        }
        cfg.hold_ms = 12.0;
        runtime.prepare(cfg, 2000.0);
        runtime.next([0.0; 2], [0.0; 2]);
        assert_eq!(
            runtime.value(),
            0.0,
            "a hold edit must not restart an expired hold"
        );
    }

    #[test]
    fn active_coefficient_changes_take_effect_without_resetting_the_envelope() {
        let mut cfg = instant();
        cfg.attack_ms = 10.0;
        cfg.release_ms = 20.0;
        cfg.gain_db = 6.0206;
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 1000.0);
        for _ in 0..10 {
            runtime.next([0.25; 2], [0.0; 2]);
        }
        let before = runtime.value();
        assert!((before - 0.5 * (1.0 - (-1.0_f32).exp())).abs() < 1e-6);
        runtime.prepare(cfg, 2000.0);
        assert_eq!(runtime.value(), before);
        for _ in 0..20 {
            runtime.next([0.25; 2], [0.0; 2]);
        }
        assert!((runtime.value() - 0.5 * (1.0 - (-2.0_f32).exp())).abs() < 1e-6);
        let before_release = runtime.value();
        for _ in 0..40 {
            runtime.next([0.0; 2], [0.0; 2]);
        }
        assert!((runtime.value() - before_release * (-1.0_f32).exp()).abs() < 1e-6);

        cfg.attack_ms = 0.0;
        runtime.prepare(cfg, 2000.0);
        runtime.next([0.4; 2], [0.0; 2]);
        assert!((runtime.value() - 0.8).abs() < 1e-6);
        cfg.gain_db = 0.0;
        runtime.prepare(cfg, 2000.0);
        runtime.next([0.25; 2], [0.0; 2]);
        assert_eq!(runtime.reading().unwrap().level, 0.25);
        assert!(runtime.value() > 0.25);
        cfg.release_ms = 0.0;
        runtime.prepare(cfg, 2000.0);
        runtime.next([0.25; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 0.25);
    }

    #[test]
    fn unusable_detector_rates_suspend_updates_until_a_valid_rate_returns() {
        let cfg = instant();
        for rate in [
            f64::NAN,
            f64::INFINITY,
            f64::MAX,
            -48000.0,
            0.0,
            f64::MIN_POSITIVE,
        ] {
            let mut runtime = FollowerRuntime::default();
            runtime.prepare(cfg, 48000.0);
            runtime.next([0.75; 2], [0.0; 2]);
            assert_eq!(runtime.prepare(cfg, rate), 0);
            runtime.next([0.5; 2], [0.0; 2]);
            assert!(runtime.reading().is_none(), "rate {rate}");
            assert_eq!(runtime.value(), 0.75);
            runtime.prepare(cfg, 48000.0);
            runtime.next([0.5; 2], [0.0; 2]);
            assert_eq!(runtime.reading().unwrap().envelope, 0.5);
        }
    }

    #[test]
    fn follower_silences_very_small_envelopes_before_delivery() {
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(instant(), 48000.0);
        runtime.next([1e-31; 2], [0.0; 2]);
        let reading = runtime.reading().unwrap();
        assert!(reading.level > 0.0);
        assert_eq!(reading.envelope, 0.0);
    }

    #[test]
    fn the_envelope_floor_preserves_its_exact_boundary_while_held() {
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(instant(), 48000.0);
        runtime.envelope = 1e-30;
        runtime.hold_remaining = 1;
        runtime.next([0.0; 2], [0.0; 2]);
        assert_eq!(runtime.value(), 1e-30_f32);
    }
    #[test]
    fn defaults_round_trip_and_invalid_settings_samples_or_rates_cannot_make_nan() {
        assert_eq!(
            serde_json::from_str::<FollowerConfig>("{}").unwrap(),
            FollowerConfig::default()
        );
        let mut cfg = instant();
        cfg.attack_ms = f32::NAN;
        cfg.release_ms = f32::INFINITY;
        cfg.gain_db = 1000.0;
        let cfg = cfg.normalized();
        assert_eq!(cfg.attack_ms, 5.0);
        assert_eq!(cfg.release_ms, 100.0);
        assert_eq!(cfg.gain_db, 24.0);
        let mut runtime = FollowerRuntime::default();
        runtime.prepare(cfg, 48000.0);
        for input in [[f32::NAN, f32::INFINITY], [f32::MAX, -f32::MAX], [0.0; 2]] {
            runtime.next(input, input);
            assert!(runtime.value().is_finite());
        }
        let value = runtime.value();
        runtime.prepare(cfg, 0.0);
        runtime.next([1.0; 2], [1.0; 2]);
        assert_eq!(runtime.value(), value);
    }
}
