use super::*;
use crate::modulation::{AuditionSettings, AuditionSource};

pub(super) struct Lease {
    pub(super) token: u64,
    source: AuditionSource,
}
impl EffectEditor {
    pub(super) fn listen_label(&self) -> &'static str {
        if self.live.as_ref().is_some_and(|live| live.row() < 16) {
            "Listen"
        } else {
            "Offline"
        }
    }
    pub fn listening(&self, source: AuditionSource) -> bool {
        self.live.as_ref().is_some_and(|live| {
            live.audition.as_ref().is_some_and(|lease| {
                lease.source == source && live.audition_control().is_current(lease.token)
            })
        })
    }
    pub(super) fn stop_listening(&mut self) {
        if let Some(live) = &mut self.live
            && let Some(lease) = live.audition.take()
        {
            live.audition_control().release(lease.token);
        }
    }
    pub(super) fn stop_listening_source(&mut self, source: AuditionSource) {
        if self
            .live
            .as_ref()
            .and_then(|live| live.audition.as_ref())
            .is_some_and(|lease| lease.source == source)
        {
            self.stop_listening();
        }
    }
    pub(super) fn sync_listening_panels(&mut self) {
        let keep = self
            .live
            .as_ref()
            .and_then(|live| live.audition.as_ref())
            .is_none_or(|lease| match lease.source {
                AuditionSource::Trigger => self.detector_open,
                AuditionSource::Follower => self.follower.open(),
            });
        if !keep {
            self.stop_listening();
        }
    }
}
impl Session<'_> {
    pub(super) fn toggle_listening(&mut self, source: AuditionSource) {
        if self.state.listening(source) {
            self.state.stop_listening();
            return;
        }
        self.state.stop_listening();
        let Some(settings) = AuditionSettings::for_source(self.modulation, source) else {
            return;
        };
        let Some(live) = &mut self.state.live else {
            self.state
                .mseg_edit
                .set_status_message("Audio monitoring unavailable");
            return;
        };
        let token = live.audition_control().begin(
            live.row(),
            self.effect.kind,
            source,
            settings,
            live.binding_key().2,
        );
        live.audition = Some(Lease { token, source });
    }
    pub(super) fn refresh_listening_settings(&mut self) {
        let Some(live) = &self.state.live else {
            return;
        };
        let Some(lease) = &live.audition else {
            return;
        };
        if let Some(settings) = AuditionSettings::for_source(self.modulation, lease.source) {
            live.audition_control().update(lease.token, settings);
        }
    }
}
