use super::*;
use crate::effects::EffectKind;
use tract_effects::modulation::{AuditionSettings, AuditionSource, DetectorInput};

#[test]
fn monitor_none_inactive_row_overrides_global_mix_without_changing_clock_or_audio_history() {
    let mut actual = Box::new(AudioEngine::new());
    let mut reference = Box::new(AudioEngine::new());
    let grid = Grid::default();
    let tracks = [TrackEffect::default(); ROWS];
    let mut cfg = std::array::from_fn(TrackModulation::default_for_row);
    cfg[3].follower.detector.input = DetectorInput::Sidechain;
    for engine in [&mut actual, &mut reference] {
        engine.set_sample_rate(1000.0);
        engine.set_effects(&tracks);
        engine.set_modulation(&cfg);
    }
    let token = actual.telemetry.audition.begin(
        3,
        EffectKind::None,
        AuditionSource::Follower,
        AuditionSettings::for_source(&cfg[3], AuditionSource::Follower).unwrap(),
        actual.telemetry.waveform.generation(),
    );
    let (mut left, mut right) = ([0.1; 16], [-0.2; 16]);
    let (mut expected_l, mut expected_r) = (left, right);
    let side_l = [0.6; 16];
    let side_r = [-0.3; 16];
    let side = SidechainInput::new(&side_l, &side_r);
    assert_no_alloc::assert_no_alloc(|| {
        actual.process_with_inputs(
            &mut left,
            &mut right,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
        reference.process_with_inputs(
            &mut expected_l,
            &mut expected_r,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
    });
    assert_eq!([left[15], right[15]], [0.6, -0.3]);
    for row in 0..ROWS {
        for slot in 0..4 {
            assert_eq!(
                actual.modulation.precise_phase(row, slot),
                reference.modulation.precise_phase(row, slot)
            );
        }
    }
    actual.telemetry.audition.release(token);
    let (mut left, mut right) = ([0.1; 16], [-0.2; 16]);
    assert_no_alloc::assert_no_alloc(|| {
        actual.process_with_inputs(
            &mut left,
            &mut right,
            side,
            true,
            3.0,
            120.0,
            0.0,
            &grid,
            std::iter::empty(),
        );
    });
    assert_eq!(&left[5..], &[0.1; 11]);
    assert_eq!(&right[5..], &[-0.2; 11]);
}

#[test]
fn listen_realigns_on_the_midi_sample_that_changes_fft_latency() {
    use crate::effects::{default_params_for_kind, parameter_specs};
    use tiny_skia_widgets::mseg::{FactoryShape, WavePalette, factory_shape};
    use tract_effects::modulation::{MidiWaveMapping, ModulationTarget, TriggerSource};

    const RATE: f32 = 1000.0;
    const WINDOW: usize = 4;
    const SWITCH: usize = 12;
    const FIRST_OUTPUT: usize = SWITCH + WINDOW + 1024;
    let mut engine = Box::new(AudioEngine::new());
    engine.set_sample_rate(RATE);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[1].kind = EffectKind::SpectralShift;
    tracks[1].params = default_params_for_kind(tracks[1].kind);
    let fft = parameter_specs(tracks[1].kind)
        .iter()
        .position(|param| param.name == "FFT")
        .unwrap();
    tracks[1].params[fft] = 0.0;
    engine.set_effects(&tracks);

    let mut config = std::array::from_fn(TrackModulation::default_for_row);
    config[0].trigger = TriggerSource::TransientSample {
        threshold: 1.5,
        hold_ms: 5.0,
    };
    config[0].audio_trigger.input = DetectorInput::Sidechain;
    config[0].lookahead = true;
    config[1].targets = [Some(ModulationTarget::EffectParameter(fft)), None, None];
    config[1].depths[0] = 1.0 / 3.0;
    for node in &mut config[1].msegs[1].nodes {
        node.value = 0.0;
    }
    config[1].wave_switch = Some(MidiWaveMapping {
        envelope: 1,
        first_note: 36,
        channel: None,
    });
    engine.set_modulation(&config);
    let mut palette = WavePalette::default();
    palette
        .store(0, &factory_shape(FactoryShape::FlatHigh))
        .unwrap();
    engine.set_wave_palette(&palette);
    engine.prepare_lookahead();
    assert_eq!(engine.chain_latency_samples(), WINDOW + 512);
    let source = AuditionSource::Trigger;
    let token = engine.telemetry.audition.begin(
        0,
        EffectKind::None,
        source,
        AuditionSettings::for_source(&config[0], source).unwrap(),
        engine.telemetry.waveform.generation(),
    );

    let mut grid = Grid::default();
    grid.cell_mut(1, 0).enabled = false;
    let mut left = [0.0; FIRST_OUTPUT + 1];
    let mut right = left;
    let mut aux = left;
    aux[SWITCH - 1] = 0.5;
    aux[SWITCH] = 0.75;
    assert_no_alloc::assert_no_alloc(|| {
        engine.process(
            &mut [0.0; WINDOW + 512 + 8],
            &mut [0.0; WINDOW + 512 + 8],
            false,
            1000.0,
            120.0,
            0.0,
            &grid,
        );
        engine.process_with_inputs(
            &mut left,
            &mut right,
            SidechainInput::new(&aux, &aux),
            false,
            1000.0,
            120.0,
            0.0,
            &grid,
            std::iter::once(MidiNoteOn {
                timing: SWITCH,
                note: 36,
                channel: 0,
            }),
        );
    });

    assert_eq!(engine.chain_latency_samples(), WINDOW + 1024);
    assert!(engine.telemetry.audition.is_current(token));
    assert_eq!(left[..FIRST_OUTPUT], [0.0; FIRST_OUTPUT]);
    assert_eq!(left[FIRST_OUTPUT], 0.75 * 0.2);
    assert_eq!(left, right);
}
