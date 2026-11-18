use super::*;
use crate::effects::{
    Effect, EffectKind, MAX_EFFECT_PARAMS, ParamSpec, TrackEffect, default_params_for_kind,
};
use crate::modulation::{ModulationState, ModulationTarget, TrackModulation, TriggerSource};
use tiny_skia_widgets::{PlayMode, Polarity};

struct Probe {
    values: [f32; MAX_EFFECT_PARAMS],
    sets: [u32; MAX_EFFECT_PARAMS],
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            values: default_params_for_kind(EffectKind::Svf),
            sets: [0; MAX_EFFECT_PARAMS],
        }
    }
}
impl Effect for Probe {
    fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        (l, r)
    }
    fn parameters(&self) -> &'static [ParamSpec] {
        crate::effects::parameter_specs(EffectKind::Svf)
    }
    fn set_param(&mut self, i: usize, value: f32) {
        self.values[i] = value;
        self.sets[i] += 1;
    }
    fn set_sample_rate(&mut self, _: f32) {}
    fn reset(&mut self) {}
}
fn config() -> TrackModulation {
    let mut cfg = TrackModulation {
        trigger: TriggerSource::DawPosition,
        targets: [
            Some(ModulationTarget::EffectParameter(0)),
            None,
            Some(ModulationTarget::Mix),
        ],
        depths: [0.05, 0.0, 1.0],
        ..TrackModulation::default()
    };
    for (slot, curve) in cfg.msegs.iter_mut().enumerate() {
        *curve = MsegData {
            play_mode: PlayMode::Cyclic,
            polarity: Polarity::Unipolar,
            sync_mode: if slot % 2 == 0 {
                SyncMode::Beat
            } else {
                SyncMode::Time
            },
            beats: if slot == 0 { 4.0 } else { 2.0 },
            time_seconds: if slot == 1 { 2.0 } else { 1.0 },
            ..MsegData::default()
        };
    }
    cfg
}
fn frame(beats: f64, seconds: f64) -> HostPosition {
    HostPosition {
        playing: true,
        beats: Some(beats),
        seconds: Some(seconds),
        bpm: 120.0,
        ..HostPosition::default()
    }
}
#[derive(Clone, Debug, PartialEq)]
struct Reading {
    phase: [f64; 4],
    source: [f32; 4],
    mix: f32,
    values: [f32; MAX_EFFECT_PARAMS],
    sets: [u32; MAX_EFFECT_PARAMS],
}
fn block(
    state: &mut ModulationState,
    cfg: &TrackModulation,
    effect: &mut Probe,
    frame: HostPosition,
    spans: &[usize],
) -> Vec<Reading> {
    let count = spans.iter().sum::<usize>();
    let input = [0.0; 1024];
    let track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        mix: 0.0,
        ..TrackEffect::default()
    };
    let mut out = Vec::with_capacity(count);
    state.set_host_position(frame);
    state.begin_block(cfg, &input[..count], &input[..count], 48000.0);
    for &span in spans {
        state.advance_segment(cfg, &track, effect, span, frame.bpm, 48000.0);
        for sample in 0..span {
            state.refresh_pending_wave(cfg, &track, effect, span - sample);
            state.process_smoothing_sample(cfg, effect);
            out.push(Reading {
                phase: std::array::from_fn(|slot| state.precise_phase(slot)),
                source: std::array::from_fn(|slot| state.output(slot)),
                mix: state.next_mix(0.0),
                values: effect.values,
                sets: effect.sets,
            });
        }
    }
    out
}

#[test]
fn daw_sources_and_mix_use_sample_zero_and_then_each_actual_sample_with_smooth_off() {
    let mut state = ModulationState::default();
    let samples = block(
        &mut state,
        &config(),
        &mut Probe::default(),
        frame(1.0, 0.125),
        &[8],
    );
    assert_eq!(samples[0].phase, [0.25, 0.0625, 0.5, 0.125]);
    assert_eq!(samples[0].source, [0.25, 0.0625, 0.5, 0.125]);
    assert!((samples[0].mix - 0.125).abs() < 1e-7);
    for (index, sample) in samples.iter().enumerate() {
        assert!((sample.phase[0] - (1.0 + index as f64 / 24000.0) / 4.0).abs() < 1e-12);
        assert_eq!(sample.source[0], sample.phase[0] as f32);
        assert!((sample.mix - sample.phase[3] as f32).abs() < 1e-7);
    }
}
#[test]
fn host_and_grid_splits_keep_daw_audio_and_fixed_parameter_cadence() {
    for ms in [0.0, 10.0] {
        let mut cfg = config();
        cfg.smooth_ms = [ms; 4];
        let mut reference = ModulationState::default();
        let mut actual = ModulationState::default();
        let mut grid = ModulationState::default();
        let mut effect = Probe::default();
        let expected = block(
            &mut reference,
            &cfg,
            &mut Probe::default(),
            frame(1.0, 0.125),
            &[256],
        );
        let same_host = block(
            &mut grid,
            &cfg,
            &mut Probe::default(),
            frame(1.0, 0.125),
            &[17, 48, 91, 100],
        );
        assert_eq!(same_host, expected);
        let mut split = Vec::new();
        for len in [5, 7, 31, 1, 48, 64, 100] {
            let offset = split.len() as f64;
            split.extend(block(
                &mut actual,
                &cfg,
                &mut effect,
                frame(1.0 + offset / 24000.0, 0.125 + offset / 48000.0),
                &[len],
            ));
        }
        for (a, b) in split.iter().zip(&expected) {
            assert_eq!(a.sets, b.sets);
            for slot in 0..4 {
                assert!((a.phase[slot] - b.phase[slot]).abs() < 1e-12);
                assert!((a.source[slot] - b.source[slot]).abs() < 2e-7);
            }
            assert!((a.mix - b.mix).abs() < 2e-7);
            assert!((a.values[0] - b.values[0]).abs() < 0.01);
        }
        assert_eq!(expected.last().unwrap().sets[0], 6);
    }
}
#[test]
fn a_missing_beat_rate_cannot_project_a_loop_return_from_its_end_marker() {
    let cfg = config();
    let mut state = ModulationState::default();
    let samples = block(
        &mut state,
        &cfg,
        &mut Probe::default(),
        HostPosition {
            bpm: 0.0,
            loop_beats: Some((2.0, 3.0)),
            ..frame(3.0, 1.5)
        },
        &[3],
    );
    assert!(
        samples
            .iter()
            .all(|sample| sample.phase[0] == 0.75 && sample.source[0] == 0.75)
    );
    assert!(!state.waveform_running(&cfg, 0));
    assert!(state.waveform_running(&cfg, 1));
}

#[test]
fn a_loop_return_at_a_host_boundary_matches_the_same_return_inside_a_buffer() {
    let mut cfg = config();
    cfg.msegs[1].sync_mode = SyncMode::Beat;
    cfg.msegs[1].beats = 4.0;
    let mut actual = ModulationState::default();
    let mut reference = ModulationState::default();
    let mut effect = Probe::default();
    let first = HostPosition {
        loop_beats: Some((2.0, 2.75 + 5.0 / 24000.0)),
        loop_seconds: Some((1.0, 1.375 + 5.0 / 48000.0)),
        ..frame(2.75, 1.375)
    };
    let expected = block(&mut reference, &cfg, &mut Probe::default(), first, &[13]);
    let mut samples = block(&mut actual, &cfg, &mut effect, first, &[5]);
    samples.extend(block(
        &mut actual,
        &cfg,
        &mut effect,
        HostPosition {
            beats: Some(2.0),
            seconds: Some(1.0),
            ..first
        },
        &[8],
    ));
    for (a, b) in samples.iter().zip(&expected) {
        assert_eq!(a.sets, b.sets);
        assert!((a.values[0] - b.values[0]).abs() < 0.01);
        for slot in 0..4 {
            assert!((a.phase[slot] - b.phase[slot]).abs() < 1e-12);
        }
    }
    for slot in 0..4 {
        assert_eq!(
            actual.capture_epoch(&cfg, slot),
            reference.capture_epoch(&cfg, slot)
        );
    }
}

#[test]
fn seeks_keep_lag_but_refresh_targets_before_the_next_control_tick() {
    let mut cfg = config();
    cfg.smooth_ms = [10.0; 4];
    let mut state = ModulationState::default();
    let mut effect = Probe::default();
    let warm = block(&mut state, &cfg, &mut effect, frame(0.0, 0.0), &[5]);
    let previous = warm.last().unwrap().source[0];
    let sets = effect.sets[0];
    let next = block(&mut state, &cfg, &mut effect, frame(3.0, 0.75), &[1]);
    let gain = -(-1.0_f64 / 480.0).exp_m1();
    assert!(
        (next[0].source[0] as f64 - (previous as f64 + gain * (0.75 - previous as f64))).abs()
            < 1e-7
    );
    assert_eq!(next[0].phase[0], 0.75);
    assert_eq!(effect.sets[0], sets + 1);
}
#[test]
fn stopped_clocks_hold_position_and_missing_beats_leave_time_capture_and_targets_alone() {
    let cfg = config();
    let mut state = ModulationState::default();
    let mut effect = Probe::default();
    let stopped = block(
        &mut state,
        &cfg,
        &mut effect,
        HostPosition {
            playing: false,
            ..frame(2.0, 0.25)
        },
        &[16],
    );
    assert!(
        stopped
            .iter()
            .all(|sample| sample.phase == stopped[0].phase)
    );
    assert!(!state.waveform_running(&cfg, 0));
    block(&mut state, &cfg, &mut effect, frame(2.0, 0.25), &[16]);
    let previous = state.precise_phase(0);
    let time_epoch = state.capture_epoch(&cfg, 1);
    let sets = effect.sets[0];
    let missing = block(
        &mut state,
        &cfg,
        &mut effect,
        HostPosition {
            beats: None,
            ..frame(0.0, 0.25 + 16.0 / 48000.0)
        },
        &[1],
    );
    assert_eq!(missing[0].phase[0], previous);
    assert_eq!(state.capture_epoch(&cfg, 1), time_epoch);
    assert_eq!(effect.sets[0], sets);
    assert!(!state.waveform_running(&cfg, 0));
    assert!(state.waveform_running(&cfg, 1));
}
#[test]
fn timing_edits_refresh_only_the_affected_capture_and_target() {
    let mut cfg = config();
    let mut state = ModulationState::default();
    let mut effect = Probe::default();
    block(&mut state, &cfg, &mut effect, frame(2.0, 0.25), &[5]);
    let epochs = std::array::from_fn::<_, 4, _>(|slot| state.capture_epoch(&cfg, slot));
    let sets = effect.sets[0];
    cfg.msegs[1].time_seconds = 4.0;
    let next = block(
        &mut state,
        &cfg,
        &mut effect,
        frame(2.0 + 5.0 / 24000.0, 0.25 + 5.0 / 48000.0),
        &[1],
    );
    assert!((next[0].phase[1] - (0.25 + 5.0 / 48000.0) / 4.0).abs() < 1e-12);
    assert_ne!(state.capture_epoch(&cfg, 1), epochs[1]);
    assert_eq!(state.capture_epoch(&cfg, 0), epochs[0]);
    assert_eq!(state.capture_epoch(&cfg, 3), epochs[3]);
    assert_eq!(effect.sets[0], sets + 1);
    cfg.msegs[1].insert_node(0.5, 0.5).unwrap();
    cfg.msegs[1].hold = tiny_skia_widgets::HoldMode::Loop { start: 0, end: 1 };
    block(&mut state, &cfg, &mut effect, frame(2.0, 0.25), &[1]);
    let before = state.capture_epoch(&cfg, 1);
    cfg.msegs[1].nodes[1].time = 0.6;
    block(
        &mut state,
        &cfg,
        &mut effect,
        frame(2.0 + 1.0 / 24000.0, 0.25 + 1.0 / 48000.0),
        &[1],
    );
    assert_ne!(state.capture_epoch(&cfg, 1), before);
}
#[test]
fn empty_host_buffers_do_not_apply_intermediate_seeks_or_lag_changes() {
    let mut cfg = config();
    cfg.smooth_ms = [10.0; 4];
    let mut actual = ModulationState::default();
    let mut reference = ModulationState::default();
    let mut effect = Probe::default();
    let mut other = Probe::default();
    for (state, probe) in [(&mut actual, &mut effect), (&mut reference, &mut other)] {
        block(state, &cfg, probe, frame(1.0, 0.125), &[5]);
    }
    block(&mut actual, &cfg, &mut effect, frame(200.0, 100.0), &[]);
    let next = frame(1.0 + 5.0 / 24000.0, 0.125 + 5.0 / 48000.0);
    assert_eq!(
        block(&mut actual, &cfg, &mut effect, next, &[8]),
        block(&mut reference, &cfg, &mut other, next, &[8])
    );
}
#[test]
fn enum_targets_bypass_lag_and_stay_on_the_fixed_control_clock() {
    let mut cfg = config();
    cfg.targets[0] = Some(ModulationTarget::EffectParameter(2));
    cfg.smooth_ms[1] = 500.0;
    let samples = block(
        &mut ModulationState::default(),
        &cfg,
        &mut Probe::default(),
        frame(1.0, 0.25),
        &[49],
    );
    assert_eq!(samples[0].source[1], 0.125);
    assert_eq!(samples[48].source[1], samples[48].phase[1] as f32);
    assert_eq!(samples.last().unwrap().sets[2], 2);
}
#[test]
fn daw_position_persists_without_changing_legacy_defaults() {
    let saved = serde_json::to_string(&config()).unwrap();
    let restored: TrackModulation = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.trigger, TriggerSource::DawPosition);
    assert_eq!(restored, config());
    assert_eq!(TrackModulation::default().trigger, TriggerSource::Free);
    assert!(
        !serde_json::to_string(&TrackModulation::default())
            .unwrap()
            .contains("position")
    );
}
