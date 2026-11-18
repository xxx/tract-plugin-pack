use super::*;
use tract_effects::effects::{EffectKind, TrackEffect};
use tract_effects::modulation::{AuditionSettings, AuditionSource, DetectorInput, TriggerSource};

fn plugin_with_lookahead(rows: &[usize]) -> Box<Multosis> {
    let mut plugin = Box::new(Multosis::default());
    plugin.engine.set_sample_rate(48000.0);
    plugin.engine.set_compressor(0.0, 1.0);
    let tracks = [TrackEffect::default(); 16];
    assert!(tracks.iter().all(|track| track.kind == EffectKind::None));
    plugin.engine.set_effects(&tracks);
    *plugin.params.track_effects.lock().unwrap() = tracks;
    {
        let mut config = plugin.params.track_modulation.lock().unwrap();
        for &row in rows {
            config[row].trigger = TriggerSource::TransientSample {
                threshold: 1.5,
                hold_ms: 5.0,
            };
            config[row].lookahead = true;
        }
        plugin.engine.set_modulation(&config);
    }
    for cell in &mut plugin.grid.cells {
        cell.enabled = false;
    }
    plugin.engine.prepare_lookahead();
    plugin
}

#[test]
fn inactive_none_lookahead_advertises_a_tail_across_silent_callbacks_at_every_mix() {
    for rows in [&[0][..], &[0, 3, 7][..]] {
        for mix in [0.0, 0.5, 1.0] {
            let mut plugin = plugin_with_lookahead(rows);
            let delay = rows.len() * 192;
            let mut rendered = vec![0.0; delay + 128];
            for (block, left) in rendered.chunks_mut(64).enumerate() {
                if block == 0 {
                    left[0] = 0.75;
                }
                let mut right = [0.0; 64];
                right.copy_from_slice(left);
                let status = assert_no_alloc::assert_no_alloc(|| {
                    plugin.engine.process(
                        left,
                        &mut right,
                        false,
                        1000.0,
                        120.0,
                        mix,
                        &plugin.grid,
                    );
                    plugin.process_status()
                });
                if (block + 1) * 64 <= delay {
                    assert_eq!(left, [0.0; 64]);
                    assert_eq!(right, [0.0; 64]);
                    assert_eq!(status, ProcessStatus::Tail(delay as u32));
                }
                assert_eq!(right, left);
            }
            assert_eq!(plugin.engine.active_mask(), 0);
            for (sample, value) in rendered.into_iter().enumerate() {
                assert_eq!(value, if sample == delay { 0.75 } else { 0.0 });
            }
        }
    }
}

#[test]
fn default_off_retains_normal_status() {
    let mut plugin = plugin_with_lookahead(&[]);
    assert_no_alloc::assert_no_alloc(|| {
        plugin.engine.process(
            &mut [0.25; 64],
            &mut [0.25; 64],
            false,
            1000.0,
            120.0,
            0.0,
            &plugin.grid,
        );
    });
    assert_eq!(plugin.process_status(), ProcessStatus::Normal);
}

#[test]
fn tail_follows_adopted_paths_across_empty_calls_mute_solo_and_source_changes() {
    let mut plugin = plugin_with_lookahead(&[0, 3]);
    assert_eq!(plugin.process_status(), ProcessStatus::Tail(384));
    {
        let mut config = plugin.params.track_modulation.lock().unwrap();
        config[3].lookahead = false;
        plugin.engine.set_modulation(&config);
    }
    assert_no_alloc::assert_no_alloc(|| {
        plugin
            .engine
            .process(&mut [], &mut [], false, 1000.0, 120.0, 0.0, &plugin.grid);
    });
    assert_eq!(plugin.process_status(), ProcessStatus::Tail(384));
    assert_no_alloc::assert_no_alloc(|| {
        plugin.engine.process(
            &mut [0.0; 64],
            &mut [0.0; 64],
            false,
            1000.0,
            120.0,
            0.0,
            &plugin.grid,
        );
    });
    assert_eq!(plugin.process_status(), ProcessStatus::Tail(192));
    for (muted, soloed) in [(true, false), (false, true), (false, false)] {
        {
            let mut tracks = plugin.params.track_effects.lock().unwrap();
            tracks[0].muted = muted;
            tracks[1].soloed = soloed;
            plugin.engine.set_effects(&tracks);
        }
        assert_no_alloc::assert_no_alloc(|| {
            plugin.engine.process(
                &mut [0.0; 64],
                &mut [0.0; 64],
                false,
                1000.0,
                120.0,
                0.0,
                &plugin.grid,
            );
        });
        assert_eq!(
            plugin.process_status(),
            if muted || soloed {
                ProcessStatus::Normal
            } else {
                ProcessStatus::Tail(192)
            }
        );
    }
    {
        let mut config = plugin.params.track_modulation.lock().unwrap();
        config[0].trigger = TriggerSource::DawPosition;
        assert!(config[0].lookahead);
        plugin.engine.set_modulation(&config);
    }
    assert_no_alloc::assert_no_alloc(|| {
        plugin.engine.process(
            &mut [0.0; 64],
            &mut [0.0; 64],
            false,
            1000.0,
            120.0,
            0.0,
            &plugin.grid,
        );
    });
    assert_eq!(plugin.process_status(), ProcessStatus::Normal);
}

#[test]
fn sidechain_listen_tail_includes_effect_latency_during_silent_main_callbacks() {
    let mut plugin = plugin_with_lookahead(&[0]);
    {
        let mut tracks = plugin.params.track_effects.lock().unwrap();
        tracks[1].kind = EffectKind::WarpZone;
        tracks[1].params = crate::effects::default_params_for_kind(EffectKind::WarpZone);
        plugin.engine.set_effects(&tracks);
    }
    let settings = {
        let mut config = plugin.params.track_modulation.lock().unwrap();
        config[0].audio_trigger.input = DetectorInput::Sidechain;
        plugin.engine.set_modulation(&config);
        AuditionSettings::for_source(&config[0], AuditionSource::Trigger).unwrap()
    };
    let delay = 4096 + 192;
    assert_eq!(plugin.process_status(), ProcessStatus::Tail(delay as u32));
    let token = plugin.engine.telemetry.audition.begin(
        0,
        EffectKind::None,
        AuditionSource::Trigger,
        settings,
        plugin.engine.telemetry.waveform.generation(),
    );
    let mut warmup_left = vec![0.0; delay + 256];
    let mut warmup_right = vec![0.0; delay + 256];
    assert_no_alloc::assert_no_alloc(|| {
        plugin.engine.process(
            &mut warmup_left,
            &mut warmup_right,
            false,
            1000.0,
            120.0,
            0.0,
            &plugin.grid,
        );
    });
    let mut rendered = vec![0.0; delay + 128];
    for (block, left) in rendered.chunks_mut(64).enumerate() {
        let mut side = [0.0; 64];
        if block == 0 {
            side[0] = 0.75;
        }
        let mut right = [0.0; 64];
        let status = assert_no_alloc::assert_no_alloc(|| {
            plugin.engine.process_with_inputs(
                left,
                &mut right,
                SidechainInput::new(&side, &side),
                false,
                1000.0,
                120.0,
                0.0,
                &plugin.grid,
                std::iter::empty(),
            );
            plugin.process_status()
        });
        assert_eq!(status, ProcessStatus::Tail(delay as u32));
        assert_eq!(right, left);
    }
    for (sample, value) in rendered.into_iter().enumerate() {
        assert_eq!(value, if sample == delay { 0.75 } else { 0.0 });
    }
    plugin.engine.telemetry.audition.release(token);
}
