use super::*;
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TRACE_CAPACITY};
use crate::waveform::{WAVEFORM_BINS, WAVEFORM_CAPACITY};
use std::sync::Arc;
use widgets::mseg::AudioBin;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioGain {
    Minus12,
    #[default]
    Unity,
    Plus12,
}
impl AudioGain {
    pub fn multiplier(self) -> f32 {
        match self {
            Self::Minus12 => 0.25118864,
            Self::Unity => 1.0,
            Self::Plus12 => 3.9810717,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AudioView {
    #[default]
    Off,
    Input,
    Output,
    Both,
    Sidechain,
}
impl AudioView {
    pub(super) fn input(self) -> bool {
        matches!(self, Self::Input | Self::Both | Self::Sidechain)
    }
    pub(super) fn output(self) -> bool {
        matches!(self, Self::Output | Self::Both)
    }
    fn label(self, frozen: bool) -> &'static str {
        match (self, frozen) {
            (Self::Off, _) => "Audio: Off",
            (Self::Input, false) => "Audio: In",
            (Self::Output, false) => "Audio: Out",
            (Self::Both, false) => "Audio: Both",
            (Self::Input, true) => "Frozen: In",
            (Self::Output, true) => "Frozen: Out",
            (Self::Both, true) => "Frozen: Both",
            (Self::Sidechain, false) => "Audio: SC",
            (Self::Sidechain, true) => "Frozen: SC",
        }
    }
}
pub(super) fn audio_items(state: &EffectEditor) -> Vec<&'static str> {
    vec![
        "Off",
        "Input",
        "Output",
        "Input + Output",
        "Sidechain",
        if state.audio_display == AudioDisplay::Magnitude {
            "✓ Magnitude"
        } else {
            "Magnitude"
        },
        if state.audio_display == AudioDisplay::Combined {
            "✓ Combined"
        } else {
            "Combined"
        },
        if state.audio_display == AudioDisplay::Stereo {
            "✓ Stereo"
        } else {
            "Stereo"
        },
        if state.audio_gain == AudioGain::Minus12 {
            "✓ Gain: -12 dB"
        } else {
            "Gain: -12 dB"
        },
        if state.audio_gain == AudioGain::Unity {
            "✓ Gain: 0 dB"
        } else {
            "Gain: 0 dB"
        },
        if state.audio_gain == AudioGain::Plus12 {
            "✓ Gain: +12 dB"
        } else {
            "Gain: +12 dB"
        },
        if state.audio_frozen {
            "Unfreeze"
        } else {
            "Freeze"
        },
    ]
}

pub(super) struct LiveSource {
    pub(super) audition: Option<super::audition::Lease>,
    telemetry: Arc<ModulationTelemetry>,
    row: usize,
    token: u64,
    cursor: u64,
    pub(super) frame: Option<ModulationFrame>,
    bins: [Option<(f32, f32)>; TRACE_CAPACITY],
    waveform_token: u64,
    waveform_cursor: u64,
    waveform_generation: u64,
    waveform_key: Option<(EffectKind, usize, bool)>,
    pub(super) waveform: [Option<AudioBin>; WAVEFORM_BINS],
}
impl Drop for LiveSource {
    fn drop(&mut self) {
        if let Some(lease) = self.audition.take() {
            self.telemetry.audition.release(lease.token);
        }
        self.telemetry.release(self.token);
        self.telemetry.waveform.release(self.waveform_token);
    }
}
impl LiveSource {
    pub(super) fn audition_control(&self) -> &crate::modulation::AuditionControl {
        &self.telemetry.audition
    }
    pub(super) fn row(&self) -> usize {
        self.row
    }
    pub(super) fn binding_key(&self) -> (usize, usize, u64) {
        (
            Arc::as_ptr(&self.telemetry) as usize,
            self.row,
            self.telemetry.waveform.generation(),
        )
    }
    fn refresh_waveform(&mut self, kind: EffectKind, slot: usize, view: AudioView, frozen: bool) {
        let telemetry = &self.telemetry.waveform;
        let generation = telemetry.generation();
        let key = (kind, slot, view == AudioView::Sidechain);
        if self.waveform_key != Some(key) || self.waveform_generation != generation {
            telemetry.release(self.waveform_token);
            self.waveform_token = 0;
            self.waveform.fill(None);
            self.waveform_generation = generation;
            self.waveform_key = Some(key);
        }
        if view == AudioView::Off || frozen {
            telemetry.release(self.waveform_token);
            self.waveform_token = 0;
            if view == AudioView::Off {
                self.waveform.fill(None);
            }
            return;
        }
        let token = if view == AudioView::Sidechain {
            telemetry.request_sidechain(self.row, slot, kind)
        } else {
            telemetry.request(self.row, slot, kind)
        };
        if token != self.waveform_token {
            self.waveform_token = token;
            self.waveform.fill(None);
            self.waveform_cursor = telemetry.head();
        }
        let head = telemetry.head();
        self.waveform_cursor = self
            .waveform_cursor
            .max(head.saturating_sub(WAVEFORM_CAPACITY as u64));
        for index in self.waveform_cursor..head {
            if let Some(point) = telemetry.stereo_point(index, token) {
                self.waveform[point.bin] = Some(point.audio);
            }
        }
        self.waveform_cursor = head;
    }
    fn refresh(
        &mut self,
        kind: EffectKind,
        config: &TrackModulation,
        slot: usize,
        trace: bool,
        wave_automation: [u8; 4],
    ) {
        let token = self.telemetry.request_with_curve(
            self.row,
            slot,
            kind,
            trace,
            config.trigger == crate::modulation::TriggerSource::DawPosition,
            config.follower.normalized().request_key(slot),
            config.trigger.pitch_settings().is_some(),
            trace.then_some(&config.msegs[slot]),
        );
        if token != self.token {
            self.bins.fill(None);
            self.frame = None;
            self.cursor = self.telemetry.head();
            self.token = token;
        }
        let matches = |frame: &ModulationFrame| {
            frame.matches(kind, config) && frame.wave_automation == wave_automation
        };
        let Some((frame, head)) = self
            .telemetry
            .frame_with_trace_head(token)
            .filter(|(frame, _)| matches(frame))
        else {
            if self.frame.as_ref().is_some_and(|frame| !matches(frame)) {
                self.frame = None;
                self.bins.fill(None);
            }
            return;
        };
        let wave_key = frame.trace_wave_key(slot);
        if self
            .frame
            .is_some_and(|previous| previous.trace_wave_key(slot) != wave_key)
        {
            self.bins.fill(None);
        }
        self.frame = Some(frame);
        self.cursor = self.cursor.max(head.saturating_sub(TRACE_CAPACITY as u64));
        for index in self.cursor..head {
            if let Some(point) = self.telemetry.point(index, token)
                && point.wave_key == wave_key
                && point.phase.is_finite()
                && point.value.is_finite()
            {
                let bin =
                    (point.phase.clamp(0.0, 1.0) * (TRACE_CAPACITY - 1) as f32).round() as usize;
                self.bins[bin] = Some((point.phase, point.value));
            }
        }
        self.cursor = self.cursor.max(head);
    }
}
impl EffectEditor {
    pub fn connect_live(&mut self, telemetry: &Arc<ModulationTelemetry>, row: usize) {
        self.measured_arcs_only = true;
        if self
            .live
            .as_ref()
            .is_some_and(|s| s.row == row && Arc::ptr_eq(&s.telemetry, telemetry))
        {
            return;
        }
        if self.live.is_some() {
            self.cancel_detector();
            self.follower.close();
            self.wave_midi.close();
            self.timing.close();
        }
        self.disconnect_live();
        self.live = Some(LiveSource {
            audition: None,
            telemetry: telemetry.clone(),
            row,
            token: 0,
            cursor: telemetry.head(),
            frame: None,
            bins: [None; TRACE_CAPACITY],
            waveform_token: 0,
            waveform_cursor: telemetry.waveform.head(),
            waveform_generation: telemetry.waveform.generation(),
            waveform_key: None,
            waveform: [None; WAVEFORM_BINS],
        });
    }
    pub fn disconnect_live(&mut self) {
        self.measured_arcs_only = true;
        self.live = None;
    }
    pub fn invalidate_trace(&mut self) {
        self.timing.close();
        self.stop_listening();
        self.cancel_detector();
        self.clear_modulation_trace();
        if let Some(live) = &mut self.live {
            live.telemetry.waveform.release(live.waveform_token);
            live.waveform_token = 0;
            live.waveform.fill(None);
        }
    }
    pub(super) fn clear_modulation_trace(&mut self) {
        if let Some(live) = &mut self.live {
            live.telemetry.release(live.token);
            live.token = 0;
            live.frame = None;
            live.bins.fill(None);
            live.cursor = live.telemetry.head();
        }
    }
    pub fn trace_enabled(&self) -> bool {
        self.trace_enabled
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) {
        if self.trace_enabled != enabled {
            self.trace_enabled = enabled;
            self.clear_modulation_trace();
        }
    }

    fn trace_visible(&self) -> bool {
        self.trace_enabled && !self.mseg_edit.is_editing_curve()
    }
    pub fn set_audio_view(&mut self, view: AudioView) {
        if (self.audio_view == AudioView::Sidechain) != (view == AudioView::Sidechain) {
            self.audio_frozen = false;
            if let Some(live) = &mut self.live {
                live.telemetry.waveform.release(live.waveform_token);
                live.waveform_token = 0;
                live.waveform_key = None;
                live.waveform.fill(None);
            }
        }
        self.audio_view = view;
        if view == AudioView::Off {
            self.audio_frozen = false;
        }
    }
    pub fn audio_view(&self) -> AudioView {
        self.audio_view
    }
    pub fn set_audio_frozen(&mut self, frozen: bool) {
        self.audio_frozen = frozen && self.audio_view != AudioView::Off;
    }
    pub fn audio_frozen(&self) -> bool {
        self.audio_frozen
    }
    pub fn set_audio_display(&mut self, display: AudioDisplay) {
        self.audio_display = display;
    }
    pub fn audio_display(&self) -> AudioDisplay {
        self.audio_display
    }
    pub fn set_audio_gain(&mut self, gain: AudioGain) {
        self.audio_gain = gain;
    }
    pub fn audio_gain(&self) -> AudioGain {
        self.audio_gain
    }
    pub(super) fn refresh_live(&mut self, effect: &TrackEffect, config: &TrackModulation) {
        let trace = self.trace_visible();
        if let Some(live) = &mut self.live {
            live.refresh_waveform(
                effect.kind,
                self.selected_mseg.min(3),
                self.audio_view,
                self.audio_frozen,
            );
            live.refresh(
                effect.kind,
                config,
                self.selected_mseg.min(3),
                trace,
                self.wave_automation.unwrap_or([0; 4]),
            );
        }
    }
    pub(super) fn draw_live_trace(
        &self,
        pm: &mut tiny_skia::Pixmap,
        layout: &widgets::mseg::MsegLayout,
        curve: &widgets::MsegData,
    ) {
        if self.trace_visible()
            && let Some(live) = &self.live
        {
            let (r, g, b, _) = widgets::color_to_rgba_u8(mseg_color(self.selected_mseg));
            let tint = |channel| channel + (255 - channel) / 4;
            widgets::mseg::draw_mseg_trace(
                pm,
                layout,
                self.mseg_edit.display_data(curve),
                &live.bins,
                tiny_skia::Color::from_rgba8(tint(r), tint(g), tint(b), 235),
            );
        }
    }
}

impl Session<'_> {
    pub(super) fn smooth_allowed(&self, slot: usize) -> bool {
        self.modulation.smoothing_supported(slot, self.effect.kind)
    }
    pub(super) fn apply_smooth(&mut self, slot: usize, ms: f32) {
        if slot >= 4 || !self.smooth_allowed(slot) {
            return;
        }
        let ms = crate::modulation::normalized_smooth_ms(ms);
        if self.modulation.smooth_ms[slot] != ms {
            self.modulation.smooth_ms[slot] = ms;
            self.response.changed = true;
        }
    }
    pub(super) fn commit_smooth_text(&mut self, slot: usize, text: &str) {
        if let Ok(value) = text.trim().trim_end_matches("ms").trim().parse::<f64>()
            && value.is_finite()
            && (0.0..=crate::modulation::MAX_SMOOTH_MS as f64).contains(&value)
        {
            self.apply_smooth(slot, value as f32);
        }
    }
    pub(super) fn commit_pending_text(&mut self) {
        match self.state.text_edit.commit() {
            Some((EffectHit::Dial(i), text)) => self.commit_dial_text_edit(i, &text),
            Some((EffectHit::Mix, text)) => self.commit_mix_text_edit(&text),
            Some((EffectHit::Smooth(slot), text)) => self.commit_smooth_text(slot, &text),
            Some((EffectHit::Detector(field), text)) => self.commit_detector_text(field, &text),
            Some((EffectHit::DetectorDetail, text)) => self.commit_detector_detail(&text),
            _ => {}
        }
        self.state.refresh_pitch_grid(self.effect, self.modulation);
    }
    pub(super) fn draw_live_controls(&mut self, pm: &mut tiny_skia::Pixmap) {
        let r = self.layout.audio;
        widgets::dropdown::draw_dropdown_trigger(
            pm,
            self.text_renderer,
            r,
            self.state.audio_view.label(self.state.audio_frozen),
            self.state.effect_dropdown.is_open_for(EffectAction::Audio),
        );
        let slot = self.state.selected_mseg.min(3);
        let allowed = self.smooth_allowed(slot);
        let ms = crate::modulation::normalized_smooth_ms(self.modulation.smooth_ms[slot]);
        let value = if !allowed {
            "n/a".into()
        } else if ms == 0.0 {
            "Off".into()
        } else {
            format!("{ms:.1} ms")
        };
        let hit = EffectHit::Smooth(slot);
        let editing = self.state.text_edit.active_for(&hit);
        let r = self.layout.smooth;
        widgets::draw_slider(
            pm,
            self.text_renderer,
            r.0,
            r.1,
            r.2,
            r.3,
            "Smooth",
            &value,
            if allowed {
                crate::modulation::smooth_to_norm(ms)
            } else {
                0.0
            },
            editing,
            self.state.text_edit.caret_visible(),
        );
        if !allowed {
            widgets::draw_rect_outline(pm, r.0, r.1, r.2, r.3, widgets::color_muted(), 1.0);
        }
        let r = self.layout.trace;
        widgets::controls::draw_button(
            pm,
            self.text_renderer,
            r.0,
            r.1,
            r.2,
            r.3,
            "Trace",
            self.state.trace_enabled,
            false,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulation::ModulationState;
    use crate::modulation_telemetry::TraceClock;

    #[test]
    fn audio_views_select_input_and_output_independently_and_use_linear_gain() {
        for (view, input, output, live, frozen) in [
            (AudioView::Off, false, false, "Audio: Off", "Audio: Off"),
            (AudioView::Input, true, false, "Audio: In", "Frozen: In"),
            (AudioView::Output, false, true, "Audio: Out", "Frozen: Out"),
            (AudioView::Both, true, true, "Audio: Both", "Frozen: Both"),
            (AudioView::Sidechain, true, false, "Audio: SC", "Frozen: SC"),
        ] {
            assert_eq!((view.input(), view.output()), (input, output));
            assert_eq!(view.label(false), live);
            assert_eq!(view.label(true), frozen);
        }
        for (gain, db) in [
            (AudioGain::Minus12, -12.0),
            (AudioGain::Unity, 0.0),
            (AudioGain::Plus12, 12.0),
        ] {
            assert!((20.0 * gain.multiplier().log10() - db).abs() < 1e-5);
        }
    }

    #[test]
    fn trace_bins_preserve_endpoints_and_automation_edits_clear_obsolete_frames() {
        let telemetry = Arc::new(ModulationTelemetry::default());
        let mut editor = EffectEditor::default();
        let effect = TrackEffect {
            kind: EffectKind::Svf,
            ..TrackEffect::default()
        };
        let config = TrackModulation::default();
        editor.connect_live(&telemetry, 13);
        editor.set_trace_enabled(true);
        assert!(editor.trace_enabled());
        editor.refresh_live(&effect, &config);
        assert_eq!(editor.live.as_ref().unwrap().row(), 13);

        let mut clock = TraceClock::default();
        let request = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        assert!(request.matches(13, effect.kind));
        assert!(request.trace);
        let mut frame = ModulationFrame::capture(effect.kind, &config, &ModulationState::default());
        for (phase, value) in [(0.0, 0.25), (0.5, 0.75), (1.0, 0.125)] {
            frame.phases[request.slot] = phase;
            frame.outputs[request.slot] = value;
            clock.record(&telemetry, 13, &frame);
        }
        clock.publish(&telemetry, 13, &frame);
        editor.refresh_live(&effect, &config);
        let live = editor.live.as_ref().unwrap();
        assert_eq!(live.frame, Some(frame));
        assert_eq!(live.bins[0], Some((0.0, 0.25)));
        assert_eq!(live.bins[TRACE_CAPACITY / 2], Some((0.5, 0.75)));
        assert_eq!(live.bins[TRACE_CAPACITY - 1], Some((1.0, 0.125)));

        let mut raster = tiny_skia::Pixmap::new(640, 360).unwrap();
        let rect = (12.0, 12.0, 608.0, 320.0);
        let layout = editor.mseg_edit.layout(rect, 1.0);
        editor.draw_live_trace(&mut raster, &layout, &config.msegs[request.slot]);
        assert!(raster.data().iter().any(|byte| *byte != 0));

        let mut displayed = widgets::MsegData::default();
        displayed.nodes[0].value = 0.25;
        displayed.nodes[1].value = 0.125;
        displayed.insert_node(0.5, 0.75).unwrap();
        editor.mseg_edit.set_read_only_curve(Some(displayed));
        raster.fill(tiny_skia::Color::TRANSPARENT);
        editor.draw_live_trace(&mut raster, &layout, &config.msegs[request.slot]);
        assert!(raster.data().iter().all(|&byte| byte == 0));

        let live = editor.live.as_mut().unwrap();
        live.refresh(effect.kind, &config, request.slot, true, [0, 1, 0, 0]);
        assert!(live.frame.is_none());
    }

    #[test]
    fn trace_discards_retained_and_unread_points_from_previous_midi_wave() {
        let telemetry = Arc::new(ModulationTelemetry::default());
        let mut editor = EffectEditor::default();
        let effect = TrackEffect::default();
        let slot = editor.selected_mseg;
        let mapping = crate::modulation::MidiWaveMapping {
            envelope: slot as u8,
            first_note: 60,
            channel: None,
        };
        let config = TrackModulation {
            wave_switch: Some(mapping),
            ..TrackModulation::default()
        };
        editor.connect_live(&telemetry, 0);
        editor.set_trace_enabled(true);
        editor.refresh_live(&effect, &config);
        let mut clock = TraceClock::default();
        clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        let mut frame = ModulationFrame::capture(effect.kind, &config, &ModulationState::default());
        frame.wave = Some((mapping, Some(0)));
        frame.phases[slot] = 0.0;
        clock.record(&telemetry, 0, &frame);
        clock.publish(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);
        assert!(editor.live.as_ref().unwrap().bins[0].is_some());

        frame.phases[slot] = 0.5;
        clock.record(&telemetry, 0, &frame);
        frame.wave = Some((mapping, Some(1)));
        frame.phases[slot] = 1.0;
        clock.record(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);
        assert!(editor.live.as_ref().unwrap().bins[TRACE_CAPACITY - 1].is_none());
        clock.publish(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);

        let bins = &editor.live.as_ref().unwrap().bins;
        assert!(bins[..TRACE_CAPACITY - 1].iter().all(Option::is_none));
        assert_eq!(bins[TRACE_CAPACITY - 1], Some((1.0, frame.outputs[slot])));
    }

    #[test]
    fn trace_restarts_after_switching_away_and_back_between_measurements() {
        use crate::effects::EffectInstance;
        use crate::modulation::{MidiNoteOn, MidiWaveMapping};
        use widgets::mseg::{FactoryShape, WavePalette, WavePaletteSnapshot, factory_shape};

        let telemetry = Arc::new(ModulationTelemetry::default());
        let mut editor = EffectEditor::default();
        let effect = TrackEffect::default();
        let slot = editor.selected_mseg;
        let mapping = MidiWaveMapping {
            envelope: slot as u8,
            first_note: 60,
            channel: None,
        };
        let config = TrackModulation {
            wave_switch: Some(mapping),
            ..TrackModulation::default()
        };
        let mut palette = WavePalette::default();
        palette
            .store(0, &factory_shape(FactoryShape::RampUp))
            .unwrap();
        palette
            .store(1, &factory_shape(FactoryShape::RampDown))
            .unwrap();
        let mut bank = WavePaletteSnapshot::default();
        bank.refresh(&palette);
        let mut state = ModulationState::default();
        state.prepare_wave_bank(&config, &bank);
        let mut dsp = EffectInstance::new(effect.kind);
        let mut select = |state: &mut ModulationState, note| {
            assert!(state.select_midi_wave(
                &config,
                &bank,
                MidiNoteOn {
                    timing: 0,
                    channel: 0,
                    note
                }
            ));
            state.refresh_pending_wave(&config, &effect, &mut dsp, 64);
        };
        editor.connect_live(&telemetry, 0);
        editor.set_trace_enabled(true);
        editor.refresh_live(&effect, &config);
        let mut clock = TraceClock::default();
        clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        let frame_at = |state: &ModulationState, phase| {
            let mut frame = ModulationFrame::capture(effect.kind, &config, state);
            frame.phases[slot] = phase;
            frame
        };

        select(&mut state, 60);
        let first = frame_at(&state, 0.0);
        clock.record(&telemetry, 0, &first);
        clock.publish(&telemetry, 0, &first);
        editor.refresh_live(&effect, &config);
        assert!(editor.live.as_ref().unwrap().bins[0].is_some());
        clock.record(&telemetry, 0, &frame_at(&state, 0.25));

        select(&mut state, 61);
        select(&mut state, 60);
        let latest = frame_at(&state, 1.0);
        assert_eq!(first.wave, latest.wave);
        clock.record(&telemetry, 0, &latest);
        clock.publish(&telemetry, 0, &latest);
        editor.refresh_live(&effect, &config);

        let bins = &editor.live.as_ref().unwrap().bins;
        assert!(bins[..TRACE_CAPACITY - 1].iter().all(Option::is_none));
        assert_eq!(bins[TRACE_CAPACITY - 1], Some((1.0, latest.outputs[slot])));
    }

    #[test]
    fn trace_restarts_for_edited_curve_and_rejects_readings_until_audio_adopts_it() {
        let telemetry = Arc::new(ModulationTelemetry::default());
        let mut editor = EffectEditor::default();
        let effect = TrackEffect::default();
        let old_config = TrackModulation::default();
        let mut config = old_config.clone();
        let slot = editor.selected_mseg;
        editor.connect_live(&telemetry, 0);
        editor.set_trace_enabled(true);
        editor.refresh_live(&effect, &config);

        let mut clock = TraceClock::default();
        let mut frame = ModulationFrame::capture(effect.kind, &config, &ModulationState::default());
        let old_request = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        assert!(clock.tick());
        frame.phases[slot] = 0.5;
        frame.outputs[slot] = 1.0;
        clock.record(&telemetry, 0, &frame);
        clock.publish(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);
        assert_eq!(
            editor.live.as_ref().unwrap().bins[TRACE_CAPACITY / 2],
            Some((0.5, 1.0))
        );

        config.msegs[slot].nodes[1].value = 0.25;
        editor.refresh_live(&effect, &config);
        let waiting = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            old_config.msegs.get(request.slot)
        });
        assert_ne!(waiting.token, old_request.token);
        assert!(!clock.tick());
        editor.refresh_live(&effect, &config);
        assert!(
            editor
                .live
                .as_ref()
                .unwrap()
                .bins
                .iter()
                .all(Option::is_none)
        );

        let adopted = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        assert_eq!(adopted.token, waiting.token);
        assert!(clock.tick());
        frame.outputs[slot] = 0.25;
        clock.record(&telemetry, 0, &frame);
        clock.publish(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);
        assert_eq!(
            editor.live.as_ref().unwrap().bins[TRACE_CAPACITY / 2],
            Some((0.5, 0.25))
        );

        config.msegs[slot].snap = !config.msegs[slot].snap;
        editor.refresh_live(&effect, &config);
        assert_eq!(editor.live.as_ref().unwrap().token, adopted.token);
    }

    #[test]
    fn trace_pauses_during_curve_gestures_and_resumes_with_fresh_history() {
        let telemetry = Arc::new(ModulationTelemetry::default());
        let mut editor = EffectEditor::default();
        let effect = TrackEffect::default();
        let mut config = TrackModulation::default();
        let slot = editor.selected_mseg;
        let rect = (12.0, 12.0, 880.0, 320.0);
        editor.connect_live(&telemetry, 0);
        editor.set_trace_enabled(true);
        editor.refresh_live(&effect, &config);
        let mut clock = TraceClock::default();
        let before = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        let mut frame = ModulationFrame::capture(effect.kind, &config, &ModulationState::default());
        frame.outputs[slot] = 1.0;
        clock.record(&telemetry, 0, &frame);
        clock.publish(&telemetry, 0, &frame);
        editor.refresh_live(&effect, &config);
        assert!(editor.live.as_ref().unwrap().bins[0].is_some());

        let layout = editor.mseg_edit.layout(rect, 1.0);
        let node = config.msegs[slot].nodes[1];
        editor.mseg_edit.on_mouse_down(
            widgets::mseg::phase_to_x(&layout, node.time),
            widgets::mseg::value_to_y(&layout, node.value),
            &mut config.msegs[slot],
            rect,
            1.0,
            false,
        );
        assert!(editor.mseg_edit.is_editing_curve());
        let mut raster = tiny_skia::Pixmap::new(920, 360).unwrap();
        editor.draw_live_trace(&mut raster, &layout, &config.msegs[slot]);
        assert!(raster.data().iter().all(|&byte| byte == 0));
        editor.refresh_live(&effect, &config);
        let paused = clock.begin_block(&telemetry, 48000.0);
        assert!(!paused.trace);
        assert_ne!(paused.token, before.token);
        assert!(editor.trace_enabled());

        let _ = editor
            .mseg_edit
            .on_mouse_up(&mut config.msegs[slot], rect, 1.0);
        editor.refresh_live(&effect, &config);
        let after = clock.begin_block_for_curve(&telemetry, 48000.0, |request| {
            config.msegs.get(request.slot)
        });
        assert!(after.trace);
        assert_ne!(after.token, before.token);
        assert!(
            editor
                .live
                .as_ref()
                .unwrap()
                .bins
                .iter()
                .all(Option::is_none)
        );
    }
}
