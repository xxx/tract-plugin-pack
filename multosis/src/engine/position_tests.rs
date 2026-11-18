use super::*;
use crate::effects::{EffectKind, default_params_for_kind};
use crate::grid::COLS;
use crate::modulation::{HostPosition, TriggerSource};
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};

fn setup() -> (Box<AudioEngine>, Grid) {
    let mut engine = Box::new(AudioEngine::new());
    engine.set_compressor(0.0, 1.0);
    let mut tracks = [TrackEffect::default(); ROWS];
    tracks[0].kind = EffectKind::Bitcrush;
    tracks[0].params = default_params_for_kind(tracks[0].kind);
    tracks[0].mix = 1.0;
    engine.set_effects(&tracks);
    let mut config = std::array::from_fn(TrackModulation::default_for_row);
    config[0].trigger = TriggerSource::DawPosition;
    config[0].targets = [None; 3];
    config[0].msegs = std::array::from_fn(|_| MsegData {
        sync_mode: SyncMode::Beat,
        beats: 4.0,
        play_mode: PlayMode::Cyclic,
        ..MsegData::default()
    });
    config[1].smooth_ms[0] = 10.0;
    engine.set_modulation(&config);
    let mut grid = Grid::default();
    for row in 0..ROWS {
        for col in 0..COLS {
            grid.cell_mut(row, col).enabled = row == 0;
        }
    }
    (engine, grid)
}
fn frame(offset: usize) -> HostPosition {
    HostPosition {
        playing: true,
        beats: Some(1.0 + offset as f64 / 24000.0),
        seconds: Some(0.5 + offset as f64 / 48000.0),
        bpm: 120.0,
        ..HostPosition::default()
    }
}

#[test]
fn transport_restart_keeps_missing_daw_domains_frozen_while_other_clocks_relock() {
    for smooth in [0.0, 10.0] {
        for (beats, seconds) in [(None, None), (None, Some(0.5)), (Some(1.0), None)] {
            let (mut engine, grid) = setup();
            let mut config = std::array::from_fn(TrackModulation::default_for_row);
            config[0].trigger = TriggerSource::DawPosition;
            config[0].targets = [None; 3];
            config[0].smooth_ms = [smooth; 4];
            config[0].msegs = std::array::from_fn(|_| MsegData {
                sync_mode: SyncMode::Beat,
                beats: 4.0,
                play_mode: PlayMode::Cyclic,
                ..MsegData::default()
            });
            config[0].msegs[1].sync_mode = SyncMode::Time;
            config[0].msegs[1].time_seconds = 2.0;
            config[1].smooth_ms[0] = smooth;
            engine.set_modulation(&config);
            let (mut before, mut restarted, mut resumed) = ([0.0; 4], [0.0; 4], [0.0; 2]);
            let mut free_before = 0.0;
            let mut free_restart = [0.0; 2];
            assert_no_alloc::assert_no_alloc(|| {
                engine.set_host_position(HostPosition {
                    playing: false,
                    beats: Some(3.0),
                    seconds: Some(1.5),
                    ..frame(0)
                });
                engine.process(&mut [0.37], &mut [0.37], false, 1e9, 120.0, 1.0, &grid);
                before = [
                    engine.modulation.phase(0, 0) as f64,
                    engine.modulation.phase(0, 1) as f64,
                    engine.modulation.precise_phase(0, 0),
                    engine.modulation.precise_phase(0, 1),
                ];
                free_before = engine.modulation.precise_phase(1, 0);
                engine.restart_transport();
                restarted = [
                    engine.modulation.phase(0, 0) as f64,
                    engine.modulation.phase(0, 1) as f64,
                    engine.modulation.precise_phase(0, 0),
                    engine.modulation.precise_phase(0, 1),
                ];
                free_restart = [
                    engine.modulation.phase(1, 0) as f64,
                    engine.modulation.precise_phase(1, 0),
                ];
                engine.set_host_position(HostPosition {
                    beats,
                    seconds,
                    ..frame(0)
                });
                engine.process(&mut [0.37], &mut [0.37], true, 1e9, 120.0, 1.0, &grid);
                resumed = [
                    engine.modulation.precise_phase(0, 0),
                    engine.modulation.precise_phase(0, 1),
                ];
            });
            assert_eq!(before, [0.75; 4]);
            assert_eq!(
                restarted, before,
                "Smooth {smooth}, beats {beats:?}, seconds {seconds:?}"
            );
            assert!(free_before > 0.0);
            assert_eq!(free_restart, [0.0; 2]);
            assert_eq!(
                resumed,
                [
                    beats.map_or(0.75, |value| value / 4.0),
                    seconds.map_or(0.75, |value| value / 2.0)
                ]
            );
        }
    }
}

#[test]
fn daw_rows_keep_phase_and_audio_across_grid_and_host_boundaries_without_allocating() {
    let (mut actual, grid) = setup();
    let (mut reference, _) = setup();
    let (mut left, mut right) = ([0.37; 256], [0.37; 256]);
    let (mut expected_l, mut expected_r) = (left, right);
    assert_no_alloc::assert_no_alloc(|| {
        reference.set_host_position(frame(0));
        reference.process(
            &mut expected_l,
            &mut expected_r,
            true,
            1e9,
            120.0,
            1.0,
            &grid,
        );
        let mut offset = 0;
        for count in [5, 7, 31, 1, 48, 64, 100] {
            actual.set_host_position(frame(offset));
            actual.process(
                &mut left[offset..offset + count],
                &mut right[offset..offset + count],
                true,
                17.0,
                120.0,
                1.0,
                &grid,
            );
            offset += count;
        }
    });
    for ((l, r), (el, er)) in left
        .iter()
        .zip(&right)
        .zip(expected_l.iter().zip(&expected_r))
    {
        assert!((l - el).abs() < 1e-7);
        assert!((r - er).abs() < 1e-7);
    }
    assert!(
        (actual.modulation.precise_phase(0, 0) - reference.modulation.precise_phase(0, 0)).abs()
            < 1e-12
    );
    assert_eq!(
        actual.modulation.precise_phase(1, 0),
        reference.modulation.precise_phase(1, 0)
    );
    assert!(
        actual
            .modulation
            .frame(0, EffectKind::Bitcrush)
            .position_sync
    );
}
