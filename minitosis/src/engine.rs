//! One effect, shared modulation, and latency-aligned dry audio.

use std::sync::Arc;
use tract_effects::effects::{Effect, EffectInstance, EffectKind, TrackEffect};
use tract_effects::modulation::AuditionRuntime;
use tract_effects::modulation::{ModulationState, TrackModulation};
use tract_effects::modulation_telemetry::ModulationTelemetry;
use tract_effects::sidechain::SidechainInput;
use tract_effects::waveform::{WaveformClock, WaveformTiming};

pub const MAX_LATENCY: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrepareKey {
    pub kind: EffectKind,
    pub generation: u64,
    pub sample_rate_bits: u32,
}

struct DryDelay {
    samples: Box<[[f32; 2]]>,
    position: usize,
}

impl DryDelay {
    fn new() -> Self {
        Self {
            samples: vec![[0.0; 2]; MAX_LATENCY + 1].into_boxed_slice(),
            position: 0,
        }
    }
    fn reset(&mut self) {
        self.samples.fill([0.0; 2]);
        self.position = 0;
    }
    fn next(&mut self, input: [f32; 2], latency: usize) -> [f32; 2] {
        debug_assert!(latency <= MAX_LATENCY);
        self.samples[self.position] = input;
        let index =
            (self.position + self.samples.len() - latency.min(MAX_LATENCY)) % self.samples.len();
        let output = self.samples[index];
        self.position = (self.position + 1) % self.samples.len();
        output
    }
}

pub struct Processor {
    pub key: PrepareKey,
    effect: EffectInstance,
    pub modulation: TrackModulation,
    state: ModulationState,
    dry: DryDelay,
    latency: usize,
    waveform: WaveformClock,
    waveform_telemetry: Option<Arc<ModulationTelemetry>>,
    span_remaining: usize,
    span_processed: usize,
    skip_precise_sample: bool,
    span_track: TrackEffect,
    span_bpm: f64,
    wave_bank: tiny_skia_widgets::mseg::WavePaletteSnapshot,
    audition: AuditionRuntime,
}

impl Processor {
    pub fn set_host_position(&mut self, position: tract_effects::modulation::HostPosition) {
        self.state.set_host_position(position);
    }
    /// Construction and sample-rate setup are restricted to initialize/background work.
    pub fn new(key: PrepareKey, track: &TrackEffect, modulation: TrackModulation) -> Self {
        let mut effect = EffectInstance::new(key.kind);
        effect.set_sample_rate(f32::from_bits(key.sample_rate_bits));
        for (i, _) in effect.parameters().iter().enumerate() {
            effect.set_param(i, track.params[i]);
        }
        let latency = effect.latency_samples();
        Self {
            key,
            effect,
            modulation,
            state: ModulationState::default(),
            dry: DryDelay::new(),
            latency,
            waveform: WaveformClock::default(),
            waveform_telemetry: None,
            span_remaining: 0,
            span_processed: 0,
            skip_precise_sample: false,
            span_track: *track,
            span_bpm: 120.0,
            wave_bank: tiny_skia_widgets::mseg::WavePaletteSnapshot::default(),
            audition: AuditionRuntime::new(MAX_LATENCY),
        }
    }

    /// Attach measurements during processor preparation, before audio-thread handoff.
    pub fn connect_waveform(&mut self, telemetry: &Arc<ModulationTelemetry>) {
        self.waveform_telemetry = Some(telemetry.clone());
    }
    pub fn inherit_precise_timing(&mut self, previous: &Self) {
        if self.key.kind == previous.key.kind
            && self.key.sample_rate_bits != previous.key.sample_rate_bits
        {
            self.state.inherit_precise_timing(&previous.state);
            self.state.inherit_midi_wave(&previous.state);
            self.wave_bank.clone_from(&previous.wave_bank);
        }
    }

    pub fn begin_block(&mut self, track: &TrackEffect, left: &[f32], right: &[f32], bpm: f64) {
        self.prepare_block(track, left, right, bpm);
        self.advance_segment(track, left.len().min(right.len()), bpm);
    }

    pub fn prepare_block(&mut self, track: &TrackEffect, left: &[f32], right: &[f32], bpm: f64) {
        self.prepare_block_with_sidechain(track, left, right, SidechainInput::default(), bpm);
    }

    pub fn prepare_block_with_sidechain(
        &mut self,
        track: &TrackEffect,
        left: &[f32],
        right: &[f32],
        sidechain: SidechainInput<'_>,
        bpm: f64,
    ) {
        if !left.is_empty() && !right.is_empty() {
            let request = self.waveform_telemetry.as_ref().and_then(|telemetry| {
                let request = telemetry.audition.snapshot()?;
                if request.row != 0 {
                    telemetry.audition.release(request.token);
                    return None;
                }
                telemetry.audition.resolve(
                    request,
                    self.key.kind,
                    &self.modulation,
                    telemetry.waveform.generation(),
                )
            });
            self.audition
                .prepare(request, f32::from_bits(self.key.sample_rate_bits) as f64);
            self.state
                .prepare_wave_bank(&self.modulation, &self.wave_bank);
        }
        self.effect.set_bpm(bpm as f32);
        for (i, _) in self.effect.parameters().iter().enumerate() {
            if !self.state.holds_smoothed_parameter(track.kind, i) {
                self.effect.set_param(i, track.params[i]);
            }
        }
        let sample_rate = f32::from_bits(self.key.sample_rate_bits) as f64;
        self.state
            .begin_block(&self.modulation, left, right, sample_rate);
        self.skip_precise_sample = false;
        if self.state.precise_trigger_active()
            && let Some((&l, &r)) = left.first().zip(right.first())
        {
            if self
                .state
                .next_precise_trigger_with_sidechain([l, r], sidechain.sample(0))
            {
                self.state.trigger();
            }
            self.skip_precise_sample = true;
        }
        if let Some(telemetry) = &self.waveform_telemetry {
            self.waveform.begin_block(&telemetry.waveform);
        }
        self.latency = self.effect.latency_samples();
    }

    pub fn advance_segment(&mut self, track: &TrackEffect, samples: usize, bpm: f64) {
        if samples == 0 {
            return;
        }
        self.span_track = *track;
        self.span_bpm = bpm;
        self.span_remaining = samples;
        self.span_processed = 0;
        let sample_rate = f32::from_bits(self.key.sample_rate_bits) as f64;
        if let Some(telemetry) = &self.waveform_telemetry {
            let request = self.waveform.request();
            if !self.state.waveform_running(&self.modulation, request.slot) {
                self.waveform.pause();
            } else {
                self.waveform.prepare_segment_with_curve(
                    &telemetry.waveform,
                    0,
                    WaveformTiming {
                        config: &self.modulation,
                        kind: self.key.kind,
                        phase: self.state.waveform_phase(&self.modulation, request.slot),
                        trigger_epoch: self.state.capture_epoch(&self.modulation, request.slot),
                        bpm,
                        sample_rate,
                    },
                    self.state.effective_mseg(&self.modulation, request.slot),
                );
            }
        }
        self.state.advance_segment(
            &self.modulation,
            track,
            &mut self.effect,
            samples,
            bpm,
            sample_rate,
        );
        self.latency = self.effect.latency_samples();
    }

    pub fn midi_note_on(
        &mut self,
        event: tract_effects::modulation::MidiNoteOn,
        within_span: bool,
    ) -> bool {
        self.state
            .select_midi_wave(&self.modulation, &self.wave_bank, event);
        self.state
            .midi_note_on(&self.modulation, event, within_span)
    }

    pub fn uses_midi_waves(&self) -> bool {
        self.modulation.wave_switch.is_some()
    }
    pub fn uses_project_waves(&self) -> bool {
        self.uses_midi_waves() || self.state.wave_automation() != [0; 4]
    }
    pub fn set_wave_automation(&mut self, values: [u8; 4]) {
        self.state.set_wave_automation(values);
    }
    pub fn set_wave_palette(&mut self, palette: &tiny_skia_widgets::mseg::WavePalette) {
        self.wave_bank.refresh(palette);
    }
    pub fn accepts_midi(&self, event: tract_effects::modulation::MidiNoteOn) -> bool {
        self.modulation.trigger.accepts_midi(event)
            || self
                .modulation
                .wave_switch
                .and_then(|mapping| mapping.slot(event))
                .is_some_and(|slot| self.wave_bank.get(slot).is_some())
    }

    pub fn process_sample(&mut self, input: [f32; 2], mix: f32, bypass: bool) -> [f32; 2] {
        self.process_sample_with_sidechain(input, [0.0; 2], mix, bypass)
    }

    pub fn process_sample_with_sidechain(
        &mut self,
        input: [f32; 2],
        sidechain: [f32; 2],
        mix: f32,
        bypass: bool,
    ) -> [f32; 2] {
        if self.skip_precise_sample {
            self.skip_precise_sample = false;
        } else if self.span_remaining > 0
            && self
                .state
                .next_precise_trigger_with_sidechain(input, sidechain)
        {
            if self.span_processed > 0 {
                self.state.trigger_in_span();
            } else {
                self.state.trigger();
            }
            let track = self.span_track;
            self.advance_segment(&track, self.span_remaining, self.span_bpm);
        }
        if let Some(slot) = self.state.refresh_pending_wave(
            &self.modulation,
            &self.span_track,
            &mut self.effect,
            self.span_remaining,
        ) {
            self.latency = self.effect.latency_samples();
            if self.waveform.request().slot == slot {
                self.waveform
                    .update_curve(self.state.effective_mseg(&self.modulation, slot));
            }
        }
        self.state
            .process_modulation_sample(&self.modulation, &mut self.effect, input, sidechain);
        self.latency = self.effect.latency_samples();
        let dry = self.dry.next(input, self.latency);
        let output = if self.key.kind == EffectKind::None {
            dry
        } else {
            let wet = self.effect.process_sample(input[0], input[1]);
            let mix = self.state.next_mix(mix).clamp(0.0, 1.0);
            if bypass {
                dry
            } else {
                let amp = self.state.amplitude();
                [
                    dry[0] * (1.0 - mix) + wet.0 * amp * mix,
                    dry[1] * (1.0 - mix) + wet.1 * amp * mix,
                ]
            }
        };
        if self.waveform.active()
            && let Some(telemetry) = &self.waveform_telemetry
        {
            self.waveform.sync_position(
                &telemetry.waveform,
                self.state
                    .capture_epoch(&self.modulation, self.waveform.request().slot),
                self.state.precise_phase(self.waveform.request().slot),
            );
            self.waveform.record_sample_with_sidechain(
                &telemetry.waveform,
                input,
                output,
                sidechain,
                self.state.precise_phase(self.waveform.request().slot),
            );
        }
        self.span_remaining = self.span_remaining.saturating_sub(1);
        self.span_processed += 1;
        if let Some(telemetry) = &self.waveform_telemetry {
            self.audition.next(
                output,
                input,
                sidechain,
                &telemetry.audition,
                self.latency,
                !bypass,
            )
        } else {
            output
        }
    }

    pub fn finish_waveform_block(&self) {
        if let Some(telemetry) = &self.waveform_telemetry {
            self.waveform.finish_block(&telemetry.waveform);
        }
    }

    pub fn reset_modulation(&mut self) {
        self.audition.reset();
        if let Some(telemetry) = &self.waveform_telemetry {
            telemetry.audition.invalidate();
        }
        self.skip_precise_sample = false;
        self.state.reset();
    }
    pub fn retrigger_modulation(&mut self) {
        self.skip_precise_sample = false;
        self.state.restart_clock(&self.modulation);
    }
    pub fn reset(&mut self) {
        self.skip_precise_sample = false;
        self.flush_dsp();
        self.state.reset();
    }
    pub fn flush_dsp(&mut self) {
        self.audition.reset();
        if let Some(telemetry) = &self.waveform_telemetry {
            telemetry.audition.invalidate();
        }
        self.effect.reset();
        self.dry.reset();
        self.state.reset_smoothing();
    }
    pub fn latency(&self) -> usize {
        self.latency
    }
    pub fn phase(&self, slot: usize) -> f32 {
        self.state.phase(slot)
    }
    pub fn set_telemetry_enabled(&mut self, enabled: bool) {
        self.state.set_telemetry_enabled(enabled);
    }
    pub fn modulation_frame(&self) -> tract_effects::modulation_telemetry::ModulationFrame {
        tract_effects::modulation_telemetry::ModulationFrame::capture(
            self.key.kind,
            &self.modulation,
            &self.state,
        )
    }
}

#[cfg(test)]
mod audition_tests;
#[cfg(test)]
mod follower_tests;
#[cfg(test)]
mod pitch_tests;
#[cfg(test)]
mod position_tests;
#[cfg(test)]
mod wave_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::neutral_modulation;
    use tiny_skia_widgets::SyncMode;
    use tract_effects::effects::default_params_for_kind;
    use tract_effects::modulation::ModulationTarget;

    fn processor(kind: EffectKind) -> (Processor, TrackEffect) {
        let track = TrackEffect {
            kind,
            params: default_params_for_kind(kind),
            ..TrackEffect::default()
        };
        let p = Processor::new(
            PrepareKey {
                kind,
                generation: 1,
                sample_rate_bits: 48000.0_f32.to_bits(),
            },
            &track,
            neutral_modulation(),
        );
        (p, track)
    }

    #[test]
    fn first_sample_audio_trigger_does_not_apply_the_old_future_fft_endpoint() {
        let (mut p, mut track) = processor(EffectKind::SpectralShift);
        let fft = p
            .effect
            .parameters()
            .iter()
            .position(|spec| spec.name == "FFT")
            .unwrap();
        track.params[fft] = 0.0;
        let mut curve = tiny_skia_widgets::MsegData {
            play_mode: tiny_skia_widgets::PlayMode::OneShot,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..tiny_skia_widgets::MsegData::default()
        };
        curve.insert_node(0.5, 1.0).unwrap();
        curve.nodes[0].stepped = true;
        p.modulation.msegs[1] = curve;
        p.modulation.targets[0] = Some(ModulationTarget::EffectParameter(fft));
        p.modulation.depths[0] = 1.0;
        let input = vec![0.0; 36000];
        p.begin_block(&track, &input, &input, 120.0);
        assert_eq!(p.latency(), 4096);
        p.effect.set_param(fft, 0.0);
        for _ in 0..8192 {
            p.effect.process_sample(0.1, 0.1);
        }
        assert!(
            p.effect.process_sample(0.1, 0.1).0 > 0.05,
            "FFT history must be audible for this regression"
        );
        p.modulation.trigger = tract_effects::modulation::TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        let mut output = [0.0; 2];
        assert_no_alloc::assert_no_alloc(|| {
            p.begin_block(&track, &[2.0; 64], &[2.0; 64], 120.0);
            output = p.process_sample([2.0; 2], 1.0, false);
        });
        assert_eq!(p.latency(), 512);
        assert!(
            output[0] > 0.05,
            "a surplus old-endpoint FFT setter erased the warmed history"
        );
        p.modulation.trigger = tract_effects::modulation::TriggerSource::Free;
        p.begin_block(&track, &input, &input, 120.0);
        p.modulation.trigger = tract_effects::modulation::TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 50.0,
        };
        p.begin_block(&track, &[0.0; 64], &[0.0; 64], 120.0);
        assert_eq!(p.latency(), 4096);
        p.process_sample([0.0; 2], 0.0, true);
        for (i, sample) in p.dry.samples.iter_mut().enumerate() {
            *sample = [i as f32; 2];
        }
        p.dry.position = 0;
        assert_no_alloc::assert_no_alloc(|| output = p.process_sample([2.0; 2], 0.0, true));
        assert_eq!(p.latency(), 512);
        assert_eq!(
            output, [3585.0; 2],
            "dry lookup used the pre-trigger latency"
        );
    }

    #[test]
    fn mix_mseg_ramps_the_audio_blend_without_allocating_or_editing_the_base() {
        let (mut modulated, mut track) = processor(EffectKind::Bitcrush);
        track.params[0] = 4.0;
        track.mix = 0.0;
        modulated.modulation.targets[0] = Some(ModulationTarget::Mix);
        modulated.modulation.depths[0] = 1.0;
        modulated.modulation.msegs[1].polarity = tiny_skia_widgets::Polarity::Unipolar;
        for node in &mut modulated.modulation.msegs[1].nodes {
            node.value = 1.0;
        }
        let (mut wet, mut wet_track) = processor(EffectKind::Bitcrush);
        wet_track.params = track.params;
        wet_track.mix = 1.0;
        let input = [0.6; 8];
        assert_no_alloc::assert_no_alloc(|| {
            modulated.begin_block(&track, &input, &input, 120.0);
            wet.begin_block(&wet_track, &input, &input, 120.0);
            for sample in 0..input.len() {
                let wet = wet.process_sample([0.6; 2], 1.0, false);
                let actual = modulated.process_sample([0.6; 2], track.mix, false);
                let mix = (sample + 1) as f32 / input.len() as f32;
                assert!((actual[0] - (0.6 + (wet[0] - 0.6) * mix)).abs() < 1e-6);
                assert!((actual[1] - actual[0]).abs() < 1e-6);
            }
        });
        assert_eq!(track.mix, 0.0);
    }

    #[test]
    fn native_and_weighted_msegs_sample_and_process_without_allocating() {
        use tiny_skia_widgets::mseg::{MsegData, PointWeight, SegmentShape, value_at_phase};
        let (mut processor, track) = processor(EffectKind::Bitcrush);
        let input = [0.1; 32];
        for shape in [SegmentShape::Curve, SegmentShape::SCurve] {
            for weight in [PointWeight::Hard, PointWeight::Medium, PointWeight::Soft] {
                let mut curve = MsegData::default();
                curve.nodes[0].shape = shape;
                curve.nodes[0].tension = -0.6;
                curve.nodes[0].weight = weight;
                curve.nodes[1].weight = PointWeight::Soft;
                processor.modulation.msegs[0] = curve;
                processor.modulation.msegs[1] = curve;
                processor.modulation.targets[0] = Some(ModulationTarget::Mix);
                processor.modulation.depths[0] = 0.5;
                assert_no_alloc::assert_no_alloc(|| {
                    for sample in 0..4096 {
                        let value = value_at_phase(&curve, sample as f32 / 4096.0);
                        assert!((0.0..=1.0).contains(&value));
                    }
                    for _ in 0..64 {
                        processor.begin_block(&track, &input, &input, 120.0);
                        for _ in 0..input.len() {
                            assert!(
                                processor
                                    .process_sample([0.1; 2], track.mix, false)
                                    .iter()
                                    .all(|value| value.is_finite())
                            );
                        }
                    }
                });
            }
        }
    }

    #[test]
    fn smoothing_and_requested_measurements_allocate_nothing_and_continue_during_bypass() {
        use tract_effects::modulation_telemetry::{ModulationTelemetry, TraceClock};
        let (mut processor, track) = processor(EffectKind::Bitcrush);
        for doc in &mut processor.modulation.msegs {
            *doc = tiny_skia_widgets::MsegData {
                play_mode: tiny_skia_widgets::PlayMode::Cyclic,
                time_seconds: 0.1,
                ..tiny_skia_widgets::MsegData::default()
            };
        }
        processor.modulation.smooth_ms = [20.0; 4];
        processor.modulation.targets = [
            Some(ModulationTarget::EffectParameter(0)),
            Some(ModulationTarget::Mix),
            None,
        ];
        processor.modulation.depths = [0.4, 0.5, 0.0];
        let telemetry = ModulationTelemetry::default();
        telemetry.request(0, 1, track.kind, true);
        let mut clock = TraceClock::default();
        let input = [0.1; 64];
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..100 {
                clock.begin_block(&telemetry, 48000.0);
                processor.begin_block(&track, &input, &input, 120.0);
                for _ in 0..64 {
                    assert_eq!(
                        processor.process_sample([0.1; 2], track.mix, true),
                        [0.1; 2]
                    );
                    if clock.tick() {
                        clock.record(&telemetry, 0, &processor.modulation_frame());
                    }
                }
                clock.publish(&telemetry, 0, &processor.modulation_frame());
            }
        });
        assert!(processor.phase(0) > 0.0);
        let phase = processor.phase(0);
        processor.flush_dsp();
        assert_eq!(processor.phase(0), phase);
    }

    #[test]
    fn none_is_passthrough_even_with_a_silent_amp_curve() {
        let (mut p, track) = processor(EffectKind::None);
        for node in &mut p.modulation.msegs[0].nodes {
            node.value = 0.0;
        }
        p.begin_block(&track, &[0.25; 32], &[-0.5; 32], 120.0);
        assert_eq!(p.process_sample([0.25, -0.5], 1.0, false), [0.25, -0.5]);
    }

    #[test]
    fn waveform_capture_preserves_audio_and_records_all_samples_without_allocating() {
        for kind in [EffectKind::Bitcrush, EffectKind::None] {
            for bypass in [false, true] {
                for smooth in [0.0, 20.0] {
                    let (mut p, track) = processor(kind);
                    let (mut reference, _) = processor(kind);
                    p.modulation.smooth_ms[1] = smooth;
                    reference.modulation = p.modulation.clone();
                    let shared = Arc::new(ModulationTelemetry::default());
                    p.connect_waveform(&shared);
                    let token = shared.waveform.request(0, 1, kind);
                    let input = [0.1; 64];
                    let mut expected_peak = 0.0_f32;
                    assert_no_alloc::assert_no_alloc(|| {
                        for _ in 0..4 {
                            p.begin_block(&track, &input, &input, 120.0);
                            reference.begin_block(&track, &input, &input, 120.0);
                            for sample in 0..64 {
                                let value = if sample == 17 {
                                    [0.9, -0.2]
                                } else {
                                    [0.1, -0.3]
                                };
                                let expected = reference.process_sample(value, track.mix, bypass);
                                assert_eq!(p.process_sample(value, track.mix, bypass), expected);
                                expected_peak =
                                    expected_peak.max(expected[0].abs()).max(expected[1].abs());
                            }
                            p.finish_waveform_block();
                        }
                    });
                    let points: Vec<_> = (0..shared.waveform.head())
                        .filter_map(|index| shared.waveform.point(index, token))
                        .collect();
                    assert!(points.len() > 1);
                    assert_eq!(
                        points.iter().map(|point| point.input).fold(0.0, f32::max),
                        0.9
                    );
                    assert_eq!(
                        points.iter().map(|point| point.output).fold(0.0, f32::max),
                        expected_peak
                    );
                    shared.waveform.release(token);
                    let head = shared.waveform.head();
                    p.begin_block(&track, &input, &input, 120.0);
                    p.process_sample([1.0; 2], track.mix, bypass);
                    p.finish_waveform_block();
                    assert_eq!(shared.waveform.head(), head);
                }
            }
        }
    }

    #[test]
    fn every_effect_processes_and_resets_without_audio_thread_allocations() {
        let input = [0.1; 512];
        for kind in EffectKind::ALL {
            let (mut p, mut track) = processor(kind);
            if kind == EffectKind::Satch {
                // Detail=0 skips the spectral path.
                track.params[2] = 50.0;
            }
            let fft_param = tract_effects::effects::parameter_specs(kind)
                .iter()
                .position(|spec| spec.name == "FFT");
            let fft_settings = if fft_param.is_some() { 4 } else { 1 };
            for fft in 0..fft_settings {
                if let Some(index) = fft_param {
                    track.params[index] = fft as f32;
                }
                assert_no_alloc::assert_no_alloc(|| {
                    // Four frames even at the largest supported 4096-point FFT.
                    for _ in 0..32 {
                        p.begin_block(&track, &input, &input, 123.0);
                        for _ in 0..input.len() {
                            assert!(
                                p.process_sample([0.1, -0.1], 1.0, false)
                                    .iter()
                                    .all(|v| v.is_finite())
                            );
                        }
                    }
                    p.reset_modulation();
                    p.reset();
                });
                assert!(p.latency() <= MAX_LATENCY);
            }
        }
    }

    #[test]
    fn dry_and_bypass_impulses_follow_spectral_latency_at_each_fft_size() {
        for fft in 0..4 {
            for bypass in [false, true] {
                let (mut p, mut track) = processor(EffectKind::SpectralShift);
                track.params[2] = fft as f32;
                p.begin_block(&track, &[1.0], &[1.0], 120.0);
                let latency = p.latency();
                assert_eq!(latency, [512, 1024, 2048, 4096][fft]);
                for i in 0..latency + 2 {
                    let input = if i == 0 { [1.0, -0.5] } else { [0.0; 2] };
                    let expected = if i == latency { [1.0, -0.5] } else { [0.0; 2] };
                    assert_eq!(p.process_sample(input, 0.0, bypass), expected);
                }
            }
        }
    }

    #[test]
    fn clocks_keep_running_and_config_edits_preserve_phase() {
        let (mut p, track) = processor(EffectKind::Svf);
        p.modulation.msegs[1].sync_mode = SyncMode::Time;
        p.modulation.msegs[1].time_seconds = 1.0;
        let input = [0.0; 2400];
        p.begin_block(&track, &input, &input, 120.0);
        assert!((p.phase(1) - 0.05).abs() < 1e-6);
        p.modulation.depths[0] = 0.7;
        p.begin_block(&track, &input, &input, 90.0);
        assert!((p.phase(1) - 0.1).abs() < 1e-6);
        p.reset_modulation();
        assert_eq!(p.phase(1), 0.0);
    }

    #[test]
    fn modulation_reset_keeps_delay_tail_but_host_reset_clears_it() {
        let (mut p, mut track) = processor(EffectKind::Delay);
        track.params[0] = 10.0;
        p.begin_block(&track, &[1.0], &[1.0], 120.0);
        p.process_sample([1.0; 2], 1.0, false);
        p.reset_modulation();
        let mut peak = 0.0_f32;
        for _ in 0..1000 {
            peak = peak.max(p.process_sample([0.0; 2], 1.0, false)[0].abs());
        }
        assert!(peak > 0.01);
        p.reset();
        for _ in 0..1000 {
            assert_eq!(p.process_sample([0.0; 2], 1.0, false), [0.0; 2]);
        }
    }
}
