use super::phase_next;
use tiny_skia_widgets::{HoldMode, MsegData, PlayMode, SyncMode};

#[cfg(test)]
mod playback_tests;

/// Song coordinates at the first sample of a host process block.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HostPosition {
    pub playing: bool,
    pub beats: Option<f64>,
    pub seconds: Option<f64>,
    pub bpm: f64,
    pub loop_beats: Option<(f64, f64)>,
    pub loop_seconds: Option<(f64, f64)>,
}
impl HostPosition {
    fn normalized(mut self) -> Self {
        self.beats = self.beats.filter(|value| value.is_finite());
        self.seconds = self.seconds.filter(|value| value.is_finite());
        if !self.bpm.is_finite() || self.bpm <= 0.0 {
            self.bpm = 0.0;
        }
        let valid = |&(start, end): &(f64, f64)| {
            start.is_finite() && end.is_finite() && end > start && (end - start).is_finite()
        };
        self.loop_beats = self.loop_beats.filter(valid);
        self.loop_seconds = self.loop_seconds.filter(valid);
        self
    }
    fn domain(self, mode: SyncMode) -> (Option<f64>, f64, Option<(f64, f64)>) {
        match mode {
            SyncMode::Beat => (self.beats, self.bpm / 60.0, self.loop_beats),
            SyncMode::Time => (self.seconds, 1.0, self.loop_seconds),
        }
    }
    fn point(self, mode: SyncMode, sample: usize, sample_rate: f64) -> Option<f64> {
        let (position, rate, range) = self.domain(mode);
        let position = position?;
        let mut value = position;
        if self.playing {
            value += sample as f64 / sample_rate * rate;
            if let Some((start, end)) = range
                && sample > 0
                && rate > 0.0
                && position <= end
                && value >= end
            {
                value = start + (value - end).rem_euclid(end - start);
            }
        }
        value.is_finite().then_some(value)
    }
    fn loop_count(self, mode: SyncMode, sample: usize, sample_rate: f64) -> f64 {
        let (Some(position), rate, Some((start, end))) = self.domain(mode) else {
            return 0.0;
        };
        if !self.playing || rate <= 0.0 || position > end || sample == 0 {
            return 0.0;
        }
        let value = position + sample as f64 / sample_rate * rate;
        if value < end {
            0.0
        } else {
            1.0 + ((value - end) / (end - start)).floor()
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct PositionClock {
    pub(super) requested: HostPosition,
    frame: HostPosition,
    sample_rate: f64,
    offset: usize,
    pub(super) active: bool,
    epochs: [u64; 2],
}
impl PositionClock {
    pub(super) fn reset(&mut self) {
        self.active = false;
        self.offset = 0;
    }
    pub(super) fn prepare(&mut self, enabled: bool, sr: f64) -> bool {
        let frame = self.requested.normalized();
        let active = enabled && sr.is_finite() && sr > 0.0;
        let mut changed = if active
            && (!self.active || sr != self.sample_rate || frame.playing != self.frame.playing)
        {
            3
        } else {
            0
        };
        if active && self.active {
            // The next sample can be a loop return even when its new block position is continuous.
            changed |= self.jump_mask();
            for mode in [SyncMode::Beat, SyncMode::Time] {
                let expected = self.frame.point(mode, self.offset, self.sample_rate);
                let current = frame.point(mode, 0, sr);
                let tolerance = (0.5 * self.frame.domain(mode).1 / self.sample_rate).max(1e-9);
                let moved = match (expected, current) {
                    (Some(a), Some(b)) => (a - b).abs() > tolerance,
                    (None, None) => false,
                    _ => true,
                };
                if moved || frame.domain(mode).2 != self.frame.domain(mode).2 {
                    changed |= domain_bit(mode);
                }
            }
        }
        self.frame = frame;
        self.sample_rate = sr;
        self.offset = 0;
        self.active = active;
        self.bump_epochs(changed);
        changed != 0
    }
    fn cycles(&self, doc: &MsegData) -> Option<f64> {
        if !self.active {
            return None;
        }
        let length = match doc.sync_mode {
            SyncMode::Beat => doc.beats as f64,
            SyncMode::Time => doc.time_seconds as f64,
        };
        if !length.is_finite() || length <= 0.0 {
            return None;
        }
        let cycles = self
            .frame
            .point(doc.sync_mode, self.offset, self.sample_rate)?
            / length;
        if !cycles.is_finite() {
            return None;
        }
        Some(cycles)
    }
    pub(super) fn phase(&self, doc: &MsegData) -> Option<f64> {
        Some(phase_next(doc, 0.0, self.cycles(doc)?).clamp(0.0, 1.0))
    }
    pub(super) fn loop_iteration(&self, doc: &MsegData) -> Option<f64> {
        let cycles = self.cycles(doc)?;
        let iteration = match (doc.play_mode, doc.hold) {
            (PlayMode::Cyclic, HoldMode::Loop { start, end }) => {
                let lo = doc.nodes[start].time as f64;
                let span = (doc.nodes[end].time as f64 - lo).max(1e-9);
                ((cycles - lo) / span).floor()
            }
            (PlayMode::Cyclic, _) => cycles.floor(),
            (PlayMode::OneShot, HoldMode::Loop { start, end })
                if cycles >= doc.nodes[end].time as f64 =>
            {
                let hi = doc.nodes[end].time as f64;
                let span = (hi - doc.nodes[start].time as f64).max(1e-9);
                1.0 + ((cycles - hi) / span).floor()
            }
            _ => 0.0,
        };
        iteration.is_finite().then_some(iteration)
    }
    pub(super) fn running(&self, doc: &MsegData) -> bool {
        !self.active
            || (self.frame.playing
                && self.frame.domain(doc.sync_mode).1 > 0.0
                && self.phase(doc).is_some())
    }
    pub(super) fn jump_mask(&self) -> u8 {
        if !self.active || self.offset == 0 {
            return 0;
        }
        [SyncMode::Beat, SyncMode::Time]
            .into_iter()
            .fold(0, |mask, mode| {
                mask | if self.frame.loop_count(mode, self.offset, self.sample_rate)
                    != self
                        .frame
                        .loop_count(mode, self.offset - 1, self.sample_rate)
                {
                    domain_bit(mode)
                } else {
                    0
                }
            })
    }
    pub(super) fn epoch(&self, mode: SyncMode) -> u64 {
        self.epochs[domain_index(mode)]
    }
    pub(super) fn bump_epochs(&mut self, domains: u8) {
        for (i, epoch) in self.epochs.iter_mut().enumerate() {
            if domains & (1 << i) != 0 {
                *epoch = epoch.wrapping_add(1);
            }
        }
    }
    pub(super) fn advance_sample(&mut self) {
        if self.active {
            self.offset = self.offset.saturating_add(1);
        }
    }
}

fn domain_index(mode: SyncMode) -> usize {
    match mode {
        SyncMode::Beat => 0,
        SyncMode::Time => 1,
    }
}
fn domain_bit(mode: SyncMode) -> u8 {
    1 << domain_index(mode)
}

impl super::ModulationState {
    pub fn set_host_position(&mut self, position: HostPosition) {
        self.position.requested = position;
    }
    pub fn waveform_phase(&self, config: &super::TrackModulation, slot: usize) -> f64 {
        self.position
            .phase(self.effective_mseg(config, slot))
            .unwrap_or_else(|| self.precise_phase(slot))
    }
    pub fn waveform_running(&self, config: &super::TrackModulation, slot: usize) -> bool {
        if config.trigger.pitch_settings().is_some() {
            return self.pitch.running();
        }
        config.trigger != super::TriggerSource::DawPosition
            || (self.position.active && self.position.running(self.effective_mseg(config, slot)))
    }
    pub fn capture_epoch(&self, config: &super::TrackModulation, slot: usize) -> u64 {
        let epoch = if config.trigger == super::TriggerSource::DawPosition
            || config.trigger.pitch_settings().is_some()
        {
            self.capture_epochs[slot]
        } else {
            self.trigger_epoch
        };
        epoch.wrapping_add(self.timing.capture_epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia_widgets::{HoldMode, MsegData, PlayMode, SyncMode};

    fn frame(beats: f64, seconds: f64) -> HostPosition {
        HostPosition {
            playing: true,
            beats: Some(beats),
            seconds: Some(seconds),
            bpm: 120.0,
            ..HostPosition::default()
        }
    }
    fn curve(mode: SyncMode) -> MsegData {
        MsegData {
            sync_mode: mode,
            play_mode: PlayMode::Cyclic,
            beats: 4.0,
            time_seconds: 2.0,
            ..MsegData::default()
        }
    }
    #[test]
    fn position_uses_the_first_sample_and_independent_beat_and_second_lengths() {
        let mut clock = PositionClock::default();
        clock.requested = frame(5.0, 3.0);
        assert!(clock.prepare(true, 48000.0));
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), Some(0.25));
        assert_eq!(clock.phase(&curve(SyncMode::Time)), Some(0.5));
        clock.advance_sample();
        assert!(
            (clock.phase(&curve(SyncMode::Beat)).unwrap() - 0.25 - 1.0 / 96000.0).abs() < 1e-12
        );
        assert!((clock.phase(&curve(SyncMode::Time)).unwrap() - 0.5 - 1.0 / 96000.0).abs() < 1e-12);
    }

    #[test]
    fn host_loop_counts_and_positions_follow_multiple_returns_in_each_time_domain() {
        let mut host = frame(2.5, 6.75);
        host.loop_beats = Some((1.0, 3.0));
        host.loop_seconds = Some((4.0, 7.0));
        for (sample, position, count) in [
            (0, 2.5, 0.0),
            (1, 1.0, 1.0),
            (4, 2.5, 1.0),
            (5, 1.0, 2.0),
            (9, 1.0, 3.0),
        ] {
            assert_eq!(host.point(SyncMode::Beat, sample, 4.0), Some(position));
            assert_eq!(host.loop_count(SyncMode::Beat, sample, 4.0), count);
        }
        for (sample, position, count) in [
            (0, 6.75, 0.0),
            (1, 4.0, 1.0),
            (12, 6.75, 1.0),
            (13, 4.0, 2.0),
            (25, 4.0, 3.0),
        ] {
            assert_eq!(host.point(SyncMode::Time, sample, 4.0), Some(position));
            assert_eq!(host.loop_count(SyncMode::Time, sample, 4.0), count);
        }
        host.beats = Some(3.0);
        assert_eq!(host.point(SyncMode::Beat, 0, 4.0), Some(3.0));
        assert_eq!(host.loop_count(SyncMode::Beat, 0, 4.0), 0.0);
        assert_eq!(host.loop_count(SyncMode::Beat, 1, 4.0), 1.0);
        host.bpm = 0.0;
        assert_eq!(host.loop_count(SyncMode::Beat, 1, 4.0), 0.0);
        host.bpm = 120.0;
        host.beats = Some(3.5);
        assert_eq!(host.point(SyncMode::Beat, 1, 4.0), Some(4.0));
        assert_eq!(host.loop_count(SyncMode::Beat, 1, 4.0), 0.0);
        host.playing = false;
        assert_eq!(host.point(SyncMode::Time, 25, 4.0), Some(6.75));
        assert_eq!(host.loop_count(SyncMode::Time, 25, 4.0), 0.0);
    }

    #[test]
    fn loop_iterations_follow_the_cyclic_wrap_or_one_shot_intro_at_exact_boundaries() {
        let mut doc = curve(SyncMode::Time);
        doc.time_seconds = 1.0;
        let start = doc.insert_node(0.25, 0.25).unwrap();
        let end = doc.insert_node(0.75, 0.75).unwrap();
        let mut clock = PositionClock::default();
        for (cycles, cyclic, one_shot, whole_curve) in [
            (-0.5, -2.0, 0.0, -1.0),
            (0.0, -1.0, 0.0, 0.0),
            (0.25, 0.0, 0.0, 0.0),
            (0.625, 0.0, 0.0, 0.0),
            (0.75, 1.0, 1.0, 0.0),
            (1.125, 1.0, 1.0, 1.0),
            (1.25, 2.0, 2.0, 1.0),
            (1.75, 3.0, 3.0, 1.0),
            (2.25, 4.0, 4.0, 2.0),
        ] {
            clock.requested = frame(0.0, cycles);
            clock.prepare(true, 48000.0);
            doc.hold = HoldMode::Loop { start, end };
            doc.play_mode = PlayMode::OneShot;
            assert_eq!(
                clock.loop_iteration(&doc),
                Some(one_shot),
                "one-shot {cycles}"
            );
            doc.play_mode = PlayMode::Cyclic;
            assert_eq!(clock.loop_iteration(&doc), Some(cyclic), "cyclic {cycles}");
            doc.hold = HoldMode::None;
            assert_eq!(
                clock.loop_iteration(&doc),
                Some(whole_curve),
                "whole curve {cycles}"
            );
        }
        clock.reset();
        assert_eq!(clock.loop_iteration(&doc), None);
        assert_eq!(clock.phase(&doc), None);
        assert!(!clock.prepare(true, 0.0));
        assert_eq!(clock.phase(&doc), None);
    }

    #[test]
    fn host_loop_ranges_and_tempo_reject_degenerate_or_nonfinite_values() {
        for bad_loop in [
            (1.0, 1.0),
            (2.0, 1.0),
            (1.0, f64::INFINITY),
            (f64::NEG_INFINITY, 1.0),
            (-f64::MAX, f64::MAX),
        ] {
            let mut host = frame(0.0, 0.0);
            host.loop_beats = Some(bad_loop);
            host.loop_seconds = Some(bad_loop);
            let normalized = host.normalized();
            assert_eq!(normalized.loop_beats, None);
            assert_eq!(normalized.loop_seconds, None);
        }
        let mut host = frame(0.0, 0.0);
        host.loop_seconds = Some((1.0e308, 1.5e308));
        assert_eq!(host.normalized().loop_seconds, host.loop_seconds);
        for bpm in [0.0, -120.0, f64::NAN, f64::INFINITY] {
            host.bpm = bpm;
            assert_eq!(host.normalized().bpm, 0.0);
        }
    }

    #[test]
    fn block_position_jitter_at_half_a_sample_does_not_bump_capture_epochs() {
        let mut clock = PositionClock::default();
        clock.requested = frame(0.0, 0.0);
        clock.requested.seconds = None;
        assert!(clock.prepare(true, 1024.0));
        clock.advance_sample();
        let epochs = [clock.epoch(SyncMode::Beat), clock.epoch(SyncMode::Time)];

        // An expected 2/1024 beats at this sample plus half a beat-sample.
        clock.requested.beats = Some(3.0 / 1024.0);
        assert!(!clock.prepare(true, 1024.0));
        assert_eq!(
            [clock.epoch(SyncMode::Beat), clock.epoch(SyncMode::Time)],
            epochs
        );

        // More than half a sample with the time domain still unavailable.
        clock.requested.beats = Some(5.0 / 1024.0);
        assert!(clock.prepare(true, 1024.0));
        assert_eq!(clock.epoch(SyncMode::Beat), epochs[0] + 1);
        assert_eq!(clock.epoch(SyncMode::Time), epochs[1]);
    }

    #[test]
    fn disabling_position_ignores_seeks_until_reactivation_at_the_same_rate() {
        let mut clock = PositionClock::default();
        clock.requested = frame(2.0, 0.75);
        assert!(clock.prepare(true, 48000.0));
        let epochs = [clock.epoch(SyncMode::Beat), clock.epoch(SyncMode::Time)];

        clock.requested = frame(4.0, 1.25);
        assert!(!clock.prepare(false, 48000.0));
        assert_eq!(
            [clock.epoch(SyncMode::Beat), clock.epoch(SyncMode::Time)],
            epochs
        );
        assert_eq!(clock.phase(&curve(SyncMode::Time)), None);
        assert!(clock.prepare(true, 48000.0));
        assert_eq!(clock.epoch(SyncMode::Beat), epochs[0] + 1);
        assert_eq!(clock.epoch(SyncMode::Time), epochs[1] + 1);
        assert_eq!(clock.phase(&curve(SyncMode::Time)), Some(0.625));
        let invalid = MsegData {
            time_seconds: -2.0,
            ..curve(SyncMode::Time)
        };
        assert_eq!(clock.phase(&invalid), None);
    }

    #[test]
    fn waveform_capture_uses_the_host_phase_for_a_stopped_daw_clock() {
        use crate::modulation::{ModulationState, TrackModulation, TriggerSource};
        let cfg = TrackModulation {
            trigger: TriggerSource::DawPosition,
            msegs: [curve(SyncMode::Time); 4],
            ..TrackModulation::default()
        };
        let mut state = ModulationState::default();
        let mut host = frame(2.0, 0.75);
        host.playing = false;
        state.set_host_position(host);
        state.begin_block(&cfg, &[0.0; 8], &[0.0; 8], 48000.0);
        assert_eq!(state.waveform_phase(&cfg, 0), 0.375);
        assert!(!state.waveform_running(&cfg, 0));
    }
    #[test]
    fn negative_positions_and_hold_modes_follow_existing_playback_rules() {
        let mut clock = PositionClock::default();
        clock.requested = frame(-1.0, -0.5);
        clock.prepare(true, 48000.0);
        let mut doc = curve(SyncMode::Beat);
        assert_eq!(clock.phase(&doc), Some(0.75));
        doc.play_mode = PlayMode::OneShot;
        assert_eq!(clock.phase(&doc), Some(0.0));
        clock.requested = frame(12.0, 6.0);
        clock.prepare(true, 48000.0);
        assert_eq!(clock.phase(&doc), Some(1.0));
        doc.insert_node(0.5, 0.5).unwrap();
        doc.hold = HoldMode::Sustain(1);
        assert_eq!(clock.phase(&doc), Some(0.5));
        doc.hold = HoldMode::Loop { start: 0, end: 1 };
        assert_eq!(clock.phase(&doc), Some(0.0));
    }
    #[test]
    fn loops_wrap_inside_a_buffer_and_mark_the_actual_return_sample() {
        let mut clock = PositionClock::default();
        clock.requested = HostPosition {
            loop_beats: Some((2.0, 3.0)),
            loop_seconds: Some((1.0, 1.5)),
            ..frame(2.75, 1.375)
        };
        clock.prepare(true, 8.0);
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), Some(0.6875));
        assert_eq!(clock.jump_mask(), 0);
        clock.advance_sample();
        assert_eq!(clock.jump_mask(), 3);
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), Some(0.5));
        assert_eq!(clock.phase(&curve(SyncMode::Time)), Some(0.5));
        for _ in 0..3 {
            clock.advance_sample();
        }
        clock.requested = HostPosition {
            beats: Some(2.75),
            seconds: Some(1.375),
            ..clock.requested
        };
        assert!(!clock.prepare(true, 8.0));
    }
    #[test]
    fn continuous_blocks_tempo_changes_seeks_and_stop_start_are_distinguished() {
        let mut clock = PositionClock::default();
        clock.requested = frame(1.0, 0.5);
        clock.prepare(true, 48000.0);
        for _ in 0..480 {
            clock.advance_sample();
        }
        clock.requested = frame(1.02, 0.51);
        clock.requested.bpm = 60.0;
        assert!(!clock.prepare(true, 48000.0));
        clock.requested = frame(8.0, 4.0);
        assert!(clock.prepare(true, 48000.0));
        clock.requested.playing = false;
        assert!(clock.prepare(true, 48000.0));
        let phase = clock.phase(&curve(SyncMode::Beat));
        for _ in 0..100 {
            clock.advance_sample();
        }
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), phase);
        assert!(!clock.running(&curve(SyncMode::Beat)));
        clock.requested.playing = true;
        assert!(clock.prepare(true, 48000.0));
    }
    #[test]
    fn missing_domains_invalid_ranges_and_extreme_inputs_never_make_phases_nonfinite() {
        let mut clock = PositionClock::default();
        clock.requested = HostPosition {
            beats: Some(f64::NAN),
            seconds: Some(-0.5),
            playing: true,
            bpm: f64::INFINITY,
            loop_seconds: Some((5.0, 1.0)),
            loop_beats: Some((0.0, f64::INFINITY)),
        };
        clock.prepare(true, 48000.0);
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), None);
        assert_eq!(clock.phase(&curve(SyncMode::Time)), Some(0.75));
        let mut doc = curve(SyncMode::Time);
        doc.time_seconds = 0.0;
        assert_eq!(clock.phase(&doc), None);
        doc.time_seconds = f32::NAN;
        assert_eq!(clock.phase(&doc), None);
        clock.requested = frame(f64::MAX, f64::MAX);
        clock.prepare(true, 48000.0);
        for doc in [curve(SyncMode::Beat), curve(SyncMode::Time)] {
            assert!(
                clock
                    .phase(&doc)
                    .is_none_or(|phase| phase.is_finite() && (0.0..=1.0).contains(&phase))
            );
        }
        clock.prepare(true, f64::NAN);
        assert_eq!(clock.phase(&curve(SyncMode::Beat)), None);
    }
}
