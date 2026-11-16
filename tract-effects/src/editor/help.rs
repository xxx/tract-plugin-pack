use super::*;

impl Session<'_> {
    pub(super) fn update_mseg_hint(&mut self) {
        let strip = self
            .state
            .mseg_edit
            .layout(self.layout.mseg_pane, self.scale_factor)
            .strip;
        if self.modulation.trigger.pitch_settings().is_some()
            && [self.layout.mseg_sync, self.layout.mseg_length]
                .iter()
                .any(|r| view::in_rect(*r, self.mouse_pos.0, self.mouse_pos.1))
        {
            self.state.mseg_edit.set_external_hint(Some("MIDI pitch controls cycle rate; last matching note wins, note-off keeps it running"));
            return;
        }
        if view::in_rect(
            widgets::mseg::follower_button_rect(strip, self.scale_factor),
            self.mouse_pos.0,
            self.mouse_pos.1,
        ) {
            self.state.mseg_edit.set_external_hint(Some(
                "Audio follower: per-curve routing, shared input and response",
            ));
            return;
        }
        if (self.state.mseg_edit.wave_palette().is_some() || self.modulation.wave_switch.is_some())
            && view::in_rect(
                widgets::mseg::wave_midi_button_rect(strip, self.scale_factor),
                self.mouse_pos.0,
                self.mouse_pos.1,
            )
        {
            self.state
                .mseg_edit
                .set_external_hint(Some("Project wave automation and MIDI switching"));
            return;
        }
        let hint = if self.state.effect_dropdown.is_open() || self.state.mseg_edit.has_open_menu() {
            None
        } else {
            match view::effect_hit(
                self.mouse_pos.0,
                self.mouse_pos.1,
                self.scale_factor,
                &self.layout,
                self.effect_param_count(),
                self.state.selected_mseg,
                self.modulation.trigger,
            ) {
                Some(EffectHit::MsegSelector(_)) => {
                    Some("Choose a curve; other curves remain as ghosts")
                }
                Some(EffectHit::Target) => Some("Choose the destination for this assignable curve"),
                Some(EffectHit::Depth) => {
                    Some("Scale modulation; Shift: fine; double-click resets")
                }
                Some(EffectHit::Trigger) => {
                    Some("Choose a shared free, DAW or MIDI pitch clock for all four curves")
                }
                Some(EffectHit::TriggerRate) => Some("Adjust trigger rate or onset sensitivity"),
                Some(EffectHit::TriggerAux) => Some("Set the time between accepted audio triggers"),
                Some(EffectHit::MidiFilter(MidiField::Transpose)) => Some(
                    "Transpose MIDI pitch by -36 to +36 semitones; note-off keeps the last pitch",
                ),
                Some(EffectHit::MidiFilter(MidiField::Channel))
                    if self.modulation.trigger.pitch_settings().is_some() =>
                {
                    Some("Choose the MIDI pitch channel; changing it waits for a new note")
                }
                Some(EffectHit::MidiFilter(_)) => Some("Filter MIDI restarts by note or channel"),
                Some(EffectHit::MsegSync(_)) => Some("Set curve length in seconds or host beats"),
                Some(EffectHit::MsegLength) => Some("Set this curve's playback length"),
                Some(EffectHit::Smooth(_)) => {
                    Some("Lag the source; right-click enters milliseconds")
                }
                Some(EffectHit::Trace) => Some("Show measured modulation output"),
                Some(EffectHit::Audio) => Some("Choose a waveform view or freeze the capture"),
                Some(EffectHit::DetectorSetup) => {
                    Some("Choose detector input, threshold and filters")
                }
                _ => None,
            }
        };
        self.state.mseg_edit.set_external_hint(hint);
    }
}
