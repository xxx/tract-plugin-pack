use super::*;
use crate::effects::{
    Effect, EffectKind, MAX_EFFECT_PARAMS, ParamSpec, TrackEffect, default_params_for_kind,
};
use crate::modulation::{ModulationState, ModulationTarget, TrackModulation};
use tiny_skia_widgets::{MsegData, PlayMode, Polarity, SyncMode};

struct Probe {
    values: [f32; MAX_EFFECT_PARAMS],
    sets: [usize; MAX_EFFECT_PARAMS],
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
    fn parameters(&self) -> &'static [ParamSpec] {
        crate::effects::parameter_specs(EffectKind::Svf)
    }
    fn set_param(&mut self, i: usize, value: f32) {
        self.values[i] = value;
        self.sets[i] += 1;
    }
    fn process_sample(&mut self, l: f32, r: f32) -> (f32, f32) {
        (l, r)
    }
    fn set_sample_rate(&mut self, _: f32) {}
    fn reset(&mut self) {}
}
fn config() -> TrackModulation {
    let mut cfg = TrackModulation {
        targets: [
            Some(ModulationTarget::EffectParameter(0)),
            None,
            Some(ModulationTarget::Mix),
        ],
        depths: [0.05, 0.0, 1.0],
        ..TrackModulation::default()
    };
    cfg.msegs = std::array::from_fn(|_| {
        let mut doc = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            polarity: Polarity::Unipolar,
            ..MsegData::default()
        };
        for node in doc.nodes.iter_mut().take(doc.node_count) {
            node.value = 1.0;
        }
        doc
    });
    cfg.follower.attack_ms = 0.0;
    cfg.follower.release_ms = 0.0;
    for route in &mut cfg.follower.routes {
        route.enabled = true;
    }
    cfg
}
fn block(
    state: &mut ModulationState,
    cfg: &TrackModulation,
    probe: &mut Probe,
    input: &[[f32; 2]],
) -> Vec<([f32; 4], f32)> {
    let track = TrackEffect {
        kind: EffectKind::Svf,
        params: default_params_for_kind(EffectKind::Svf),
        mix: 0.0,
        ..TrackEffect::default()
    };
    let zeros = vec![0.0; input.len()];
    state.begin_block(cfg, &zeros, &zeros, 48000.0);
    state.advance_segment(cfg, &track, probe, input.len(), 120.0, 48000.0);
    input
        .iter()
        .map(|sample| {
            state.process_modulation_sample(cfg, probe, *sample, [0.0; 2]);
            (
                std::array::from_fn(|slot| state.output(slot)),
                state.next_mix(0.0),
            )
        })
        .collect()
}
#[test]
fn follower_routes_update_each_sample_and_keep_parameter_setters_on_the_control_clock() {
    let cfg = config();
    let mut state = ModulationState::default();
    let mut probe = Probe::default();
    let input: Vec<_> = (0..100).map(|i| [0.1, -(i as f32 / 100.0)]).collect();
    let output = block(&mut state, &cfg, &mut probe, &input);
    for (i, (sources, mix)) in output.iter().enumerate() {
        let level = (i as f32 / 100.0).max(0.1);
        for value in sources {
            assert!((value - level).abs() < 1e-7);
        }
        assert!((mix - level).abs() < 1e-7);
    }
    assert_eq!(probe.sets[0], 3);
}
#[test]
fn disabling_continuous_mix_retains_its_blend_when_the_curve_result_is_identical() {
    for other_route in [false, true] {
        let mut cfg = config();
        for route in &mut cfg.follower.routes {
            route.enabled = false;
        }
        cfg.follower.routes[3].enabled = true;
        cfg.follower.routes[1].enabled = other_route;
        let mut state = ModulationState::default();
        let mut probe = Probe::default();
        assert!(
            block(&mut state, &cfg, &mut probe, &[[1.0; 2]; 5])
                .iter()
                .all(|(_, mix)| *mix == 1.0)
        );
        cfg.follower.routes[3].enabled = false;
        assert!(
            block(&mut state, &cfg, &mut probe, &[[1.0; 2]; 8])
                .iter()
                .all(|(_, mix)| *mix == 1.0)
        );
    }
}
#[test]
fn follower_attack_precedes_source_lag_and_retriggers_keep_envelope_history() {
    let mut cfg = config();
    cfg.follower.attack_ms = 10.0;
    cfg.smooth_ms[0] = 20.0;
    let mut state = ModulationState::default();
    let mut probe = Probe::default();
    let out = block(&mut state, &cfg, &mut probe, &[[1.0; 2]]);
    let attack = -(-1.0_f64 / 480.0).exp_m1();
    let lag = -(-1.0_f64 / 960.0).exp_m1();
    assert!((out[0].0[0] as f64 - attack * lag).abs() < 1e-8);
    let envelope = state.follower_reading().unwrap().envelope;
    state.trigger();
    assert_eq!(state.follower_reading().unwrap().envelope, envelope);
    state.restart_clock(&cfg);
    assert_eq!(state.follower_reading().unwrap().envelope, envelope);
    state.reset_smoothing();
    assert_eq!(state.follower_reading().unwrap().envelope, envelope);
    state.reset();
    assert_eq!(state.follower_reading().unwrap().envelope, 0.0);
}
#[test]
fn follower_playback_is_host_partition_invariant_and_zero_frames_do_not_apply_input_excursions() {
    let mut cfg = config();
    cfg.follower.attack_ms = 3.0;
    cfg.follower.hold_ms = 2.0;
    cfg.follower.release_ms = 15.0;
    cfg.smooth_ms = [5.0; 4];
    let input: Vec<_> = (0..256)
        .map(|i| if i % 61 < 20 { [0.7, -0.3] } else { [0.02; 2] })
        .collect();
    let mut reference = ModulationState::default();
    let mut actual = ModulationState::default();
    let mut effect = Probe::default();
    let expected = block(&mut reference, &cfg, &mut Probe::default(), &input);
    let mut output = Vec::new();
    for count in [5, 7, 31, 1, 48, 64, 100] {
        let start = output.len();
        output.extend(block(
            &mut actual,
            &cfg,
            &mut effect,
            &input[start..start + count],
        ));
    }
    assert_eq!(output, expected);
    let before = actual.follower_reading();
    let mut transient = cfg.clone();
    transient.follower.detector.input = DetectorInput::Sidechain;
    block(&mut actual, &transient, &mut effect, &[]);
    assert_eq!(actual.follower_reading(), before);
}
#[test]
fn editing_an_off_route_cannot_resample_or_reapply_the_legacy_smoothed_source() {
    let mut cfg = config();
    for route in &mut cfg.follower.routes {
        route.enabled = false;
    }
    cfg.smooth_ms[1] = 10.0;
    cfg.msegs[1].nodes[0].value = 0.0;
    let mut actual = ModulationState::default();
    let mut reference = ModulationState::default();
    let mut effect = Probe::default();
    let mut other = Probe::default();
    block(&mut actual, &cfg, &mut effect, &[[0.0; 2]; 5]);
    block(&mut reference, &cfg, &mut other, &[[0.0; 2]; 5]);
    let unchanged = cfg.clone();
    cfg.follower.routes[1].amount = 2.0;
    assert_eq!(
        block(&mut actual, &cfg, &mut effect, &[[0.0; 2]; 8]),
        block(&mut reference, &unchanged, &mut other, &[[0.0; 2]; 8])
    );
    assert_eq!(effect.sets, other.sets);
}

#[test]
fn follower_state_round_trips_and_defaults_do_not_change_legacy_wire_data() {
    let cfg = config();
    let saved = serde_json::to_string(&cfg).unwrap();
    assert_eq!(
        serde_json::from_str::<TrackModulation>(&saved).unwrap(),
        cfg
    );
    assert!(
        !serde_json::to_string(&TrackModulation::default())
            .unwrap()
            .contains("follower")
    );
}
