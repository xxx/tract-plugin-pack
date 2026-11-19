use super::{MidiNoteOn, TrackModulation};
use tiny_skia_widgets::mseg::{HoldMode, MsegData, WavePaletteSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct MidiWaveMapping {
    pub envelope: u8,
    pub first_note: u8,
    pub channel: Option<u8>,
}
impl MidiWaveMapping {
    pub fn is_valid(self) -> bool {
        self.envelope < 4
            && self.first_note <= 120
            && self.channel.is_none_or(|channel| channel < 16)
    }
    pub fn slot(self, event: MidiNoteOn) -> Option<usize> {
        if !self.is_valid()
            || event.note >= 128
            || event.channel >= 16
            || self.channel.is_some_and(|channel| event.channel != channel)
        {
            return None;
        }
        event
            .note
            .checked_sub(self.first_note)
            .filter(|slot| *slot < 8)
            .map(usize::from)
    }
}
impl<'de> serde::Deserialize<'de> for MidiWaveMapping {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            envelope: u8,
            first_note: u8,
            channel: Option<u8>,
        }
        let saved = Saved::deserialize(deserializer)?;
        let mapping = Self {
            envelope: saved.envelope,
            first_note: saved.first_note,
            channel: saved.channel,
        };
        if !mapping.is_valid() {
            return Err(serde::de::Error::custom("Invalid MIDI wave mapping"));
        }
        Ok(mapping)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct MidiWaveRuntime {
    binding: Option<MidiWaveMapping>,
    active: Option<u8>,
    curve: Box<MsegData>,
    before_curve: Box<MsegData>,
    revision: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PendingWave {
    slot: usize,
    previous_midi_wave: Option<u8>,
}

impl MidiWaveRuntime {
    pub(super) fn curve<'a>(&'a self, config: &'a TrackModulation, slot: usize) -> &'a MsegData {
        if self
            .binding
            .is_some_and(|mapping| mapping.envelope as usize == slot)
        {
            &self.curve
        } else {
            &config.msegs[slot]
        }
    }
    pub(super) fn geometry_unchanged(&self, curve: &MsegData) -> bool {
        self.before_curve.active() == curve.active() && self.before_curve.hold == curve.hold
    }
    pub(super) fn reading(&self) -> Option<(MidiWaveMapping, Option<u8>)> {
        self.binding.map(|mapping| (mapping, self.active))
    }
    pub(super) fn inherit(&mut self, previous: &Self) {
        self.binding = previous.binding;
        self.active = previous.active;
        self.revision = previous.revision;
        *self.curve = *previous.curve;
    }
    pub(super) fn reset(&mut self) {
        self.active = None;
        self.binding = None;
        self.revision = None;
    }
    pub(super) fn prepare(
        &mut self,
        config: &TrackModulation,
        bank: &WavePaletteSnapshot,
    ) -> Option<usize> {
        let mapping = config.wave_switch.filter(|mapping| mapping.is_valid());
        if mapping != self.binding {
            let previous = self
                .active
                .and(self.binding)
                .map(|mapping| mapping.envelope as usize);
            self.binding = mapping;
            self.active = None;
            self.revision = bank.revision();
            if let Some(mapping) = mapping {
                *self.curve = config.msegs[mapping.envelope as usize];
                self.curve.hold = HoldMode::None;
            }
            return previous;
        }
        let mapping = mapping?;
        let envelope = mapping.envelope as usize;
        let Some(slot) = self.active else {
            *self.curve = config.msegs[envelope];
            self.curve.hold = HoldMode::None;
            return None;
        };
        let mut changed = false;
        if self.revision != bank.revision() {
            if let Some(shape) = bank.get(slot as usize) {
                changed = self.curve.active() != shape.nodes();
                shape.apply_to(&mut self.curve);
            } else {
                self.active = None;
                *self.curve = config.msegs[envelope];
                self.curve.hold = HoldMode::None;
                changed = true;
            }
            self.revision = bank.revision();
        }
        let author = &config.msegs[envelope];
        self.curve.play_mode = author.play_mode;
        self.curve.sync_mode = author.sync_mode;
        self.curve.time_seconds = author.time_seconds;
        self.curve.beats = author.beats;
        self.curve.polarity = author.polarity;
        changed.then_some(envelope)
    }
    pub(super) fn select(
        &mut self,
        config: &TrackModulation,
        bank: &WavePaletteSnapshot,
        event: MidiNoteOn,
        capture_before: bool,
    ) -> Option<usize> {
        let mapping = config.wave_switch?;
        let slot = mapping.slot(event)?;
        let shape = bank.get(slot)?;
        if self.binding == Some(mapping)
            && self.active == Some(slot as u8)
            && self.revision == bank.revision()
        {
            return None;
        }
        if capture_before {
            *self.before_curve = *self.curve(config, mapping.envelope as usize);
        }
        self.binding = Some(mapping);
        self.active = Some(slot as u8);
        self.revision = bank.revision();
        *self.curve = config.msegs[mapping.envelope as usize];
        shape.apply_to(&mut self.curve);
        Some(mapping.envelope as usize)
    }
}

impl super::ModulationState {
    pub fn effective_mseg<'a>(&'a self, config: &'a TrackModulation, slot: usize) -> &'a MsegData {
        self.automated.curve(self.wave.curve(config, slot), slot)
    }
    pub fn midi_wave(&self) -> Option<(MidiWaveMapping, Option<u8>)> {
        self.wave.reading()
    }
    pub(crate) fn wave_generations(&self) -> [u32; 4] {
        self.wave_generations
    }
    pub fn inherit_midi_wave(&mut self, previous: &Self) {
        self.wave.inherit(&previous.wave);
        self.wave_generations = previous.wave_generations;
        self.automated.inherit(&previous.automated);
    }
    /// Prepare overrides, returning whether a first-sample geometry refresh is pending.
    pub fn prepare_wave_bank(
        &mut self,
        config: &TrackModulation,
        bank: &WavePaletteSnapshot,
    ) -> bool {
        let transition = config
            .wave_switch
            .filter(|mapping| mapping.is_valid())
            .map(|mapping| mapping.envelope as usize)
            .filter(|&slot| self.automated.active(slot) != (self.automated.requested[slot] != 0));
        if let Some(slot) = transition
            && self.pending_wave.is_none()
        {
            // Resolve bank edits and same-offset MIDI before refreshing the audible geometry.
            let before = *self.effective_mseg(config, slot);
            *self.wave.before_curve = before;
            self.pending_wave = Some(PendingWave {
                slot,
                previous_midi_wave: self.wave.active,
            });
        }
        if let Some(slot) = self.wave.prepare(config, bank)
            && !self.automated.active(slot)
            && transition != Some(slot)
        {
            self.wave_generations[slot] = self.wave_generations[slot].wrapping_add(1);
            self.smooth.wave_changed(slot);
        }
        let changed = self.automated.prepare(config, &self.wave, bank);
        for slot in 0..4 {
            if changed & (1 << slot) != 0 && transition != Some(slot) {
                self.smooth.wave_changed(slot);
            }
        }
        self.pending_wave.is_some()
    }
    pub fn select_midi_wave(
        &mut self,
        config: &TrackModulation,
        bank: &WavePaletteSnapshot,
        event: MidiNoteOn,
    ) -> bool {
        let previous_midi_wave = self.wave.active;
        if let Some(slot) = self
            .wave
            .select(config, bank, event, self.pending_wave.is_none())
        {
            if self.automated.active(slot) {
                return false;
            }
            if self.pending_wave.is_none() {
                self.pending_wave = Some(PendingWave {
                    slot,
                    previous_midi_wave,
                });
            }
            true
        } else {
            false
        }
    }
    /// Apply after precise-trigger handling; all same-offset selections are now final.
    pub fn refresh_pending_wave(
        &mut self,
        config: &TrackModulation,
        track: &super::TrackEffect,
        effect: &mut impl super::Effect,
        remaining: usize,
    ) -> Option<usize> {
        let PendingWave {
            slot,
            previous_midi_wave,
        } = self.pending_wave.take()?;
        if remaining == 0 {
            return None;
        }
        let geometry_changed = !self
            .wave
            .geometry_unchanged(self.effective_mseg(config, slot));
        if geometry_changed
            || (!self.automated.active(slot) && previous_midi_wave != self.wave.active)
        {
            self.wave_generations[slot] = self.wave_generations[slot].wrapping_add(1);
        }
        if !geometry_changed {
            return None;
        }
        if self.smooth.wave_changed(slot) {
            return Some(slot);
        }
        let doc = self.automated.curve(self.wave.curve(config, slot), slot);
        let value = tiny_skia_widgets::mseg::value_at_phase(doc, self.phases[slot]);
        let previous = self.outputs[slot];
        self.outputs[slot] = value;
        self.output_valid[slot] = true;
        if slot == 0 {
            self.amplitude = value;
            return Some(slot);
        }
        let Some(target) = config.targets[slot - 1] else {
            return Some(slot);
        };
        if config.winning_slot(target) != Some(slot) || value == previous {
            return Some(slot);
        }
        let spec = match target {
            super::ModulationTarget::EffectParameter(index) => {
                effect.parameters().get(index).copied()
            }
            super::ModulationTarget::Mix => target.spec(track.kind),
        }?;
        let effective = super::assignable_value(
            value,
            target.base_value(track),
            config.depths[slot - 1],
            spec,
            doc.polarity,
        );
        match target {
            super::ModulationTarget::EffectParameter(index) => {
                effect.set_param(index, effective);
                if self.telemetry_enabled
                    && let Some(norm) = self.parameter_norms.get_mut(index)
                {
                    *norm = Some((
                        crate::effects::value_to_norm(effective, spec.min, spec.max, spec.scaling),
                        slot as u8,
                    ));
                }
            }
            super::ModulationTarget::Mix => {
                let target = (effective * 0.01).clamp(0.0, 1.0);
                let current = self.mix.as_ref().map_or(track.mix, |ramp| ramp.current);
                self.mix = Some(super::MixRamp {
                    current,
                    target,
                    step: (target - current) / remaining as f32,
                    remaining,
                    release: false,
                });
                self.mix_slot = Some(slot as u8);
            }
        }
        Some(slot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{Effect, EffectKind, ParamFormat, ParamScaling, ParamSpec, TrackEffect};
    use crate::modulation::{MidiNoteOn, ModulationState, ModulationTarget, TriggerSource};
    use tiny_skia_widgets::mseg::{
        CurveShape, FactoryShape, PlayMode, SyncMode, WavePalette, factory_shape,
    };

    #[derive(Default)]
    struct Probe {
        value: f32,
        sets: usize,
    }
    impl Effect for Probe {
        fn process_sample(&mut self, left: f32, right: f32) -> (f32, f32) {
            (left, right)
        }
        fn parameters(&self) -> &'static [ParamSpec] {
            &[ParamSpec {
                name: "Value",
                min: 0.0,
                max: 1.0,
                default: 0.0,
                scaling: ParamScaling::Linear,
                format: ParamFormat::Number {
                    decimals: 2,
                    unit: "",
                },
            }]
        }
        fn set_param(&mut self, _: usize, value: f32) {
            self.value = value;
            self.sets += 1;
        }
        fn reset(&mut self) {}
        fn set_sample_rate(&mut self, _: f32) {}
    }
    fn setup(
        envelope: u8,
    ) -> (
        TrackModulation,
        WavePalette,
        WavePaletteSnapshot,
        TrackEffect,
    ) {
        let mut config = TrackModulation {
            wave_switch: Some(MidiWaveMapping {
                envelope,
                first_note: 36,
                channel: None,
            }),
            ..TrackModulation::default()
        };
        for curve in &mut config.msegs {
            curve.sync_mode = SyncMode::Time;
            curve.time_seconds = 1.0;
            curve.play_mode = PlayMode::Cyclic;
            for node in &mut curve.nodes {
                node.value = 0.0;
            }
        }
        config.targets = [Some(ModulationTarget::EffectParameter(0)), None, None];
        config.depths = [1.0, 0.0, 0.0];
        let mut palette = WavePalette::default();
        palette
            .store(0, &CurveShape::capture(&config.msegs[envelope as usize]))
            .unwrap();
        palette
            .store(1, &factory_shape(FactoryShape::FlatHigh))
            .unwrap();
        let mut snapshot = WavePaletteSnapshot::default();
        snapshot.refresh(&palette);
        let track = TrackEffect {
            kind: EffectKind::Svf,
            mix: 0.0,
            ..TrackEffect::default()
        };
        (config, palette, snapshot, track)
    }
    fn note(slot: u8) -> MidiNoteOn {
        MidiNoteOn {
            timing: 0,
            channel: 0,
            note: 36 + slot,
        }
    }

    #[test]
    fn playback_notes_distinguish_staged_waves_from_retriggers() {
        let (mut config, _, snapshot, track) = setup(0);
        config.trigger = TriggerSource::MidiNote {
            note: Some(42),
            channel: None,
        };
        assert!(config.accepts_playback_note(&snapshot, note(1)));
        assert!(config.accepts_playback_note(&snapshot, note(6)));
        assert!(!config.accepts_playback_note(&snapshot, note(7)));

        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        let wave = state.dispatch_playback_note(&config, &snapshot, note(1), true);
        assert!(wave.wave_staged);
        assert!(!wave.retriggered);
        assert_eq!(state.trigger_epoch(), 0);
        let trigger = state.dispatch_playback_note(&config, &snapshot, note(6), true);
        assert!(!trigger.wave_staged);
        assert!(trigger.retriggered);
        assert_eq!(state.trigger_epoch(), 1);
        assert_eq!(
            state.refresh_pending_wave(&config, &track, &mut effect, 8),
            Some(0)
        );
        assert_eq!(state.amplitude(), 1.0);
    }

    #[test]
    fn switches_change_only_geometry_without_advancing_phases_or_trigger_epochs() {
        let (mut config, _, snapshot, track) = setup(0);
        config.msegs[0].hold = HoldMode::Sustain(0);
        let before = config.clone();
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        assert_eq!(state.effective_mseg(&config, 0).hold, HoldMode::None);
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 1000.0);
        let phases = state.phases;
        let epoch = state.trigger_epoch();
        assert!(state.select_midi_wave(&config, &snapshot, note(1)));
        assert_eq!(state.amplitude(), 0.0);
        assert_eq!(
            state.refresh_pending_wave(&config, &track, &mut effect, 50),
            Some(0)
        );
        assert_eq!(state.amplitude(), 1.0);
        assert_eq!(state.phases, phases);
        assert_eq!(state.trigger_epoch(), epoch);
        assert_eq!(config, before);
        assert_eq!(state.midi_wave().unwrap().1, Some(1));
        assert!(!state.select_midi_wave(&config, &snapshot, note(7)));
        assert_eq!(state.midi_wave().unwrap().1, Some(1));
        config.wave_switch = None;
        state.prepare_wave_bank(&config, &snapshot);
        assert_eq!(state.effective_mseg(&config, 0).hold, HoldMode::Sustain(0));
    }

    #[test]
    fn same_offset_return_to_original_wave_performs_no_intermediate_parameter_sets() {
        let (config, _, snapshot, track) = setup(1);
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        state.select_midi_wave(&config, &snapshot, note(0));
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 1000.0);
        state.refresh_pending_wave(&config, &track, &mut effect, 100);
        let sets = effect.sets;
        let phases = state.phases;
        let generations = state.wave_generations();
        state.select_midi_wave(&config, &snapshot, note(1));
        state.select_midi_wave(&config, &snapshot, note(0));
        assert_eq!(effect.sets, sets);
        assert_eq!(
            state.refresh_pending_wave(&config, &track, &mut effect, 80),
            None
        );
        assert_eq!(effect.sets, sets);
        assert_eq!(state.phases, phases);
        assert_eq!(state.wave_generations(), generations);
    }

    #[test]
    fn smooth_switch_preserves_lag_and_samples_the_new_wave_before_the_next_control_tick() {
        let (mut config, _, snapshot, track) = setup(0);
        config.smooth_ms[0] = 10.0;
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 48000.0);
        for _ in 0..5 {
            state.process_smoothing_sample(&config, &mut effect);
        }
        assert_eq!(state.amplitude(), 0.0);
        let phase = state.precise_phase(0);
        state.select_midi_wave(&config, &snapshot, note(1));
        state.refresh_pending_wave(&config, &track, &mut effect, 95);
        assert_eq!(state.amplitude(), 0.0);
        state.process_smoothing_sample(&config, &mut effect);
        assert!((state.amplitude() - (1.0 - (-1.0_f32 / 480.0).exp())).abs() < 1e-7);
        assert!((state.precise_phase(0) - phase - 1.0 / 48000.0).abs() < 1e-12);
    }

    #[test]
    fn smoothed_wave_response_is_identical_across_segment_preparation() {
        for (envelope, target) in [
            (0, None),
            (1, Some(ModulationTarget::EffectParameter(0))),
            (1, Some(ModulationTarget::Mix)),
        ] {
            let (mut config, _, bank, track) = setup(envelope);
            config.targets = [target, None, None];
            config.smooth_ms[envelope as usize] = 10.0;
            let mut actual = ModulationState::default();
            let mut reference = ModulationState::default();
            let mut effect = Probe::default();
            let mut reference_effect = Probe::default();
            actual.prepare_wave_bank(&config, &bank);
            reference.prepare_wave_bank(&config, &bank);
            actual.advance_segment(&config, &track, &mut effect, 5, 120.0, 48000.0);
            reference.advance_segment(&config, &track, &mut reference_effect, 13, 120.0, 48000.0);
            for _ in 0..5 {
                actual.process_smoothing_sample(&config, &mut effect);
                reference.process_smoothing_sample(&config, &mut reference_effect);
            }
            actual.select_midi_wave(&config, &bank, note(1));
            reference.select_midi_wave(&config, &bank, note(1));
            actual.advance_segment(&config, &track, &mut effect, 8, 120.0, 48000.0);
            for sample in 0..8 {
                actual.refresh_pending_wave(&config, &track, &mut effect, 8 - sample);
                reference.refresh_pending_wave(&config, &track, &mut reference_effect, 8 - sample);
                actual.process_smoothing_sample(&config, &mut effect);
                reference.process_smoothing_sample(&config, &mut reference_effect);
                assert_eq!(
                    actual.output(envelope as usize).to_bits(),
                    reference.output(envelope as usize).to_bits(),
                    "envelope {envelope}, sample {sample}, target {target:?}"
                );
                assert_eq!(effect.value.to_bits(), reference_effect.value.to_bits());
                assert_eq!(
                    actual.next_mix(0.0).to_bits(),
                    reference.next_mix(0.0).to_bits()
                );
                assert_eq!(
                    actual.precise_phase(envelope as usize),
                    reference.precise_phase(envelope as usize)
                );
            }
            assert!(actual.output(envelope as usize) > 0.0);
        }
    }

    #[test]
    fn wave_mix_transition_continues_from_the_consumed_value_at_the_existing_endpoint() {
        for (base, depth) in [(0.0, 1.0), (0.25, 0.25)] {
            let (mut config, _, snapshot, mut track) = setup(1);
            config.targets[0] = Some(ModulationTarget::Mix);
            config.depths[0] = depth;
            track.mix = base;
            let mut state = ModulationState::default();
            let mut effect = Probe::default();
            state.prepare_wave_bank(&config, &snapshot);
            state.advance_segment(&config, &track, &mut effect, 100, 120.0, 1000.0);
            for _ in 0..40 {
                assert_eq!(state.next_mix(base), base);
            }
            let phases = state.phases;
            state.select_midi_wave(&config, &snapshot, note(1));
            state.refresh_pending_wave(&config, &track, &mut effect, 60);
            assert!((state.next_mix(base) - (base + depth / 60.0)).abs() < 1e-7);
            assert_eq!(state.phases, phases);
            for _ in 0..59 {
                state.next_mix(base);
            }
            assert_eq!(state.next_mix(base), base + depth);
        }
    }

    #[test]
    fn wave_switch_updates_a_shadowed_slot_without_delivering_its_parameter() {
        let (mut config, _, snapshot, track) = setup(1);
        config.targets[1] = config.targets[0];
        config.depths[1] = 1.0;
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        state.advance_segment(&config, &track, &mut effect, 16, 120.0, 48000.0);
        let sets = effect.sets;
        assert!(state.select_midi_wave(&config, &snapshot, note(1)));
        assert_eq!(
            state.refresh_pending_wave(&config, &track, &mut effect, 16),
            Some(1)
        );
        assert_eq!(state.output(1), 1.0);
        assert_eq!(effect.sets, sets);
    }

    #[test]
    fn effect_replacement_inherits_latched_wave_and_ignores_repeat_selection_until_bank_edits() {
        let (config, mut palette, mut snapshot, track) = setup(1);
        let mut old = ModulationState::default();
        let mut effect = Probe::default();
        old.prepare_wave_bank(&config, &snapshot);
        assert!(old.select_midi_wave(&config, &snapshot, note(1)));
        old.refresh_pending_wave(&config, &track, &mut effect, 16);
        let mut replacement = ModulationState::default();
        replacement.inherit_midi_wave(&old);
        assert_eq!(replacement.midi_wave(), old.midi_wave());
        assert_eq!(
            replacement.effective_mseg(&config, 1),
            old.effective_mseg(&config, 1)
        );
        assert!(!replacement.select_midi_wave(&config, &snapshot, note(1)));

        palette
            .store(1, &CurveShape::capture(&config.msegs[1]))
            .unwrap();
        assert!(snapshot.refresh(&palette));
        assert!(replacement.select_midi_wave(&config, &snapshot, note(1)));
        assert_eq!(replacement.effective_mseg(&config, 1).nodes[0].value, 0.0);
    }

    #[test]
    fn selected_bank_edits_and_deletion_refresh_smoothed_sources_and_keep_the_clock() {
        let (mut config, mut palette, mut snapshot, track) = setup(0);
        config.smooth_ms[0] = 10.0;
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        state.select_midi_wave(&config, &snapshot, note(0));
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 48000.0);
        for _ in 0..5 {
            state.process_smoothing_sample(&config, &mut effect);
        }
        palette
            .store(0, &factory_shape(FactoryShape::FlatHigh))
            .unwrap();
        snapshot.refresh(&palette);
        let phase = state.precise_phase(0);
        state.prepare_wave_bank(&config, &snapshot);
        state.advance_segment(&config, &track, &mut effect, 50, 120.0, 48000.0);
        state.process_smoothing_sample(&config, &mut effect);
        let current = state.amplitude();
        assert!(current > 0.0 && current < 0.01);
        assert!(state.precise_phase(0) > phase);
        palette.clear(0).unwrap();
        snapshot.refresh(&palette);
        state.prepare_wave_bank(&config, &snapshot);
        state.advance_segment(&config, &track, &mut effect, 50, 120.0, 48000.0);
        state.process_smoothing_sample(&config, &mut effect);
        assert!(state.amplitude() < current);
        assert_eq!(state.midi_wave().unwrap().1, None);
    }

    #[test]
    fn mapping_serde_defaults_off_and_reset_differs_from_transport_retrigger() {
        let (config, _, snapshot, track) = setup(0);
        let default = serde_json::to_string(&TrackModulation::default()).unwrap();
        assert!(!default.contains("wave_switch"));
        assert_eq!(
            serde_json::from_str::<TrackModulation>(&default)
                .unwrap()
                .wave_switch,
            None
        );
        let restored: TrackModulation =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.wave_switch, config.wave_switch);
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &snapshot);
        state.select_midi_wave(&config, &snapshot, note(1));
        state.advance_segment(&config, &track, &mut effect, 100, 120.0, 1000.0);
        state.restart_clock(&config);
        assert_eq!(state.midi_wave().unwrap().1, Some(1));
        state.reset();
        state.prepare_wave_bank(&config, &snapshot);
        assert_eq!(state.midi_wave().unwrap().1, None);
        assert_eq!(
            state.effective_mseg(&config, 0).active(),
            config.msegs[0].active()
        );
    }

    #[test]
    fn selecting_identical_authored_or_bank_geometry_keeps_the_lag_clock_and_parameter_setters() {
        for initial_slot in [None, Some(0)] {
            let (mut config, mut palette, mut bank, track) = setup(1);
            config.msegs[1].nodes[1].value = 1.0;
            config.smooth_ms[1] = 100.0;
            let shape = CurveShape::capture(&config.msegs[1]);
            palette.store(0, &shape).unwrap();
            palette.store(1, &shape).unwrap();
            bank.refresh(&palette);
            let mut actual = ModulationState::default();
            let mut reference = ModulationState::default();
            let mut effect = Probe::default();
            let mut reference_effect = Probe::default();
            actual.prepare_wave_bank(&config, &bank);
            reference.prepare_wave_bank(&config, &bank);
            if let Some(slot) = initial_slot {
                actual.select_midi_wave(&config, &bank, note(slot));
                reference.select_midi_wave(&config, &bank, note(slot));
            }
            actual.advance_segment(&config, &track, &mut effect, 128, 120.0, 48000.0);
            reference.advance_segment(&config, &track, &mut reference_effect, 128, 120.0, 48000.0);
            for _ in 0..23 {
                actual.process_smoothing_sample(&config, &mut effect);
                reference.process_smoothing_sample(&config, &mut reference_effect);
            }
            let sets = effect.sets;
            actual.select_midi_wave(
                &config,
                &bank,
                note(if initial_slot.is_some() { 1 } else { 0 }),
            );
            assert_eq!(
                actual.refresh_pending_wave(&config, &track, &mut effect, 105),
                None
            );
            actual.process_smoothing_sample(&config, &mut effect);
            reference.process_smoothing_sample(&config, &mut reference_effect);
            assert_eq!(actual.output(1).to_bits(), reference.output(1).to_bits());
            assert_eq!(effect.sets, sets);
            assert_eq!(effect.sets, reference_effect.sets);
            assert_eq!(actual.precise_phase(1), reference.precise_phase(1));
        }
    }

    #[test]
    fn automated_waves_override_midi_independently_and_resume_its_latest_selection() {
        let (mut config, mut palette, mut bank, track) = setup(0);
        let mut quarter = config.msegs[0];
        for node in &mut quarter.nodes {
            node.value = 0.25;
        }
        palette.store(2, &CurveShape::capture(&quarter)).unwrap();
        bank.refresh(&palette);
        config.msegs[0].hold = HoldMode::Sustain(0);
        let saved = config.clone();
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.set_wave_automation([3, 2, 8, 0]);
        state.prepare_wave_bank(&config, &bank);
        state.advance_segment(&config, &track, &mut effect, 64, 120.0, 48000.0);
        state.refresh_pending_wave(&config, &track, &mut effect, 64);
        assert_eq!(state.output(0), 0.25);
        assert_eq!(state.output(1), 1.0);
        assert_eq!(state.output(2), 0.0);
        let phases = state.phases;
        let sets = effect.sets;
        state.select_midi_wave(&config, &bank, note(1));
        assert_eq!(
            state.refresh_pending_wave(&config, &track, &mut effect, 64),
            None
        );
        assert_eq!(state.output(0), 0.25);
        assert_eq!(state.phases, phases);
        assert_eq!(effect.sets, sets);
        state.set_wave_automation([0; 4]);
        state.prepare_wave_bank(&config, &bank);
        state.advance_segment(&config, &track, &mut effect, 64, 120.0, 48000.0);
        assert_eq!(state.output(0), 1.0);
        assert_eq!(state.midi_wave().unwrap().1, Some(1));
        config.wave_switch = None;
        state.prepare_wave_bank(&config, &bank);
        assert_eq!(state.effective_mseg(&config, 0).hold, HoldMode::Sustain(0));
        config.wave_switch = saved.wave_switch;
        assert_eq!(config, saved);
    }

    #[test]
    fn automated_slot_refreshes_empty_populated_deleted_and_authored_fallbacks() {
        let (mut config, mut palette, mut bank, track) = setup(0);
        config.wave_switch = None;
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.set_wave_automation([8; 4]);
        for expected in [0.0, 1.0, 0.0] {
            state.prepare_wave_bank(&config, &bank);
            state.advance_segment(&config, &track, &mut effect, 32, 120.0, 48000.0);
            for slot in 0..4 {
                assert_eq!(state.output(slot), expected);
                assert_eq!(state.effective_mseg(&config, slot).hold, HoldMode::None);
            }
            if expected == 0.0 {
                palette
                    .store(7, &factory_shape(FactoryShape::FlatHigh))
                    .unwrap();
            } else {
                palette.clear(7).unwrap();
            }
            bank.refresh(&palette);
        }
        state.reset();
        state.prepare_wave_bank(&config, &bank);
        assert_eq!(state.wave_automation(), [8; 4]);
        state.set_wave_automation([9, 255, 0, 1]);
        state.prepare_wave_bank(&config, &bank);
        assert_eq!(state.wave_automation(), [0, 0, 0, 1]);
    }

    #[test]
    fn automation_activation_ignores_same_boundary_edits_to_the_shadowed_midi_wave() {
        for (envelope, target) in [
            (0, None),
            (1, Some(ModulationTarget::EffectParameter(0))),
            (1, Some(ModulationTarget::Mix)),
        ] {
            let (mut config, mut palette, mut bank, track) = setup(envelope);
            config.targets[0] = target;
            config.smooth_ms[envelope as usize] = 10.0;
            config.msegs[envelope as usize].time_seconds = 0.001;
            palette
                .store(0, &factory_shape(FactoryShape::RampUp))
                .unwrap();
            palette
                .store(1, &factory_shape(FactoryShape::RampUp))
                .unwrap();
            bank.refresh(&palette);
            let old_bank = bank.clone();
            let mut actual = ModulationState::default();
            let mut reference = ModulationState::default();
            let mut effect = Probe::default();
            let mut reference_effect = Probe::default();
            for (state, effect) in [
                (&mut actual, &mut effect),
                (&mut reference, &mut reference_effect),
            ] {
                state.prepare_wave_bank(&config, &bank);
                state.select_midi_wave(&config, &bank, note(0));
                state.advance_segment(&config, &track, effect, 5, 120.0, 48000.0);
                state.refresh_pending_wave(&config, &track, effect, 5);
                for _ in 0..5 {
                    state.process_smoothing_sample(&config, effect);
                    state.next_mix(track.mix);
                }
            }
            palette
                .store(0, &factory_shape(FactoryShape::FlatCenter))
                .unwrap();
            bank.refresh(&palette);
            let mut values = [0; 4];
            values[envelope as usize] = 2;
            actual.set_wave_automation(values);
            reference.set_wave_automation(values);
            actual.prepare_wave_bank(&config, &bank);
            reference.prepare_wave_bank(&config, &old_bank);
            actual.advance_segment(&config, &track, &mut effect, 8, 120.0, 48000.0);
            reference.advance_segment(&config, &track, &mut reference_effect, 8, 120.0, 48000.0);
            for sample in 0..8 {
                actual.refresh_pending_wave(&config, &track, &mut effect, 8 - sample);
                reference.refresh_pending_wave(&config, &track, &mut reference_effect, 8 - sample);
                actual.process_smoothing_sample(&config, &mut effect);
                reference.process_smoothing_sample(&config, &mut reference_effect);
                assert_eq!(
                    actual.output(envelope as usize).to_bits(),
                    reference.output(envelope as usize).to_bits(),
                    "{target:?} sample {sample}"
                );
                assert_eq!(effect.value.to_bits(), reference_effect.value.to_bits());
                assert_eq!(effect.sets, reference_effect.sets);
                assert_eq!(
                    actual.next_mix(track.mix).to_bits(),
                    reference.next_mix(track.mix).to_bits()
                );
                assert_eq!(
                    actual.precise_phase(envelope as usize),
                    reference.precise_phase(envelope as usize)
                );
            }
        }
    }

    #[test]
    fn automation_release_and_same_offset_midi_resolve_as_one_geometry_change() {
        for (envelope, target) in [
            (0, None),
            (1, Some(ModulationTarget::EffectParameter(0))),
            (1, Some(ModulationTarget::Mix)),
        ] {
            let (mut config, mut palette, mut bank, track) = setup(envelope);
            config.targets[0] = target;
            config.smooth_ms[envelope as usize] = 10.0;
            config.msegs[envelope as usize].time_seconds = 0.001;
            palette
                .store(0, &factory_shape(FactoryShape::RampUp))
                .unwrap();
            palette
                .store(1, &CurveShape::capture(&config.msegs[envelope as usize]))
                .unwrap();
            bank.refresh(&palette);
            let mut actual = ModulationState::default();
            let mut reference = ModulationState::default();
            let mut effect = Probe::default();
            let mut reference_effect = Probe::default();
            let mut values = [0; 4];
            values[envelope as usize] = 1;
            for (state, effect) in [
                (&mut actual, &mut effect),
                (&mut reference, &mut reference_effect),
            ] {
                state.set_wave_automation(values);
                state.prepare_wave_bank(&config, &bank);
                state.advance_segment(&config, &track, effect, 5, 120.0, 48000.0);
                state.select_midi_wave(&config, &bank, note(1));
                state.refresh_pending_wave(&config, &track, effect, 5);
                for _ in 0..5 {
                    state.process_smoothing_sample(&config, effect);
                    state.next_mix(track.mix);
                }
            }
            actual.set_wave_automation([0; 4]);
            for (state, effect) in [
                (&mut actual, &mut effect),
                (&mut reference, &mut reference_effect),
            ] {
                state.prepare_wave_bank(&config, &bank);
                state.select_midi_wave(&config, &bank, note(0));
                state.advance_segment(&config, &track, effect, 8, 120.0, 48000.0);
            }
            for sample in 0..8 {
                actual.refresh_pending_wave(&config, &track, &mut effect, 8 - sample);
                reference.refresh_pending_wave(&config, &track, &mut reference_effect, 8 - sample);
                actual.process_smoothing_sample(&config, &mut effect);
                reference.process_smoothing_sample(&config, &mut reference_effect);
                assert_eq!(
                    actual.output(envelope as usize).to_bits(),
                    reference.output(envelope as usize).to_bits(),
                    "{target:?} sample {sample}"
                );
                assert_eq!(effect.value.to_bits(), reference_effect.value.to_bits());
                assert_eq!(effect.sets, reference_effect.sets);
                assert_eq!(
                    actual.next_mix(track.mix).to_bits(),
                    reference.next_mix(track.mix).to_bits()
                );
                assert_eq!(
                    actual.precise_phase(envelope as usize),
                    reference.precise_phase(envelope as usize)
                );
            }
            // Releasing without another note must still refresh the newly audible source.
            actual.set_wave_automation(values);
            actual.prepare_wave_bank(&config, &bank);
            actual.select_midi_wave(&config, &bank, note(1));
            actual.set_wave_automation([0; 4]);
            actual.prepare_wave_bank(&config, &bank);
            actual.advance_segment(&config, &track, &mut effect, 8, 120.0, 48000.0);
            let previous = actual.output(envelope as usize);
            actual.refresh_pending_wave(&config, &track, &mut effect, 8);
            actual.process_smoothing_sample(&config, &mut effect);
            assert!(
                (actual.output(envelope as usize) - previous * (-1.0_f32 / 480.0).exp()).abs()
                    < 1e-7
            );
        }
    }

    #[test]
    fn automation_boundary_refreshes_smoothed_raw_without_resetting_phase_or_lag() {
        let (mut config, _, bank, track) = setup(0);
        config.wave_switch = None;
        config.smooth_ms = [10.0; 4];
        let mut state = ModulationState::default();
        let mut effect = Probe::default();
        state.prepare_wave_bank(&config, &bank);
        state.advance_segment(&config, &track, &mut effect, 5, 120.0, 48000.0);
        for _ in 0..5 {
            state.process_smoothing_sample(&config, &mut effect);
        }
        let phase = state.precise_phase(0);
        let epoch = state.trigger_epoch();
        state.set_wave_automation([2; 4]);
        state.prepare_wave_bank(&config, &bank);
        state.advance_segment(&config, &track, &mut effect, 8, 120.0, 48000.0);
        assert_eq!(state.output(0), 0.0);
        state.process_smoothing_sample(&config, &mut effect);
        for slot in 0..4 {
            assert!(
                (state.output(slot) - (1.0 - (-1.0_f32 / 480.0).exp())).abs() < 1e-7,
                "slot {slot}"
            );
        }
        assert!((state.precise_phase(0) - phase - 1.0 / 48000.0).abs() < 1e-12);
        assert_eq!(state.trigger_epoch(), epoch);
    }

    #[test]
    fn wave_mapping_bounds_channels_and_saved_state_are_validated() {
        let any = MidiWaveMapping {
            envelope: 1,
            first_note: 36,
            channel: None,
        };
        assert_eq!(any.slot(note(7)), Some(7));
        assert_eq!(any.slot(note(8)), None);
        assert_eq!(
            any.slot(MidiNoteOn {
                channel: 16,
                ..note(1)
            }),
            None
        );
        for first_note in [0, 36, 120] {
            let mapping = MidiWaveMapping {
                envelope: 3,
                first_note,
                channel: Some(15),
            };
            for slot in 0..8 {
                let event = MidiNoteOn {
                    timing: 7,
                    channel: 15,
                    note: first_note + slot,
                };
                assert_eq!(mapping.slot(event), Some(slot as usize));
                assert_eq!(
                    mapping.slot(MidiNoteOn {
                        channel: 0,
                        ..event
                    }),
                    None
                );
            }
            let text = serde_json::to_string(&mapping).unwrap();
            assert_eq!(
                serde_json::from_str::<MidiWaveMapping>(&text).unwrap(),
                mapping
            );
        }
        for mapping in [
            MidiWaveMapping {
                envelope: 4,
                first_note: 36,
                channel: None,
            },
            MidiWaveMapping {
                envelope: 0,
                first_note: 121,
                channel: None,
            },
            MidiWaveMapping {
                envelope: 0,
                first_note: 36,
                channel: Some(16),
            },
        ] {
            assert_eq!(
                mapping.slot(MidiNoteOn {
                    timing: 0,
                    channel: 0,
                    note: 36
                }),
                None
            );
            assert!(
                serde_json::from_str::<MidiWaveMapping>(&serde_json::to_string(&mapping).unwrap())
                    .is_err()
            );
        }
    }
}
