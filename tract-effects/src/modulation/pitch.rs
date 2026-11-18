use super::*;

pub const MIDI_PITCH_TRANSPOSE_MAX: i8 = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiPitchSettings {
    pub transpose: i8,
    pub channel: Option<u8>,
}
impl TriggerSource {
    pub fn pitch_settings(self) -> Option<MidiPitchSettings> {
        if let Self::MidiPitch { transpose, channel } = self {
            Some(MidiPitchSettings {
                transpose: transpose.clamp(-MIDI_PITCH_TRANSPOSE_MAX, MIDI_PITCH_TRANSPOSE_MAX),
                channel,
            })
        } else {
            None
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MidiPitchReading {
    pub settings: MidiPitchSettings,
    pub note: Option<u8>,
    pub hz: f64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MidiPitchRuntime {
    settings: Option<MidiPitchSettings>,
    note: Option<u8>,
    hz: f64,
    sample_rate: f64,
    pub(super) step: f64,
    pub(super) first_sample: bool,
    pub(super) phases: [f64; 4],
}
impl MidiPitchRuntime {
    pub(super) fn active(&self) -> bool {
        self.settings.is_some()
    }
    pub(super) fn running(&self) -> bool {
        self.active() && self.note.is_some() && self.step > 0.0
    }
    fn update_rate(&mut self) {
        self.hz = self
            .note
            .zip(self.settings)
            .map_or(0.0, |(note, settings)| {
                440.0 * 2.0_f64.powf((note as f64 - 69.0 + settings.transpose as f64) / 12.0)
            });
        self.step = if self.sample_rate.is_finite() && self.sample_rate > 0.0 {
            let step = self.hz / self.sample_rate;
            if step.is_finite() { step } else { 0.0 }
        } else {
            0.0
        };
    }
    pub(super) fn prepare(&mut self, source: TriggerSource, sr: f64) -> Option<bool> {
        let settings = source.pitch_settings();
        if settings == self.settings && sr == self.sample_rate {
            return None;
        }
        let reset_phase =
            settings.is_some() && self.settings.map(|s| s.channel) != settings.map(|s| s.channel);
        if reset_phase || settings.is_none() {
            self.note = None;
        }
        self.settings = settings;
        self.sample_rate = sr;
        self.update_rate();
        if reset_phase {
            self.first_sample = true;
            self.phases = [0.0; 4];
        }
        // Inactive legacy clocks must retain their source refresh cadence.
        (settings.is_some() || reset_phase).then_some(reset_phase)
    }
    fn select(&mut self, settings: MidiPitchSettings, note: u8) {
        self.settings = Some(settings);
        self.note = Some(note);
        self.update_rate();
    }
    pub(super) fn restart(&mut self) {
        if self.active() {
            self.first_sample = true;
            self.phases = [0.0; 4];
        }
    }
    pub(super) fn reading(&self) -> Option<MidiPitchReading> {
        self.settings.map(|settings| MidiPitchReading {
            settings,
            note: self.note,
            hz: self.hz,
        })
    }
}
impl ModulationState {
    pub fn pitch_reading(&self) -> Option<MidiPitchReading> {
        self.pitch.reading()
    }
    pub fn midi_note_on(
        &mut self,
        config: &TrackModulation,
        event: MidiNoteOn,
        within_span: bool,
    ) -> bool {
        if !config.trigger.accepts_midi(event) {
            return false;
        }
        if let Some(settings) = config.trigger.pitch_settings() {
            self.pitch.select(settings, event.note);
        }
        if within_span {
            self.trigger_midi_in_span();
        } else {
            self.trigger();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_readout_keeps_a_latched_note_while_invalid_rates_suspend_its_clock() {
        let (mut config, _, mut state, _) = setup();
        state.begin_block(&config, &[0.0], &[0.0], 48000.0);
        state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false);
        assert!(state.waveform_running(&config, 1));
        for rate in [0.0, -48000.0, f64::NAN, f64::INFINITY] {
            let epoch = state.capture_epoch(&config, 1);
            state.begin_block(&config, &[0.0], &[0.0], rate);
            assert_eq!(state.pitch.step, 0.0);
            assert!(!state.waveform_running(&config, 1));
            assert_eq!(state.pitch_reading().unwrap().note, Some(69));
            assert_eq!(state.capture_epoch(&config, 1), epoch + 1);
        }
        config.trigger = TriggerSource::MidiPitch {
            transpose: 12,
            channel: None,
        };
        state.begin_block(&config, &[0.0], &[0.0], 48000.0);
        assert!(state.waveform_running(&config, 1));
        assert_eq!(state.pitch_reading().unwrap().hz, 880.0);
    }
    fn setup() -> (
        TrackModulation,
        TrackEffect,
        ModulationState,
        crate::effects::EffectInstance,
    ) {
        let mut config = TrackModulation {
            trigger: TriggerSource::MidiPitch {
                transpose: 0,
                channel: None,
            },
            ..TrackModulation::default()
        };
        for doc in &mut config.msegs {
            *doc = MsegData {
                play_mode: PlayMode::Cyclic,
                ..MsegData::default()
            };
        }
        let track = TrackEffect::default();
        let effect = crate::effects::EffectInstance::new(track.kind);
        (config, track, ModulationState::default(), effect)
    }
    #[test]
    fn pitch_waits_then_starts_at_zero_and_ignores_saved_lengths_and_tempo() {
        let (mut config, track, mut state, mut effect) = setup();
        config.msegs[1].beats = 256.0;
        config.msegs[2].sync_mode = SyncMode::Time;
        config.msegs[2].time_seconds = 0.05;
        state.begin_block(&config, &[0.0; 16], &[0.0; 16], 1760.0);
        state.advance_segment(&config, &track, &mut effect, 16, 37.0, 1760.0);
        for _ in 0..4 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
            assert_eq!(state.phase(0), 0.0);
        }
        assert_eq!(state.pitch_reading().unwrap().note, None);
        assert!(state.midi_note_on(&config, MidiNoteOn::new(4, 0, 69, 1.0).unwrap(), true));
        state.advance_segment(&config, &track, &mut effect, 12, 211.0, 1760.0);
        for expected in [0.0, 0.25, 0.5, 0.75, 0.0, 0.25, 0.5, 0.75] {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
            for slot in 0..4 {
                assert_eq!(state.phase(slot), expected);
                assert_eq!(state.output(slot), expected);
            }
        }
        assert_eq!(state.pitch_reading().unwrap().hz, 440.0);
    }
    #[test]
    fn pitch_transpose_changes_rate_without_reset_and_channel_changes_wait_again() {
        let (mut config, track, mut state, mut effect) = setup();
        state.begin_block(&config, &[0.0; 4], &[0.0; 4], 1760.0);
        state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false);
        state.advance_segment(&config, &track, &mut effect, 4, 120.0, 1760.0);
        for _ in 0..2 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        }
        assert_eq!(state.phase(0), 0.25);
        config.trigger = TriggerSource::MidiPitch {
            transpose: 12,
            channel: None,
        };
        state.begin_block(&config, &[0.0], &[0.0], 1760.0);
        state.advance_segment(&config, &track, &mut effect, 1, 120.0, 1760.0);
        state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(state.phase(0), 0.75);
        assert_eq!(state.pitch_reading().unwrap().hz, 880.0);
        config.trigger = TriggerSource::MidiPitch {
            transpose: -12,
            channel: Some(2),
        };
        state.begin_block(&config, &[0.0], &[0.0], 1760.0);
        state.advance_segment(&config, &track, &mut effect, 1, 120.0, 1760.0);
        assert!(!state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false));
        state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(state.phase(0), 0.0);
        assert_eq!(state.pitch_reading().unwrap().note, None);
        assert!(state.midi_note_on(&config, MidiNoteOn::new(0, 2, 69, 1.0).unwrap(), false));
        assert_eq!(state.pitch_reading().unwrap().hz, 220.0);
    }
    #[test]
    fn pitch_frequency_bounds_filters_and_state_preserve_legacy_defaults() {
        for transpose in [-128, -36, 0, 36, 127] {
            let source = TriggerSource::MidiPitch {
                transpose,
                channel: Some(3),
            };
            let settings = source.pitch_settings().unwrap();
            assert_eq!(settings.transpose, transpose.clamp(-36, 36));
            for note in [0, 69, 127] {
                let event = MidiNoteOn::new(0, 3, note, 0.5).unwrap();
                assert!(source.accepts_midi(event));
                assert!(!source.accepts_midi(MidiNoteOn {
                    channel: 2,
                    ..event
                }));
                let mut pitch = MidiPitchRuntime::default();
                pitch.prepare(source, 48000.0);
                pitch.select(settings, note);
                let hz =
                    440.0 * 2.0_f64.powf((note as f64 - 69.0 + settings.transpose as f64) / 12.0);
                assert_eq!(pitch.reading().unwrap().hz, hz);
                assert_eq!(pitch.step, hz / 48000.0);
                pitch.prepare(source, 1e-308);
                assert!(pitch.step.is_finite());
            }
            let config = TrackModulation {
                trigger: source,
                ..TrackModulation::default()
            };
            let json = serde_json::to_string(&config).unwrap();
            assert_eq!(
                serde_json::from_str::<TrackModulation>(&json)
                    .unwrap()
                    .trigger,
                source
            );
        }
        let source: TriggerSource = serde_json::from_str(r#"{"MidiPitch":{}}"#).unwrap();
        assert_eq!(
            source,
            TriggerSource::MidiPitch {
                transpose: 0,
                channel: None
            }
        );
        let config = serde_json::to_string(&TrackModulation::default()).unwrap();
        assert!(!config.contains("pitch"));
    }
    #[test]
    fn waiting_hold_loops_stay_zero_and_only_actual_high_rate_traversals_change_capture_epochs() {
        let (mut config, track, mut state, mut effect) = setup();
        for doc in &mut config.msegs {
            let start = doc.insert_node(0.5, 0.5).unwrap();
            let end = doc.insert_node(0.75, 0.75).unwrap();
            doc.hold = tiny_skia_widgets::HoldMode::Loop { start, end };
        }
        state.begin_block(&config, &[0.0; 8], &[0.0; 8], 440.0);
        state.advance_segment(&config, &track, &mut effect, 8, 0.0, 440.0);
        let waiting_epoch = state.capture_epoch(&config, 0);
        for _ in 0..3 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
            assert_eq!(state.phase(0), 0.0);
            assert_eq!(state.capture_epoch(&config, 0), waiting_epoch);
            assert!(!state.waveform_running(&config, 0));
        }
        state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), true);
        state.advance_segment(&config, &track, &mut effect, 5, 0.0, 440.0);
        let note_epoch = state.capture_epoch(&config, 0);
        state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(state.phase(0), 0.0);
        assert_eq!(state.capture_epoch(&config, 0), note_epoch);
        for offset in 1..=3 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
            assert_eq!(state.phase(0), 0.5);
            assert_eq!(state.capture_epoch(&config, 0), note_epoch + offset);
        }
    }
    #[test]
    fn one_shots_retrigger_and_sample_rate_handoff_preserves_phase_lag_and_pending_starts() {
        let (mut config, track, mut state, mut effect) = setup();
        config.smooth_ms = [10.0; 4];
        config.msegs[0].play_mode = PlayMode::OneShot;
        state.begin_block(&config, &[0.0; 2], &[0.0; 2], 1760.0);
        state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false);
        state.advance_segment(&config, &track, &mut effect, 2, 120.0, 1760.0);
        for _ in 0..2 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        }
        let level = state.output(0);
        let mut next = ModulationState::default();
        next.inherit_precise_timing(&state);
        next.begin_block(&config, &[0.0], &[0.0], 3520.0);
        next.advance_segment(&config, &track, &mut effect, 1, 120.0, 3520.0);
        next.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(next.phase(0), 0.375);
        let expected = level as f64 + (1.0 - (-1.0_f64 / 35.2).exp()) * (0.375 - level as f64);
        assert!((next.output(0) as f64 - expected).abs() < 1e-7);
        next.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), true);
        let mut pending = ModulationState::default();
        pending.inherit_precise_timing(&next);
        pending.begin_block(&config, &[0.0; 10], &[0.0; 10], 1760.0);
        pending.advance_segment(&config, &track, &mut effect, 10, 120.0, 1760.0);
        pending.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(pending.phase(0), 0.0);
        for _ in 0..8 {
            pending.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        }
        assert_eq!(pending.phase(0), 1.0);
        pending.restart_clock(&config);
        pending.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(pending.phase(0), 0.0);
        assert_eq!(pending.pitch_reading().unwrap().note, Some(69));
        pending.reset();
        pending.begin_block(&config, &[0.0], &[0.0], 1760.0);
        pending.advance_segment(&config, &track, &mut effect, 1, 120.0, 1760.0);
        pending.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(pending.pitch_reading().unwrap().note, None);
        assert_eq!(pending.phase(0), 0.0);
    }
    #[test]
    fn zero_frame_source_excursions_do_not_change_pitch() {
        let (mut config, track, mut state, mut effect) = setup();
        state.begin_block(&config, &[0.0; 2], &[0.0; 2], 1760.0);
        state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false);
        state.advance_segment(&config, &track, &mut effect, 2, 120.0, 1760.0);
        for _ in 0..2 {
            state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        }
        config.trigger = TriggerSource::Free;
        state.begin_block(&config, &[], &[], 1.0);
        config.trigger = TriggerSource::MidiPitch {
            transpose: 0,
            channel: None,
        };
        state.begin_block(&config, &[0.0], &[0.0], 1760.0);
        state.advance_segment(&config, &track, &mut effect, 1, 120.0, 1760.0);
        state.process_modulation_sample(&config, &mut effect, [0.0; 2], [0.0; 2]);
        assert_eq!(state.phase(0), 0.5);
    }
    #[test]
    fn pitch_amp_and_mix_sample_every_phase_while_parameter_setters_keep_the_control_cadence() {
        struct Probe {
            inner: crate::effects::EffectInstance,
            calls: usize,
        }
        impl Effect for Probe {
            fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
                self.inner.process_sample(l, r)
            }
            fn set_sample_rate(&mut self, sr: f32) {
                self.inner.set_sample_rate(sr);
            }
            fn reset(&mut self) {
                self.inner.reset();
            }
            fn parameters(&self) -> &'static [ParamSpec] {
                self.inner.parameters()
            }
            fn set_param(&mut self, i: usize, value: f32) {
                self.calls += 1;
                self.inner.set_param(i, value);
            }
        }
        for ms in [0.0, 4.0] {
            let (mut config, _, mut state, _) = setup();
            config.targets = [
                Some(ModulationTarget::EffectParameter(0)),
                Some(ModulationTarget::Mix),
                None,
            ];
            config.depths = [1.0; 3];
            config.smooth_ms = [ms; 4];
            for doc in &mut config.msegs {
                doc.polarity = Polarity::Unipolar;
            }
            let track = TrackEffect {
                kind: EffectKind::Svf,
                params: default_params_for_kind(EffectKind::Svf),
                mix: 0.0,
                ..TrackEffect::default()
            };
            let mut probe = Probe {
                inner: crate::effects::EffectInstance::new(track.kind),
                calls: 0,
            };
            state.begin_block(&config, &[0.0; 64], &[0.0; 64], 48000.0);
            state.midi_note_on(&config, MidiNoteOn::new(0, 0, 69, 1.0).unwrap(), false);
            state.advance_segment(&config, &track, &mut probe, 64, 120.0, 48000.0);
            let mut expected = 0.0_f64;
            let gain = if ms == 0.0 {
                1.0
            } else {
                1.0 - (-1.0_f64 / 192.0).exp()
            };
            for i in 0..64 {
                if i == 13 {
                    state.midi_note_on(&config, MidiNoteOn::new(13, 0, 81, 1.0).unwrap(), true);
                    state.advance_segment(&config, &track, &mut probe, 51, 1.0, 48000.0);
                }
                state.process_modulation_sample(&config, &mut probe, [0.0; 2], [0.0; 2]);
                let phase = if i < 13 {
                    i as f64 * 440.0 / 48000.0
                } else {
                    (i - 13) as f64 * 880.0 / 48000.0
                };
                expected += gain * (phase - expected);
                assert!((state.amplitude() as f64 - expected).abs() < 1e-7);
                assert!((state.next_mix(0.0) as f64 - expected).abs() < 1e-6);
                assert_eq!(probe.calls, 1 + usize::from(i >= 13) + usize::from(i >= 48));
            }
        }
    }
}
