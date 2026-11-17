use super::*;

pub const MAX_AUDIO_DELAY_MS: f32 = 4.0;
pub const MAX_RETRIGGER_BLEND_MS: f32 = 20.0;
const PENDING_SIZE: usize = 65536;
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TriggerTiming {
    pub audio_delay_ms: f32,
    pub blend_ms: f32,
}
impl TriggerTiming {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    pub fn normalized(self) -> Self {
        self.normalized_with_lookahead(false)
    }
    pub fn normalized_with_lookahead(self, lookahead: bool) -> Self {
        let bound = |v: f32, hi| if v.is_finite() { v.clamp(0.0, hi) } else { 0.0 };
        Self {
            audio_delay_ms: if self.audio_delay_ms.is_finite() {
                self.audio_delay_ms.clamp(
                    if lookahead { -MAX_AUDIO_DELAY_MS } else { 0.0 },
                    MAX_AUDIO_DELAY_MS,
                )
            } else {
                0.0
            },
            blend_ms: bound(self.blend_ms, MAX_RETRIGGER_BLEND_MS),
        }
    }
}
impl TrackModulation {
    pub fn lookahead_samples(&self, sr: f64) -> usize {
        if self.lookahead && matches!(self.trigger, TriggerSource::TransientSample { .. }) {
            crate::lookahead::lookahead_samples(sr)
        } else {
            0
        }
    }
    pub(super) fn timing_active(&self) -> bool {
        let settings = self
            .trigger_timing
            .normalized_with_lookahead(self.lookahead);
        self.trigger != TriggerSource::DawPosition
            && (settings.blend_ms > 0.0
                || ((settings.audio_delay_ms != 0.0 || self.lookahead)
                    && matches!(self.trigger, TriggerSource::TransientSample { .. })))
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Blend {
    from: f32,
    total: u32,
    remaining: u32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TimingRuntime {
    settings: TriggerTiming,
    sample_rate: f64,
    audio: bool,
    input: DetectorInput,
    delay: usize,
    pending: [u64; PENDING_SIZE / 64],
    cursor: usize,
    count: usize,
    blends: [Blend; 4],
    pub(super) sample_clocked: bool,
    pub(super) first_sample: bool,
    pub(super) phases: [f64; 4],
    pub(super) requested_path: Option<u16>,
    path: u16,
    pub(super) capture_epoch: u64,
}
impl Default for TimingRuntime {
    fn default() -> Self {
        Self {
            settings: TriggerTiming::default(),
            sample_rate: 0.0,
            audio: false,
            input: DetectorInput::Main,
            delay: 0,
            pending: [0; PENDING_SIZE / 64],
            cursor: 0,
            count: 0,
            blends: [Blend::default(); 4],
            sample_clocked: false,
            first_sample: false,
            phases: [0.0; 4],
            requested_path: None,
            path: 0,
            capture_epoch: 0,
        }
    }
}
impl TimingRuntime {
    pub(super) fn clear_pending(&mut self) {
        self.pending.fill(0);
        self.cursor = 0;
        self.count = 0;
    }
    pub(super) fn cancel_blends(&mut self) {
        self.blends.fill(Blend::default());
    }
    pub(super) fn reset(&mut self) {
        self.clear_pending();
        self.cancel_blends();
        // Legacy clocks clear this during preference preparation.
        self.first_sample = true;
        self.phases = [0.0; 4];
    }
    pub(super) fn set_preferences(&mut self, config: &TrackModulation) {
        let settings = config
            .trigger_timing
            .normalized_with_lookahead(config.lookahead);
        let windows = self.requested_path.unwrap_or(u16::from(
            config.lookahead && matches!(config.trigger, TriggerSource::TransientSample { .. }),
        ));
        if settings.audio_delay_ms == 0.0 && windows == 0 && self.count != 0 {
            self.clear_pending();
        }
        if settings.blend_ms == 0.0 || config.trigger == TriggerSource::DawPosition {
            self.cancel_blends();
        }
        self.settings = settings;
        self.sample_clocked = config.timing_active()
            || (windows > 0 && matches!(config.trigger, TriggerSource::TransientSample { .. }));
        if !self.sample_clocked {
            self.first_sample = false;
        }
    }
    pub(super) fn prepare(
        &mut self,
        config: &TrackModulation,
        audio: bool,
        sr: f64,
        outputs: [f32; 4],
        phases: [f64; 4],
    ) {
        let was_clocked = self.sample_clocked;
        let path = self.requested_path.unwrap_or(u16::from(
            config.lookahead && matches!(config.trigger, TriggerSource::TransientSample { .. }),
        ));
        if path != self.path {
            self.clear_pending();
            self.path = path;
        }
        self.set_preferences(config);
        if !was_clocked && self.sample_clocked {
            self.phases = phases;
        }
        if !sr.is_finite() || sr <= 0.0 || !(sr as f32).is_finite() || sr as f32 <= 0.0 {
            self.reset();
            self.audio = false;
            return;
        }
        let input = config.audio_trigger.input;
        if !audio || !self.audio || self.input != input {
            if self.count != 0 {
                self.clear_pending();
            }
        } else if self.sample_rate > 0.0 && self.sample_rate != sr && self.count > 0 {
            let pending = self.pending;
            let cursor = self.cursor;
            self.clear_pending();
            for (i, mut word) in pending.into_iter().enumerate() {
                while word != 0 {
                    let bit = word.trailing_zeros() as usize;
                    word &= word - 1;
                    let distance = (i * 64 + bit + PENDING_SIZE - cursor) % PENDING_SIZE;
                    let next = (distance as f64 * sr / self.sample_rate)
                        .ceil()
                        .clamp(0.0, (PENDING_SIZE - 1) as f64)
                        as usize;
                    self.enqueue(next);
                }
            }
        }
        if self.sample_rate > 0.0 && self.sample_rate != sr {
            for (i, blend) in self.blends.iter_mut().enumerate() {
                if blend.remaining > 0 {
                    let remaining = (blend.remaining as f64 * sr / self.sample_rate)
                        .ceil()
                        .clamp(1.0, u32::MAX as f64) as u32;
                    *blend = Blend {
                        from: outputs[i],
                        total: remaining,
                        remaining,
                    };
                }
            }
        }
        self.sample_rate = sr;
        self.audio = audio;
        self.input = input;
        self.delay = (self.path.count_ones() as f64
            * crate::lookahead::lookahead_samples(sr) as f64
            + (self.settings.audio_delay_ms as f64 * sr * 0.001).round())
        .round()
        .clamp(0.0, (PENDING_SIZE - 1) as f64) as usize;
    }
    fn enqueue(&mut self, distance: usize) {
        let index = (self.cursor + distance) % PENDING_SIZE;
        let bit = 1_u64 << (index % 64);
        if self.pending[index / 64] & bit == 0 {
            self.pending[index / 64] |= bit;
            self.count += 1;
        }
    }
    pub(super) fn dispatch(&mut self, detected: bool) -> bool {
        if !self.audio || (self.delay == 0 && self.count == 0) {
            return detected;
        }
        let bit = 1_u64 << (self.cursor % 64);
        let mut fired = self.pending[self.cursor / 64] & bit != 0;
        if fired {
            self.pending[self.cursor / 64] &= !bit;
            self.count -= 1;
        }
        if detected {
            if self.delay == 0 {
                fired = true;
            } else {
                self.enqueue(self.delay);
            }
        }
        self.cursor = (self.cursor + 1) % PENDING_SIZE;
        fired
    }
    pub(super) fn arm(&mut self, outputs: [f32; 4], valid: [bool; 4]) {
        self.first_sample = self.sample_clocked;
        if self.sample_clocked {
            self.phases = [0.0; 4];
        }
        if self.settings.blend_ms <= 0.0 || self.sample_rate <= 0.0 {
            return;
        }
        let frames = (self.settings.blend_ms as f64 * self.sample_rate * 0.001)
            .ceil()
            .clamp(1.0, u32::MAX as f64) as u32;
        for (i, blend) in self.blends.iter_mut().enumerate() {
            *blend = if valid[i] {
                Blend {
                    from: outputs[i],
                    total: frames,
                    remaining: frames,
                }
            } else {
                Blend::default()
            };
        }
    }
    pub(super) fn cancel_slot(&mut self, slot: usize) {
        self.blends[slot] = Blend::default();
    }
    pub(super) fn source(&mut self, slot: usize, value: f32) -> f32 {
        let blend = &mut self.blends[slot];
        if blend.remaining <= 1 {
            blend.remaining = 0;
            return value;
        }
        blend.remaining -= 1;
        let progress = 1.0 - blend.remaining as f64 / blend.total as f64;
        (blend.from as f64 + (value as f64 - blend.from as f64) * progress) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_shift_compensates_logical_lookahead_paths_and_old_positive_states_stay_compatible() {
        for shift in [-4.0, 0.0, 4.0] {
            let cfg = TrackModulation {
                trigger: TriggerSource::TransientSample {
                    threshold: 1.5,
                    hold_ms: 5.0,
                },
                lookahead: true,
                trigger_timing: TriggerTiming {
                    audio_delay_ms: shift,
                    blend_ms: 0.0,
                },
                ..TrackModulation::default()
            };
            let mut timing = TimingRuntime::default();
            timing.requested_path = Some(0b101);
            timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
            let due = (8.0 + shift) as usize;
            for i in 0..14 {
                assert_eq!(timing.dispatch(i == 0), i == due);
            }
            assert!(timing.sample_clocked);
            let json = serde_json::to_string(&cfg).unwrap();
            let restored: TrackModulation = serde_json::from_str(&json).unwrap();
            assert_eq!(restored.lookahead, true);
            assert_eq!(restored.trigger_timing, cfg.trigger_timing);
        }
        let old: TriggerTiming =
            serde_json::from_str(r#"{"audio_delay_ms":2.0,"blend_ms":3.0}"#).unwrap();
        assert_eq!(old.normalized(), old);
        assert!(
            !serde_json::to_string(&TrackModulation::default())
                .unwrap()
                .contains("lookahead")
        );
    }
    #[test]
    fn compensated_rows_with_local_timing_off_keep_sample_clocking_and_new_topology_clears_pending()
    {
        let cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            },
            ..TrackModulation::default()
        };
        let mut timing = TimingRuntime::default();
        timing.requested_path = Some(1);
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        assert!(timing.sample_clocked);
        assert!(!timing.dispatch(true));
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        assert!(!timing.dispatch(false));
        assert!(!timing.dispatch(false));
        assert!(!timing.dispatch(false));
        assert!(timing.dispatch(false));
        assert!(!timing.dispatch(true));
        timing.requested_path = Some(2);
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        for _ in 0..8 {
            assert!(!timing.dispatch(false));
        }
    }
    #[test]
    fn queued_audio_resets_follow_each_accepted_sample_and_merge_only_coincident_deadlines() {
        let mut timing = TimingRuntime::default();
        let mut cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 0.0,
            },
            trigger_timing: TriggerTiming {
                audio_delay_ms: 4.0,
                blend_ms: 0.0,
            },
            ..TrackModulation::default()
        };
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        let events = [true, false, true, false, false, false, false, false];
        let actual: Vec<_> = events.into_iter().map(|hit| timing.dispatch(hit)).collect();
        assert_eq!(
            actual,
            [false, false, false, false, true, false, true, false]
        );
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        assert!(!timing.dispatch(true));
        cfg.trigger_timing.audio_delay_ms = 2.0;
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        assert!(!timing.dispatch(false));
        assert!(!timing.dispatch(true));
        assert!(!timing.dispatch(false));
        assert!(timing.dispatch(false));
        assert!(!timing.dispatch(false));
    }
    #[test]
    fn retrigger_blends_start_at_delivered_values_and_reverse_without_jumps() {
        let mut timing = TimingRuntime::default();
        let cfg = TrackModulation {
            trigger_timing: TriggerTiming {
                audio_delay_ms: 0.0,
                blend_ms: 4.0,
            },
            ..TrackModulation::default()
        };
        timing.prepare(&cfg, false, 1000.0, [0.0; 4], [0.0; 4]);
        timing.arm([1.0; 4], [true; 4]);
        assert_eq!(timing.source(0, 0.0), 0.75);
        assert_eq!(timing.source(0, 0.0), 0.5);
        timing.arm([0.5; 4], [true; 4]);
        assert_eq!(timing.source(0, 1.0), 0.625);
        assert_eq!(timing.source(0, 1.0), 0.75);
        assert_eq!(timing.source(0, 1.0), 0.875);
        assert_eq!(timing.source(0, 1.0), 1.0);
    }
    #[test]
    fn pending_delays_and_blends_rescale_and_input_or_off_changes_drop_pending_resets() {
        let mut timing = TimingRuntime::default();
        let mut cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            },
            trigger_timing: TriggerTiming {
                audio_delay_ms: 4.0,
                blend_ms: 4.0,
            },
            ..TrackModulation::default()
        };
        timing.prepare(&cfg, true, 1000.0, [1.0; 4], [0.25; 4]);
        assert!(!timing.dispatch(true));
        assert!(!timing.dispatch(false));
        timing.arm([1.0; 4], [true; 4]);
        assert_eq!(timing.source(0, 0.0), 0.75);
        timing.prepare(&cfg, true, 2000.0, [0.75; 4], [0.0; 4]);
        for _ in 0..4 {
            assert!(!timing.dispatch(false));
        }
        assert!(timing.dispatch(false));
        let mut value = 0.0;
        for i in 0..6 {
            value = timing.source(0, 0.0);
            assert!((value - 0.75 * (5 - i) as f32 / 6.0).abs() < 1e-7);
        }
        assert_eq!(value, 0.0);
        assert!(!timing.dispatch(true));
        cfg.audio_trigger.input = DetectorInput::Sidechain;
        timing.prepare(&cfg, true, 2000.0, [0.0; 4], [0.0; 4]);
        for _ in 0..10 {
            assert!(!timing.dispatch(false));
        }
        assert!(!timing.dispatch(true));
        cfg.trigger_timing.audio_delay_ms = 0.0;
        timing.prepare(&cfg, true, 2000.0, [0.0; 4], [0.0; 4]);
        for _ in 0..10 {
            assert!(!timing.dispatch(false));
        }
        let wire = serde_json::to_string(&TrackModulation::default()).unwrap();
        assert!(!wire.contains("trigger_timing"));
        cfg.trigger_timing.audio_delay_ms = 2.0;
        let wire = serde_json::to_string(&cfg).unwrap();
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&wire)
                .unwrap()
                .trigger_timing,
            cfg.trigger_timing
        );
    }
    #[test]
    fn delivered_blend_is_after_source_lag_and_mix_and_reset_sample_use_actual_values() {
        for smooth in [0.0, 10.0] {
            let mut doc = MsegData {
                play_mode: PlayMode::OneShot,
                sync_mode: SyncMode::Time,
                time_seconds: 1.0,
                ..MsegData::default()
            };
            let sustain = doc.insert_node(0.01, 1.0).unwrap();
            doc.hold = tiny_skia_widgets::HoldMode::Sustain(sustain);
            let cfg = TrackModulation {
                msegs: [doc; 4],
                targets: [Some(ModulationTarget::Mix), None, None],
                depths: [1.0; 3],
                smooth_ms: [smooth; 4],
                trigger_timing: TriggerTiming {
                    blend_ms: 4.0,
                    audio_delay_ms: 0.0,
                },
                ..TrackModulation::default()
            };
            let track = TrackEffect {
                mix: 0.0,
                ..TrackEffect::default()
            };
            let mut effect = crate::effects::EffectInstance::new(track.kind);
            let mut state = ModulationState::default();
            state.begin_block(&cfg, &[0.0; 400], &[0.0; 400], 1000.0);
            state.advance_segment(&cfg, &track, &mut effect, 400, 120.0, 1000.0);
            for _ in 0..400 {
                state.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
                state.next_mix(0.0);
            }
            assert_eq!(state.output(0), 1.0);
            state.trigger();
            state.advance_segment(&cfg, &track, &mut effect, 4, 120.0, 1000.0);
            let mut base = 1.0_f64;
            let gain = if smooth == 0.0 {
                1.0
            } else {
                1.0 - (-0.1_f64).exp()
            };
            for i in 0..4 {
                state.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
                base += gain * (i as f64 * 0.1 - base);
                let expected = 1.0 + (base - 1.0) * (i + 1) as f64 / 4.0;
                assert!((state.output(0) as f64 - expected).abs() < 1e-6);
                assert!((state.next_mix(0.0) as f64 - expected).abs() < 1e-6);
                assert!((state.precise_phase(0) - i as f64 * 0.001).abs() < 1e-12);
            }
        }
    }
    #[test]
    fn follow_and_enum_sources_skip_retrigger_blending_and_clock_survives_flush() {
        let mut doc = MsegData {
            play_mode: PlayMode::OneShot,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        };
        let point = doc.insert_node(0.01, 1.0).unwrap();
        doc.hold = tiny_skia_widgets::HoldMode::Sustain(point);
        let mut cfg = TrackModulation {
            msegs: [doc; 4],
            targets: [Some(ModulationTarget::EffectParameter(2)), None, None],
            trigger_timing: TriggerTiming {
                blend_ms: 4.0,
                audio_delay_ms: 0.0,
            },
            ..TrackModulation::default()
        };
        cfg.follower.routes[0].enabled = true;
        cfg.follower.attack_ms = 0.0;
        cfg.follower.release_ms = 0.0;
        let track = TrackEffect {
            kind: EffectKind::Svf,
            params: default_params_for_kind(EffectKind::Svf),
            ..TrackEffect::default()
        };
        let mut effect = crate::effects::EffectInstance::new(track.kind);
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.8; 32], &[0.8; 32], 1000.0);
        state.advance_segment(&cfg, &track, &mut effect, 32, 120.0, 1000.0);
        for _ in 0..32 {
            state.process_modulation_sample(&cfg, &mut effect, [0.8; 2], [0.0; 2]);
        }
        state.trigger();
        state.advance_segment(&cfg, &track, &mut effect, 1, 120.0, 1000.0);
        state.process_modulation_sample(&cfg, &mut effect, [0.2; 2], [0.0; 2]);
        assert_eq!(state.output(0), 0.2);
        assert_eq!(state.output(1), 0.0);
        let phase = state.precise_phase(0);
        state.reset_smoothing();
        assert_eq!(state.precise_phase(0), phase);
        state.advance_segment(&cfg, &track, &mut effect, 1, 120.0, 1000.0);
        state.process_modulation_sample(&cfg, &mut effect, [0.2; 2], [0.0; 2]);
        assert_eq!(state.precise_phase(0), phase + 0.001);
    }
    #[test]
    fn queued_deadlines_wrap_and_round_zero_edits_merge_with_an_existing_due_reset() {
        let mut timing = TimingRuntime::default();
        let mut cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            },
            trigger_timing: TriggerTiming {
                audio_delay_ms: 4.0,
                blend_ms: 0.0,
            },
            ..TrackModulation::default()
        };
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        for i in 0..(PENDING_SIZE * 2 + 10) {
            assert_eq!(timing.dispatch(i % 97 == 0), i >= 4 && (i - 4) % 97 == 0);
        }
        timing.reset();
        assert!(!timing.dispatch(true));
        cfg.trigger_timing.audio_delay_ms = 0.1;
        timing.prepare(&cfg, true, 1000.0, [0.0; 4], [0.0; 4]);
        for _ in 0..3 {
            assert!(!timing.dispatch(false));
        }
        assert!(timing.dispatch(true));
        assert_eq!(timing.count, 0);
        assert!(!timing.dispatch(false));
    }
    #[test]
    fn explicit_transport_restart_on_empty_audio_clears_cues_but_keeps_the_pending_phase_zero_sample()
     {
        let mut cfg = TrackModulation {
            trigger: TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            },
            trigger_timing: TriggerTiming {
                audio_delay_ms: 4.0,
                blend_ms: 4.0,
            },
            ..TrackModulation::default()
        };
        cfg.detector_response.algorithm = TransientAlgorithm::Peak;
        cfg.msegs = std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        });
        let track = TrackEffect::default();
        let mut effect = crate::effects::EffectInstance::new(track.kind);
        let mut state = ModulationState::default();
        state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
        state.advance_segment(&cfg, &track, &mut effect, 1, 120.0, 1000.0);
        assert!(!state.next_precise_trigger([1.0; 2]));
        state.process_modulation_sample(&cfg, &mut effect, [1.0; 2], [0.0; 2]);
        state.restart_clock(&cfg);
        let epoch = state.trigger_epoch();
        state.begin_block(&cfg, &[], &[], 2000.0);
        state.begin_block(&cfg, &[0.0; 8], &[0.0; 8], 1000.0);
        state.advance_segment(&cfg, &track, &mut effect, 8, 120.0, 1000.0);
        for i in 0..8 {
            assert!(!state.next_precise_trigger([0.0; 2]));
            state.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
            assert!((state.precise_phase(0) - i as f64 * 0.001).abs() < 1e-12);
        }
        assert_eq!(state.trigger_epoch(), epoch);
    }
    #[test]
    fn explicit_reset_keeps_phase_zero_with_timing_enabled_and_preserves_legacy_off_sampling() {
        for enabled in [false, true] {
            let cfg = TrackModulation {
                msegs: std::array::from_fn(|_| MsegData {
                    play_mode: PlayMode::Cyclic,
                    sync_mode: SyncMode::Time,
                    time_seconds: 1.0,
                    ..MsegData::default()
                }),
                smooth_ms: [1.0; 4],
                targets: [None; 3],
                trigger_timing: TriggerTiming {
                    blend_ms: if enabled { 4.0 } else { 0.0 },
                    audio_delay_ms: 0.0,
                },
                ..TrackModulation::default()
            };
            let track = TrackEffect::default();
            let mut effect = crate::effects::EffectInstance::new(track.kind);
            let mut state = ModulationState::default();
            state.begin_block(&cfg, &[0.0; 4], &[0.0; 4], 1000.0);
            state.advance_segment(&cfg, &track, &mut effect, 4, 120.0, 1000.0);
            for _ in 0..4 {
                state.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
            }
            state.reset();
            state.begin_block(&cfg, &[], &[], 2000.0);
            state.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
            state.advance_segment(&cfg, &track, &mut effect, 1, 120.0, 1000.0);
            state.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
            assert_eq!(state.precise_phase(0), if enabled { 0.0 } else { 0.001 });
            let mut fresh = ModulationState::default();
            fresh.reset();
            fresh.begin_block(&cfg, &[0.0], &[0.0], 1000.0);
            fresh.advance_segment(&cfg, &track, &mut effect, 1, 120.0, 1000.0);
            fresh.process_modulation_sample(&cfg, &mut effect, [0.0; 2], [0.0; 2]);
            assert_eq!(fresh.precise_phase(0), if enabled { 0.0 } else { 0.001 });
        }
    }
}
