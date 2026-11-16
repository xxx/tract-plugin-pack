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
        self.telemetry.release(self.token);
        self.telemetry.waveform.release(self.waveform_token);
    }
}
impl LiveSource {
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
        let token = self.telemetry.request_for_sources(
            self.row,
            slot,
            kind,
            trace,
            config.trigger == crate::modulation::TriggerSource::DawPosition,
            config.follower.normalized().request_key(slot),
        );
        if token != self.token {
            self.bins.fill(None);
            self.frame = None;
            self.cursor = self.telemetry.head();
            self.token = token;
        }
        if let Some(frame) = self
            .telemetry
            .frame(token)
            .filter(|f| f.matches(kind, config) && f.wave_automation == wave_automation)
        {
            self.frame = Some(frame);
        } else if self
            .frame
            .is_some_and(|f| !f.matches(kind, config) || f.wave_automation != wave_automation)
        {
            self.frame = None;
        }
        let head = self.telemetry.head();
        self.cursor = self.cursor.max(head.saturating_sub(TRACE_CAPACITY as u64));
        for index in self.cursor..head {
            if let Some(point) = self.telemetry.point(index, token)
                && point.phase.is_finite()
                && point.value.is_finite()
            {
                let bin =
                    (point.phase.clamp(0.0, 1.0) * (TRACE_CAPACITY - 1) as f32).round() as usize;
                self.bins[bin] = Some((point.phase, point.value));
            }
        }
        self.cursor = head;
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
        }
        self.disconnect_live();
        self.live = Some(LiveSource {
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
                self.trace_enabled,
                self.wave_automation.unwrap_or([0; 4]),
            );
        }
    }
    pub(super) fn draw_live_trace(
        &self,
        pm: &mut tiny_skia::Pixmap,
        rect: (f32, f32, f32, f32),
        scale: f32,
    ) {
        if self.trace_enabled
            && let Some(live) = &self.live
        {
            widgets::mseg::draw_mseg_trace(
                pm,
                &self.mseg_edit.layout(rect, scale),
                &live.bins,
                scale,
                tiny_skia::Color::from_rgba8(235, 245, 255, 210),
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
