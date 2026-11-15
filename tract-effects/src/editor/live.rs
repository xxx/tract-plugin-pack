use super::*;
use crate::modulation_telemetry::{ModulationFrame, ModulationTelemetry, TRACE_CAPACITY};
use std::sync::Arc;

pub(super) struct LiveSource {
    telemetry: Arc<ModulationTelemetry>,
    row: usize,
    token: u64,
    cursor: u64,
    pub(super) frame: Option<ModulationFrame>,
    bins: [Option<(f32, f32)>; TRACE_CAPACITY],
}
impl Drop for LiveSource {
    fn drop(&mut self) {
        self.telemetry.release(self.token);
    }
}
impl LiveSource {
    fn refresh(&mut self, kind: EffectKind, config: &TrackModulation, slot: usize, trace: bool) {
        let token = self.telemetry.request(self.row, slot, kind, trace);
        if token != self.token {
            self.bins.fill(None);
            self.frame = None;
            self.cursor = self.telemetry.head();
            self.token = token;
        }
        if let Some(frame) = self
            .telemetry
            .frame(token)
            .filter(|f| f.matches(kind, config))
        {
            self.frame = Some(frame);
        } else if self.frame.is_some_and(|f| !f.matches(kind, config)) {
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
        self.disconnect_live();
        self.live = Some(LiveSource {
            telemetry: telemetry.clone(),
            row,
            token: 0,
            cursor: telemetry.head(),
            frame: None,
            bins: [None; TRACE_CAPACITY],
        });
    }
    pub fn disconnect_live(&mut self) {
        self.measured_arcs_only = true;
        self.live = None;
    }
    pub fn invalidate_trace(&mut self) {
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
    pub(super) fn refresh_live(&mut self, effect: &TrackEffect, config: &TrackModulation) {
        if let Some(live) = &mut self.live {
            live.refresh(
                effect.kind,
                config,
                self.selected_mseg.min(3),
                self.trace_enabled,
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
            _ => {}
        }
    }
    pub(super) fn draw_live_controls(&mut self, pm: &mut tiny_skia::Pixmap) {
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
